//! `dotbox install` et `dotbox remove` (Journal, étape 7): une app du catalogue dans /opt/<app>,
//! routée par Traefik sur <sous-domaine>.<domaine>, derrière Authelia selon son niveau d'accès.
//! Même sortie que dotbox init: une ligne par app, le détail dans /opt/dotbox/logs/.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::catalog::{self, Access};
use crate::config::{self, Config};
use crate::engine::{self, Ctx};
use crate::out::say;
use crate::{authelia, docker, log, prompt, system, traefik, ui};

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Apps to install
    #[arg(required = true, value_name = "APP")]
    apps: Vec<String>,

    /// Read the catalog from this folder instead of /opt/dotbox/catalog (for tests)
    #[arg(long, value_name = "DIR")]
    catalog: Option<PathBuf>,

    /// Show every sub-step as it happens (they are always in the log)
    #[arg(long)]
    verbose: bool,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    /// App to remove: its configuration stays in /opt/APP
    #[arg(value_name = "APP")]
    app: String,

    /// Also delete /opt/APP and the image, after a confirmation
    #[arg(long)]
    purge: bool,

    /// Show every sub-step as it happens (they are always in the log)
    #[arg(long)]
    verbose: bool,
}

/// « prowlarr » devient « Prowlarr » sur la ligne de l'étape.
fn title(app: &str) -> String {
    let mut t = app.to_string();
    if let Some(c) = t.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    t
}

/// Root, le log de ce lancement, l'affichage.
fn begin(kind: &str, what: &str, steps: usize, verbose: bool) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err(format!(
            "run dotbox {kind} as root: sudo dotbox {kind} {what}"
        ));
    }
    let stamp = system::run("date", &["+%Y-%m-%d-%H%M"])?;
    let now = system::run("date", &["+%Y-%m-%d %H:%M:%S %Z"]).unwrap_or_default();
    log::open(
        kind,
        &stamp,
        &format!(
            "dotbox {kind} {what} {}, started {now}",
            env!("CARGO_PKG_VERSION")
        ),
    )?;
    ui::start_steps(steps, verbose);
    Ok(())
}

/// Une confirmation refusée: rien n'a changé, ce n'est pas une erreur (ui::cancel).
const CANCELLED: &str = "cancelled, nothing changed";

/// Une erreur dans une étape est déjà affichée par ui::fail, avec la fin du log. Une annulation
/// s'affiche en ligne neutre, et le code de retour reste 1: rien n'a été fait.
fn finish(result: Result<(), String>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e == CANCELLED && ui::cancel(&e) => Err(String::new()),
        Err(e) if ui::fail(&e) => Err(String::new()),
        Err(e) => Err(e),
    }
}

/// Le domaine et l'utilisateur, posés par dotbox init.
fn server(cfg: &Config) -> Result<(String, String), String> {
    let domain = cfg
        .get("domain")
        .ok_or("no domain in /opt/dotbox/dotbox.yml: run sudo dotbox init first")?;
    let user = cfg
        .get("user")
        .ok_or("no user in /opt/dotbox/dotbox.yml: run sudo dotbox init first")?;
    Ok((domain, user))
}

/// Les fichiers compose d'une app: le sien, et celui de l'utilisateur s'il existe.
fn compose_files(app: &str) -> Vec<String> {
    let mut files = vec!["-f".to_string(), engine::compose_path(app)];
    let over = format!("{}/compose.override.yml", engine::dir(app));
    if Path::new(&over).exists() {
        files.push("-f".into());
        files.push(over);
    }
    files
}

