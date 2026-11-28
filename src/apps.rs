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
    pub apps: Vec<String>,

    /// Read the catalog from this folder instead of /opt/dotbox/catalog (for tests, with --local)
    #[arg(long, value_name = "DIR")]
    pub catalog: Option<PathBuf>,

    /// The access level instead of the manifest's: two_factor, one_factor, app or public
    #[arg(long, value_name = "LEVEL")]
    pub access: Option<String>,

    /// Show every sub-step as it happens (they are always in the log)
    #[arg(long)]
    pub verbose: bool,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    /// App to remove: its configuration stays in /opt/APP
    #[arg(value_name = "APP")]
    pub app: String,

    /// Also delete /opt/APP and the image, after a confirmation
    #[arg(long)]
    pub purge: bool,

    /// Set from the global --yes
    #[arg(skip)]
    pub yes: bool,

    /// Show every sub-step as it happens (they are always in the log)
    #[arg(long)]
    pub verbose: bool,
}

/// « prowlarr » devient « Prowlarr » sur la ligne de l'étape.
pub fn title(app: &str) -> String {
    let mut t = app.to_string();
    if let Some(c) = t.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    t
}

/// Root, le log de ce lancement, l'affichage.
pub fn begin(kind: &str, what: &str, steps: usize, verbose: bool) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err(format!(
            "run dotbox {kind} --local as root: sudo dotbox {kind} {what} --local"
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
pub const CANCELLED: &str = "cancelled, nothing changed";

/// Une erreur dans une étape est déjà affichée par ui::fail, avec la fin du log. Une annulation
/// s'affiche en ligne neutre, et le code de retour reste 1: rien n'a été fait.
pub fn finish(result: Result<(), String>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e == CANCELLED && ui::cancel(&e) => Err(String::new()),
        Err(e) if ui::fail(&e) => Err(String::new()),
        Err(e) => Err(e),
    }
}

/// Le domaine et l'utilisateur, posés par dotbox init.
pub fn server(cfg: &Config) -> Result<(String, String), String> {
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

pub fn compose(app: &str, args: &[&str]) -> Result<String, String> {
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
pub fn wait_ready(app: &str, health: bool, timeout: Duration) -> Result<String, String> {
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
        let level = match &args.access {
            None => None,
            Some(a) => Some(catalog::Access::parse(a).ok_or_else(|| {
                format!("unknown access {a:?}: two_factor, one_factor, app or public")
            })?),
        };
        if !authelia::apps_rule_present(&domain) {
            return Err(
                "the Authelia rule for the apps is missing: run sudo dotbox init, then try again"
                    .into(),
            );
        }
        for app in &apps {
            // l'installer, c'est la vouloir en marche: plus arrêtée exprès (D81)
            let mut cfg = Config::load()?;
            if cfg.set_stopped(app, false) {
                system::write_if_changed(config::PATH, &cfg.render()?, 0o644)?;
                say(
                    "write",
                    &format!("{} ({app}: no longer stopped)", config::PATH),
                );
            }
            // --access (D178): le niveau écrit dans dotbox.yml avant le compose.yml de l'app
            if let Some(level) = level {
                let sub = cfg.app(app).map(|(s, _)| s).unwrap_or_default();
                cfg.set_app(app, &sub, level.as_str());
                if system::write_if_changed(config::PATH, &cfg.render()?, 0o644)? {
                    say(
                        "write",
                        &format!("{} ({app}: access {})", config::PATH, level.as_str()),
                    );
                }
            }
            ready.push(install_one(dir, app, &domain, &user, None, true)?);
        }
        // une app en one_factor a sa règle dans Authelia: il ne redémarre que si elles changent
        authelia::refresh_rules(&Config::load()?, &domain)?;
        Ok(())
    })();
    finish(result)?;
    if ui::events() {
        ui::result(serde_json::to_value(&ready).unwrap_or_default());
    } else {
        print_ready(&ready);
    }
    Ok(())
}

/// Le résumé de la fin: l'adresse de chaque app, et ce qu'il reste à faire pour celles qui
/// attendent leur assistant de premier démarrage.
pub fn print_ready(ready: &[Ready]) {
    if crate::out::quiet() {
        return;
    }
    eprintln!();
    for r in ready {
        let access = if r.setup_pending {
            "two_factor for now".to_string()
        } else {
            r.access.clone()
        };
        eprintln!(
            "  {:<13} {}  {}",
            r.app,
            console::style(&r.address).underlined(),
            console::style(access).dim()
        );
    }
    eprintln!();
    for r in ready.iter().filter(|r| r.setup_pending) {
        eprintln!(
            "  {} Open {} and finish its first-run setup (Authelia asks you to log in first).",
            console::style("→").cyan(),
            r.address
        );
        eprintln!(
            "    Then run dotbox install {} again: Authelia steps aside ({}).",
            r.app, r.access
        );
        eprintln!();
    }
}

/// Ce que dotbox écrirait pour une app, sans rien écrire ni lancer: le manifest, le contexte et
/// le compose.yml. install_one l'écrit, dotbox diff et dotbox sync le comparent au fichier (15.2).
pub struct Wanted {
    pub m: catalog::Manifest,
    pub ctx: Ctx,
    pub sub: String,
    /// le niveau voulu (dotbox.yml ou le manifest)
    pub access: Access,
    /// derrière Authelia tant que son assistant de premier démarrage n'est pas fini (D71)
    pub setup_pending: bool,
    pub compose: String,
}

/// Des images à la place de celles du catalogue, par container (l'app, puis <app>-<service>):
/// celles d'avant pour dotbox update --rollback (13.6), celles qui tournent pour dotbox sync, qui
/// ne change jamais de version. Un container absent de la liste garde l'image du catalogue.
pub type Pins = [(String, String)];

/// L'image épinglée d'un container, s'il y en a une.
pub fn pinned<'a>(pins: &'a Pins, container: &str) -> Option<&'a str> {
    pins.iter()
        .find(|(c, _)| c == container)
        .map(|(_, i)| i.as_str())
}

/// `image`: voir Pins.
pub fn wanted(
    dir: &Path,
    app: &str,
    domain: &str,
    user: &str,
    image: Option<&Pins>,
) -> Result<Wanted, String> {
    let from = crate::add::manifest_dir(dir, app);
    let mut m = catalog::load(&from, app)?;
    if let Some(p) = image {
        if let Some(i) = pinned(p, app) {
            m.image = i.to_string();
        }
        for s in &mut m.services {
            if let Some(i) = pinned(p, &s.container(app)) {
                s.image = i.to_string();
            }
        }
    }
    // le sous-domaine et le niveau: ceux de dotbox.yml s'ils y sont, sinon ceux du manifest
    let cfg = Config::load()?;
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
    // ouverte à Internet, mais son assistant de premier démarrage n'est pas fini: derrière
    // Authelia en attendant, pour que personne ne crée le compte admin avant toi (D71). Un
    // assistant qu'on ne peut pas interroger (l'app ne tourne pas): ce que dit le compose écrit.
    let setup_pending = !access.behind_authelia()
        && m.setup.is_some()
        && match setup_done(&m) {
            Some(done) => !done,
            None => {
                !Path::new(&engine::compose_path(app)).exists()
                    || crate::status::setup_pending(app, access.as_str())
            }
        };
    let (uid, gid) = system::user_ids(user)?;
    let ctx = Ctx {
        domain: domain.to_string(),
        subdomain: sub.clone(),
        access: if setup_pending {
            Access::TwoFactor
        } else {
            access
        },
        uid,
        gid,
        tz: system::timezone(),
        subnet: docker::network_subnet()?,
    };
    let compose = engine::compose(&m, &ctx);
    Ok(Wanted {
        m,
        ctx,
        sub,
        access,
        setup_pending,
        compose,
    })
}