fn compose(app: &str, args: &[&str]) -> Result<String, String> {
    let mut all: Vec<String> = vec!["compose".into()];
    all.extend(compose_files(app));
    all.extend(args.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    system::run("docker", &refs)
}

/// /opt/<app>/compose.yml existe sans avoir été écrit par dotbox: on n'y touche pas.
fn foreign_compose(app: &str) -> Option<String> {
    let path = engine::compose_path(app);
    match std::fs::read_to_string(&path) {
        Ok(text) if !text.starts_with(engine::HEADER) => Some(format!(
            "{path} was not written by dotbox: move /opt/{app} away, then run it again"
        )),
        _ => None,
    }
}

/// Attend que l'app tourne: healthy si elle a un healthcheck, running sinon.
fn wait_ready(app: &str, health: bool, timeout: Duration) -> Result<String, String> {
    let start = Instant::now();
    loop {
        let state = traefik::container_state(app);
        let ready = if health {
            state == "healthy"
        } else {
            state == "running"
        };
        if ready {
            return Ok(state);
        }
        if state == "unhealthy" || state == "exited" || start.elapsed() > timeout {
            let logs = system::run("docker", &["logs", "--tail", "20", app]).unwrap_or_default();
            return Err(format!(
                "{app} is not ready after {}s ({state}), last logs:\n{logs}",
                start.elapsed().as_secs()
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

pub fn install(args: &InstallArgs) -> Result<(), String> {
    // le catalogue rangé par dotbox init (D64), lu sur GitHub s'il n'y est pas encore
    let dir = match &args.catalog {
        Some(dir) => dir.clone(),
        None => PathBuf::from(crate::github::DIR),
    };
    let dir = &dir;
    let mut apps: Vec<&str> = Vec::new();
    for a in &args.apps {
        if !apps.contains(&a.as_str()) {
            apps.push(a);
        }
    }
    begin("install", &apps.join(" "), apps.len(), args.verbose)?;
    let mut ready = Vec::new();
    let result = (|| {
        if args.catalog.is_none() {
            let source = match crate::github::current() {
                Some(s) => s,
                None => crate::github::fetch()?,
            };
            say(
                "read",
                &format!(
                    "catalog {} of {} in {}",
                    source.short(),
                    source.date,
                    dir.display()
                ),
            );
        }
        let (domain, user) = server(&Config::load()?)?;
        if !authelia::apps_rule_present(&domain) {
            return Err(
                "the Authelia rule for the apps is missing: run sudo dotbox init, then try again"
                    .into(),
            );
        }
        for app in &apps {
            ready.push(install_one(dir, app, &domain, &user)?);
        }
        Ok(())
    })();
    finish(result)?;
    eprintln!();
    for (app, address, access) in ready {
        eprintln!(
            "  {:<13} {}  {}",
            app,
            console::style(address).underlined(),
            console::style(access).dim()
        );
    }
    eprintln!();
    Ok(())
}

/// Installe une app (ou la remet d'aplomb si elle l'est déjà). Rend son nom, son adresse et
/// son niveau d'accès.
fn install_one(
    dir: &Path,
    app: &str,
    domain: &str,
    user: &str,
) -> Result<(String, String, String), String> {
    ui::step(&title(app), "from the catalog");
    let m = catalog::load(dir, app)?;
    say(
        "read",
        &format!(
            "{}/{app}.yml: {}, {}",
            dir.display(),
            m.description,
            m.image
        ),
    );
    if let Some(e) = foreign_compose(app) {
        return Err(e);
    }

    // le sous-domaine et le niveau: ceux de dotbox.yml s'ils y sont, sinon ceux du manifest
    let mut cfg = Config::load()?;
    let (sub, access) = match cfg.app(app) {
        Some((s, a)) => (
            if s.is_empty() { app.to_string() } else { s },
            Access::parse(&a).unwrap_or(m.access),
        ),
        None => (app.to_string(), m.access),
    };
    if catalog::RESERVED.contains(&sub.as_str()) {
        return Err(format!("the subdomain {sub} is taken by the core"));
    }

    let (uid, gid) = system::user_ids(user)?;
    let ctx = Ctx {
        domain: domain.to_string(),
        subdomain: sub.clone(),
        access,
        uid,
        gid,
        tz: system::timezone(),
        subnet: docker::network_subnet()?,
    };
    for v in &m.volumes {
        // /data: ses dossiers et la vérification des hardlinks, plus bas
        if v.host != "/data" && !Path::new(&v.host).exists() {
            system::owned_dir(&v.host, uid, gid)?;
            say("create", &format!("{} (owned by {user})", v.host));
        }
    }
    if m.uses_data() {
        prepare_data(&m, uid, gid, user)?;
    }
    let settings_changed = write_settings(&m, &ctx, uid, gid)?;
    if !m.ports.is_empty() && !running(&traefik::container_state(app)) {
        check_ports(&m)?;
    }
    let path = engine::compose_path(app);
    let changed = system::write_if_changed(&path, &engine::compose(&m, &ctx), 0o644)?;
    if changed {
        say("write", &path);
    }
    cfg.set_app(app, &sub, access.as_str());
    if system::write_if_changed(config::PATH, &cfg.render()?, 0o644)? {
        say("write", &format!("{} (apps: {app})", config::PATH));
    }

    docker::pull(&m.image)?;
    // de nouveaux réglages: write_settings a arrêté l'app, elle repart ici et les relit
    let state = traefik::container_state(app);
    if changed || settings_changed || !running(&state) {
        compose(app, &["up", "-d", "--quiet-pull"])?;
        say("start", app);
    }
    let state = wait_ready(app, m.health.is_some(), Duration::from_secs(180))?;
    say("check", &format!("{app} {state} ({})", m.image));
    ui::done(&format!("{}, {}", m.version(), access.as_str()));
    Ok((
        app.to_string(),
        format!("https://{sub}.{domain}"),
        access.as_str().to_string(),
    ))
}

/// Le container tourne (avec ou sans healthcheck).
fn running(state: &str) -> bool {
    matches!(state, "running" | "healthy" | "starting" | "unhealthy")
}

/// Les ports que l'app publie (D44) sont libres sur le serveur, avant son premier démarrage.
fn check_ports(m: &catalog::Manifest) -> Result<(), String> {
    let tcp = traefik::parse_listening_ports(&system::run("ss", &["-Htln"])?);
    let udp = traefik::parse_listening_ports(&system::run("ss", &["-Huln"])?);
    for p in &m.ports {
        let busy = if p.proto == "tcp" { &tcp } else { &udp };
        if busy.contains(&p.number) {
            return Err(format!(
                "port {}/{} is already used on this server: {} publishes it (D44), free it first",
                p.number, p.proto, m.name
            ));
        }
        say("check", &format!("port {}/{} free", p.number, p.proto));
    }
    Ok(())
}

/// /data, /data/torrents, /data/media et les dossiers que l'app demande, à l'utilisateur. torrents/
/// et media/ doivent être sur le même système de fichiers, sinon pas de hardlink (Journal, 7.9).
fn prepare_data(m: &catalog::Manifest, uid: u32, gid: u32, user: &str) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let mut dirs: Vec<String> = ["/data", "/data/torrents", "/data/media"]
        .iter()
        .map(|d| d.to_string())
        .collect();
    dirs.extend(m.data.iter().map(|d| format!("/data/{d}")));
    for d in &dirs {
        if !Path::new(d).exists() {
            system::owned_dir(d, uid, gid)?;
            say("create", &format!("{d} (owned by {user})"));
        }
    }
    let dev = |p: &str| {
        std::fs::metadata(p)
            .map(|md| md.dev())
            .map_err(|e| format!("cannot read {p}: {e}"))
    };
    if dev("/data/torrents")? != dev("/data/media")? {
        return Err("/data/torrents and /data/media are on different filesystems: a hardlink between them is impossible. Put them on the same disk, then run dotbox install again".into());
    }
    say(
        "check",
        "/data/torrents and /data/media on the same filesystem (hardlinks)",
    );
    Ok(())
}

/// Pose les réglages du manifest dans le fichier de configuration de l'app, sans toucher au
/// reste. Rend true si le fichier a changé.
fn write_settings(m: &catalog::Manifest, ctx: &Ctx, uid: u32, gid: u32) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let Some(st) = &m.settings else {
        return Ok(false);
    };
    let path = format!("{}/{}", engine::dir(&m.name), st.file);
    if let Some(parent) = Path::new(&path).parent() {
        if !parent.exists() {
            system::owned_dir(&parent.to_string_lossy(), uid, gid)?;
        }
    }
    let values: Vec<(String, String)> = st
        .values
        .iter()
        .map(|(k, v)| (k.clone(), engine::fill(v, &m.name, ctx)))
        .collect();
    let keys: Vec<&str> = values.iter().map(|(k, _)| k.as_str()).collect();
    let merge = |old: Option<&str>| {
        match st.format.as_str() {
            "xml" => engine::merge_xml(old, &values),
            "ini" => engine::merge_ini(old, &values),
            other => Err(format!("format {other} not supported")),
        }
        .map_err(|e| format!("{path}: {e}"))
    };
    let mut old = std::fs::read_to_string(&path).ok();
    let mut new = merge(old.as_deref())?;
    if old.as_deref() == Some(new.as_str()) {
        say("check", &format!("{path} ({})", keys.join(", ")));
        return Ok(false);
    }
    // une app qui tourne réécrit parfois son fichier en s'arrêtant (qBittorrent): elle s'arrête
    // d'abord, dotbox relit le fichier puis écrit, et elle repart ensuite (install_one)
    if running(&traefik::container_state(&m.name)) {
        system::run("docker", &["stop", &m.name])?;
        say("stop", &format!("{} (new settings)", m.name));
        old = std::fs::read_to_string(&path).ok();
        new = merge(old.as_deref())?;
    }
    std::fs::write(&path, &new).map_err(|e| format!("cannot write {path}: {e}"))?;
    if old.is_none() {
        // il contiendra la clé d'API de l'app: lisible par l'app seule
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot set the permissions of {path}: {e}"))?;
    }
    std::os::unix::fs::chown(&path, Some(uid), Some(gid))
        .map_err(|e| format!("cannot give {path} to {uid}:{gid}: {e}"))?;
    say("write", &format!("{path} ({})", keys.join(", ")));
    Ok(true)
}

pub fn remove(args: &RemoveArgs) -> Result<(), String> {
    let app = args.app.as_str();
    if !catalog::valid_name(app) || catalog::RESERVED.contains(&app) {
        return Err(format!("{app} is not an app that dotbox can remove"));
    }
    begin("remove", app, 1, args.verbose)?;
    let what = if args.purge {
        "the app, its configuration and its image"
    } else {
        "the app, its configuration is kept"
    };
    ui::step(&title(app), what);
    finish(remove_steps(app, args.purge))
}

fn remove_steps(app: &str, purge: bool) -> Result<(), String> {
    let mut cfg = Config::load()?;
    let path = engine::compose_path(app);
    let dir = engine::dir(app);
    let has_compose = Path::new(&path).exists();
    if cfg.app(app).is_none() && !has_compose {
        return Err(format!("{app} is not installed"));
    }
    if let Some(e) = foreign_compose(app) {
        return Err(e);
    }
    // la confirmation avant tout changement (D42)
    if purge {
        if !prompt::is_terminal() {
            return Err("--purge asks for a confirmation: run it in a terminal".into());
        }
        let question = format!("Delete {dir} and everything in it?");
        if !ui::ask(|| prompt::confirm(&question, "←/→ to choose, Enter to confirm"))? {
            return Err(CANCELLED.into());
        }
    }

    if has_compose {
        if purge {
            compose(app, &["down", "--rmi", "all"])?;
            say("stop", &format!("{app}, image removed"));
        } else {
            compose(app, &["down"])?;
            say("stop", app);
        }
        std::fs::remove_file(&path).map_err(|e| format!("cannot delete {path}: {e}"))?;
        say("delete", &path);
    }
    if cfg.remove_app(app) {
        system::write_if_changed(config::PATH, &cfg.render()?, 0o644)?;
        say("write", &format!("{} (apps: without {app})", config::PATH));
    }
    if purge {
        if Path::new(&dir).exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot delete {dir}: {e}"))?;
            say("delete", &dir);
        }
        ui::done(&format!("removed, {dir} deleted"));
    } else {
        ui::done(&format!("removed, configuration kept in {dir}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title("prowlarr"), "Prowlarr");
        assert_eq!(title("radarr-4k"), "Radarr-4k");
    }
}