/// Installe une app (ou la remet d'aplomb si elle l'est déjà). Rend son nom, son adresse et
/// son niveau d'accès.
/// `image`: voir wanted. `start`: false pour une app arrêtée exprès (D81): tout est écrit, elle
/// reste arrêtée (update et sync).
pub fn install_one(
    dir: &Path,
    app: &str,
    domain: &str,
    user: &str,
    image: Option<&Pins>,
    start: bool,
) -> Result<Ready, String> {
    let from = crate::add::manifest_dir(dir, app);
    let doing = match image.and_then(|p| pinned(p, app)) {
        Some(i) => format!("image {}", crate::status::tag(i)),
        None if from != dir => "from your apps (dotbox add)".into(),
        None => "from the catalog".into(),
    };
    ui::step(&title(app), &doing);
    if let Some(e) = foreign_compose(app) {
        return Err(e);
    }
    let Wanted {
        m,
        mut ctx,
        sub,
        access,
        mut setup_pending,
        compose: text,
    } = wanted(dir, app, domain, user, image)?;
    say(
        "read",
        &format!(
            "{}/{app}.yml: {}, {}",
            dir.display(),
            m.description,
            m.image
        ),
    );
    if setup_pending {
        say(
            "check",
            &format!("{app}: first-run setup not done yet, behind Authelia until then"),
        );
    }
    let mut cfg = Config::load()?;
    let (uid, gid) = (ctx.uid, ctx.gid);
    let service_volumes = m.services.iter().flat_map(|s| s.volumes.iter());
    for v in m.volumes.iter().chain(service_volumes) {
        // /data: ses dossiers et la vérification des hardlinks, plus bas
        if !catalog::in_data(&v.host) && !Path::new(&v.host).exists() {
            system::owned_dir(&v.host, uid, gid)?;
            say("create", &format!("{} (owned by {user})", v.host));
        }
    }
    if m.uses_data() {
        prepare_data(&m, uid, gid, user)?;
    }
    load_modules(&m)?;
    write_secrets(&m, &ctx)?;
    let settings_changed = write_settings(&m, &ctx, uid, gid)?;
    if !m.ports.is_empty() && !running(&traefik::container_state(app)) {
        check_ports(&m)?;
    }
    let path = engine::compose_path(app);
    let changed = system::write_if_changed(&path, &text, 0o644)?;
    if changed {
        say("write", &path);
    }
    remember_compose(app)?;
    cfg.set_app(app, &sub, access.as_str());
    if system::write_if_changed(config::PATH, &cfg.render()?, 0o644)? {
        say("write", &format!("{} (apps: {app})", config::PATH));
    }

    for (_, image) in m.images() {
        docker::pull(&image)?;
    }
    if !start {
        // arrêtée exprès (D81): tout est prêt, elle reste arrêtée. Un container qui tourne
        // encore (ses réglages l'ont arrêté, ou il a été lancé à la main) s'arrête.
        if running(&traefik::container_state(app)) {
            compose(app, &["stop"])?;
            say("stop", app);
        }
        if let Some(w) = &m.crowdsec {
            crate::crowdsec::watch_app(app, &w.collection, &w.kind)?;
        }
        ui::done(&format!("{}, stopped (dotbox stop)", m.version()));
        return Ok(Ready {
            app: app.to_string(),
            address: format!("https://{sub}.{domain}"),
            access: access.as_str().to_string(),
            setup_pending,
        });
    }
    // de nouveaux réglages: write_settings a arrêté l'app, elle repart ici et les relit
    let state = traefik::container_state(app);
    if changed || settings_changed || !running(&state) {
        compose(app, &["up", "-d", "--quiet-pull"])?;
        say("start", app);
    }
    let mut state = wait_ready(app, m.health.is_some(), Duration::from_secs(180))?;
    // un fichier de réglages que l'app crée elle-même (Seerr): il existe maintenant, dotbox y
    // pose ses réglages (write_settings arrête l'app), puis la relance
    if let Some(st) = m.settings.as_ref().filter(|st| st.created_by_app) {
        let path = format!("{}/{}", engine::dir(app), st.file);
        let start = Instant::now();
        while !Path::new(&path).exists() {
            if start.elapsed() > Duration::from_secs(60) {
                return Err(format!("{app} did not create {path}"));
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        if write_settings(&m, &ctx, uid, gid)? {
            compose(app, &["up", "-d", "--quiet-pull"])?;
            say("start", app);
            state = wait_ready(app, m.health.is_some(), Duration::from_secs(180))?;
        }
    }
    // l'assistant demandé à l'app (Wizarr): elle ne pouvait pas répondre avant de démarrer.
    // Déjà fait (une réinstallation): elle passe tout de suite au niveau voulu.
    if setup_pending
        && matches!(m.setup, Some(catalog::Setup::Probe { .. }))
        && setup_done(&m) == Some(true)
    {
        setup_pending = false;
        ctx.access = access;
        say("check", &format!("{app}: first-run setup already done"));
        if system::write_if_changed(&path, &engine::compose(&m, &ctx), 0o644)? {
            say("write", &path);
            remember_compose(app)?;
            compose(app, &["up", "-d", "--quiet-pull"])?;
            say("start", app);
            state = wait_ready(app, m.health.is_some(), Duration::from_secs(180))?;
        }
    }
    say("check", &format!("{app} {state} ({})", m.image));
    // CrowdSec lit ses logs et bannit qui devine les mots de passe (D72)
    if let Some(w) = &m.crowdsec {
        crate::crowdsec::watch_app(app, &w.collection, &w.kind)?;
    }
    if setup_pending {
        ui::done(&format!("{}, two_factor until set up", m.version()));
    } else {
        ui::done(&format!("{}, {}", m.version(), access.as_str()));
    }
    Ok(Ready {
        app: app.to_string(),
        address: format!("https://{sub}.{domain}"),
        access: access.as_str().to_string(),
        setup_pending,
    })
}

/// Les secrets de l'app: tirés une seule fois, jamais réécrits (D151). Un secret composé est
/// écrit à partir des autres, et réécrit seulement s'il change (D159). Tous lisibles dans les
/// containers qui les montent.
fn write_secrets(m: &catalog::Manifest, ctx: &Ctx) -> Result<(), String> {
    let app = &m.name;
    for s in m
        .secrets
        .iter()
        .filter(|s| !m.composed.iter().any(|(c, _)| c == *s))
    {
        let name = engine::secret_name(app, s);
        if crate::secrets::ensure_hex(&name, 64)? {
            say("create", &crate::secrets::path(&name));
        } else {
            say("check", &crate::secrets::path(&name));
        }
        crate::secrets::mountable(&name)?;
    }
    for (s, template) in &m.composed {
        let mut value = template.clone();
        for p in &m.secrets {
            let key = format!("{{{{secret:{p}}}}}");
            if value.contains(&key) {
                let v = crate::secrets::read(&engine::secret_name(app, p))
                    .ok_or(format!("the secret {p} of {app} is missing"))?;
                value = value.replace(&key, &v);
            }
        }
        let value = engine::fill(&value, app, ctx);
        let name = engine::secret_name(app, s);
        if crate::secrets::read(&name).as_deref() != Some(value.as_str()) {
            crate::secrets::store(&name, &value)?;
            say("write", &crate::secrets::path(&name));
        } else {
            say("check", &crate::secrets::path(&name));
        }
        crate::secrets::mountable(&name)?;
    }
    Ok(())
}

/// L'empreinte du compose.yml que dotbox vient d'écrire: dotbox diff voit s'il a été changé à la
/// main depuis (13.7).
pub fn compose_sum_path(app: &str) -> String {
    format!("{}/.dotbox-compose.sha256", engine::dir(app))
}

fn remember_compose(app: &str) -> Result<(), String> {
    let sum = crate::crowdsec::sha256(&engine::compose_path(app)).unwrap_or_default();
    system::write_if_changed(&compose_sum_path(app), &format!("{sum}\n"), 0o644)?;
    Ok(())
}

/// Les modules du noyau que l'app demande (wireguard): chargés maintenant, et à chaque démarrage
/// du serveur (/etc/modules-load.d/dotbox-<app>.conf). Le container n'a pas SYS_MODULE.
fn load_modules(m: &catalog::Manifest) -> Result<(), String> {
    if m.kernel_modules.is_empty() {
        return Ok(());
    }
    let conf = format!("/etc/modules-load.d/dotbox-{}.conf", m.name);
    let text = format!(
        "# Écrit par dotbox install {}: les modules du noyau dont il a besoin.\n{}\n",
        m.name,
        m.kernel_modules.join("\n")
    );
    if system::write_if_changed(&conf, &text, 0o644)? {
        say("write", &conf);
    }
    for module in &m.kernel_modules {
        system::run("modprobe", &[module.as_str()])?;
        say("check", &format!("kernel module {module} loaded"));
    }
    Ok(())
}

/// L'assistant de premier démarrage est-il fini? None: on ne peut pas encore le savoir (l'app
/// ne tourne pas, pour un assistant demandé à l'app).
fn setup_done(m: &catalog::Manifest) -> Option<bool> {
    match m.setup.as_ref()? {
        catalog::Setup::File { file, done } => {
            let path = format!("{}/{}", engine::dir(&m.name), file);
            Some(std::fs::read_to_string(path).is_ok_and(|t| t.contains(done.as_str())))
        }
        catalog::Setup::Probe { probe, pending } => {
            if !running(&traefik::container_state(&m.name)) {
                return None;
            }
            let url = format!("http://localhost:{}{probe}", m.port);
            let to = system::run(
                "docker",
                &[
                    "exec",
                    &m.name,
                    "curl",
                    "-s",
                    "-o",
                    "/dev/null",
                    "-w",
                    "%{redirect_url}",
                    &url,
                ],
            )
            .ok()?;
            Some(!to.contains(pending.as_str()))
        }
    }
}

/// Une app installée, pour le résumé de la fin.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Ready {
    pub app: String,
    pub address: String,
    /// le niveau voulu (dotbox.yml ou le manifest)
    pub access: String,
    /// derrière Authelia tant que son assistant de premier démarrage n'est pas fini (D71)
    pub setup_pending: bool,
}

/// Le container tourne (avec ou sans healthcheck).
pub fn running(state: &str) -> bool {
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
/// Les dossiers qui manquent entre `base` (non compris) et `dir`, du plus bas au plus haut.
fn missing_below(base: &Path, dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut p = dir;
    while p != base && p.starts_with(base) && !p.exists() {
        out.push(p.to_path_buf());
        match p.parent() {
            Some(q) => p = q,
            None => break,
        }
    }
    out
}

fn write_settings(m: &catalog::Manifest, ctx: &Ctx, uid: u32, gid: u32) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let Some(st) = &m.settings else {
        return Ok(false);
    };
    let path = format!("{}/{}", engine::dir(&m.name), st.file);
    if let Some(parent) = Path::new(&path).parent() {
        // chaque dossier qui manque sous /opt/<app>/config est à l'utilisateur, pas seulement le
        // dernier: config/.config/code-server, .config compris (le dossier personnel de l'app)
        let base = Path::new(&engine::dir(&m.name)).join("config");
        for d in missing_below(&base, parent).iter().rev() {
            system::owned_dir(&d.to_string_lossy(), uid, gid)?;
        }
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
    // l'app crée ce fichier à son premier démarrage: dotbox attend qu'il existe (install_one)
    if st.created_by_app && !Path::new(&path).exists() {
        say(
            "check",
            &format!("{path}: written by {} at its first start", m.name),
        );
        return Ok(false);
    }
    let merge = |old: Option<&str>| {
        match st.format.as_str() {
            "xml" => engine::merge_xml(old, &st.root, &values),
            "ini" => engine::merge_ini(old, &values),
            "json" => engine::merge_json(old, &values),
            "yaml" => engine::merge_yaml(old, &values),
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
    finish(remove_steps(app, args.purge, args.yes))
}

fn remove_steps(app: &str, purge: bool, yes: bool) -> Result<(), String> {
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
    if purge && !yes {
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
        let _ = std::fs::remove_file(compose_sum_path(app));
    }
    crate::crowdsec::unwatch_app(app)?;
    // le module reste chargé jusqu'au prochain démarrage du serveur: il ne gêne rien
    let modules = format!("/etc/modules-load.d/dotbox-{app}.conf");
    if Path::new(&modules).exists() {
        std::fs::remove_file(&modules).map_err(|e| format!("cannot delete {modules}: {e}"))?;
        say("delete", &modules);
    }
    if cfg.remove_app(app) {
        system::write_if_changed(config::PATH, &cfg.render()?, 0o644)?;
        say("write", &format!("{} (apps: without {app})", config::PATH));
        // sa règle one_factor, s'il en avait une, quitte Authelia (D178)
        if let Some(domain) = cfg.get("domain") {
            authelia::refresh_rules(&cfg, &domain)?;
        }
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
    #[test]
    fn folders_missing_below_the_config() {
        let tmp = std::env::temp_dir().join(format!("dotbox-missing-{}", std::process::id()));
        let base = tmp.join("config");
        std::fs::create_dir_all(&base).unwrap();
        let dir = base.join(".config/code-server");
        assert_eq!(
            missing_below(&base, &dir),
            [dir.clone(), base.join(".config")]
        );
        // hors de config: rien
        assert!(missing_below(&base, &tmp.join("other/x")).is_empty());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title("prowlarr"), "Prowlarr");
        assert_eq!(title("radarr-4k"), "Radarr-4k");
    }
}
