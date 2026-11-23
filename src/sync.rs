//! `dotbox diff` et `dotbox sync` (Journal, étape 15, D81, D93): le plan, façon terraform plan,
//! puis son application, façon terraform apply. Le plan compare ce qui est voulu (dotbox.yml, les
//! manifests rangés, la façon dont init et install écrivent les fichiers) à ce qui tourne, sans
//! rien changer ni aller sur GitHub. sync le remontre, demande une confirmation (-y pour la
//! sauter), puis refait ce que font init et install, seulement là où le plan le dit. Il ne
//! change jamais de version (celle qui tourne est gardée: c'est le rôle de dotbox update), et il
//! laisse arrêtée une app arrêtée avec dotbox stop.

use std::path::{Path, PathBuf};
use std::time::Duration;

use console::style;
use serde::{Deserialize, Serialize};

use crate::clack::{self, Mark};
use crate::config::{self, Config};
use crate::out::say;
use crate::status::Container;
use crate::{
    apps, authelia, backup, catalog, crowdsec, engine, github, motd, prompt, status, system,
    traefik, ui,
};

/// Une ligne du plan.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Gap {
    /// l'app, ou le morceau du core (traefik, authelia, crowdsec)
    pub app: String,
    pub what: String,
    /// la commande qui le corrige
    pub hint: String,
    /// ce que dotbox sync en fera, vide s'il le laisse (une version: dotbox update)
    #[serde(default)]
    pub sync: String,
    /// pas un écart: une app arrêtée exprès (D81), montrée pour mémoire
    #[serde(default)]
    pub info: bool,
}

impl Gap {
    /// sync va agir.
    pub fn applies(&self) -> bool {
        !self.sync.is_empty() && !self.info
    }
}

/// Le nombre d'écarts (pour le code de sortie de diff): une app arrêtée exprès n'en est pas un.
pub fn differences(plan: &[Gap]) -> usize {
    plan.iter().filter(|g| !g.info).count()
}

/// Ce que sync va toucher, dans l'ordre: le core d'abord, puis les apps.
pub fn targets(plan: &[Gap]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in plan.iter().filter(|g| g.applies()) {
        if !out.contains(&g.app) {
            out.push(g.app.clone());
        }
    }
    out
}

/// L'image telle que Docker la montre pour un container, si c'est une référence avec sa version
/// (et pas l'identifiant d'une image dont le tag est parti).
fn image_ref(image: &str) -> Option<String> {
    (image.contains(':') && !image.starts_with("sha256:")).then(|| image.to_string())
}

fn running_image(list: &[Container], name: &str) -> Option<String> {
    list.iter()
        .find(|c| c.name == name)
        .and_then(|c| image_ref(&c.image))
}

/// La première image d'un compose.yml écrit par dotbox.
pub fn compose_image(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim_start().strip_prefix("image: "))
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
}

/// La version gardée par sync pour une app: celle qui tourne, sinon celle de son compose.yml,
/// sinon (jamais installée) celle du catalogue.
fn kept_image(list: &[Container], app: &str) -> Option<String> {
    running_image(list, app).or_else(|| {
        std::fs::read_to_string(engine::compose_path(app))
            .ok()
            .and_then(|t| compose_image(&t))
    })
}

/// Un morceau du core: ses fichiers tels qu'init les écrit depuis dotbox.yml, et ses containers.
pub struct CorePart {
    pub name: &'static str,
    pub compose: &'static str,
    pub files: Vec<(&'static str, String)>,
    pub containers: Vec<&'static str>,
}

/// Les fichiers du core voulus, avec les images qui tournent (sync ne change pas de version:
/// sudo dotbox init met le core aux versions du catalogue).
pub fn core_parts(
    dir: &Path,
    domain: &str,
    ca: &str,
    list: &[Container],
) -> Result<Vec<CorePart>, String> {
    let image = |container: &str, core: &str, key: &str| -> Result<String, String> {
        match running_image(list, container) {
            Some(i) => Ok(i),
            None => Ok(catalog::load_core(dir, core)?.image(key)?.to_string()),
        }
    };
    let proxy = image("socket-proxy", "traefik", "socket-proxy")?;
    Ok(vec![
        CorePart {
            name: "traefik",
            compose: traefik::COMPOSE,
            files: vec![
                (
                    traefik::COMPOSE,
                    traefik::compose(domain, &image("traefik", "traefik", "traefik")?, &proxy),
                ),
                (traefik::STATIC, traefik::static_config(ca)),
            ],
            containers: vec!["traefik", "socket-proxy"],
        },
        CorePart {
            name: "authelia",
            compose: authelia::COMPOSE,
            files: vec![
                (
                    authelia::COMPOSE,
                    authelia::compose(domain, &image("authelia", "authelia", "authelia")?),
                ),
                (authelia::CONFIG, authelia::configuration(domain)),
            ],
            containers: vec!["authelia"],
        },
        CorePart {
            name: "crowdsec",
            compose: crowdsec::COMPOSE,
            files: vec![(
                crowdsec::COMPOSE,
                crowdsec::compose(
                    &image("crowdsec", "crowdsec", "crowdsec")?,
                    &image(crowdsec::PROXY, "traefik", "socket-proxy")?,
                ),
            )],
            containers: vec!["crowdsec", crowdsec::PROXY],
        },
    ])
}

fn state_of(list: &[Container], name: &str) -> String {
    list.iter()
        .find(|c| c.name == name)
        .map(|c| c.state.clone())
        .unwrap_or_else(|| "missing".into())
}

fn core_gaps(parts: &[CorePart], list: &[Container]) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for p in parts {
        let gap = |what: String, sync: &str| Gap {
            app: p.name.to_string(),
            what,
            hint: "dotbox sync".into(),
            sync: sync.into(),
            info: false,
        };
        for (path, text) in &p.files {
            match std::fs::read_to_string(path) {
                Err(_) => gaps.push(gap(format!("{path} missing"), "written again")),
                Ok(now) if &now != text => gaps.push(gap(
                    format!("{path} out of date"),
                    if *path == p.compose {
                        "written again"
                    } else {
                        "written again, restarted"
                    },
                )),
                _ => {}
            }
        }
        for c in &p.containers {
            let state = state_of(list, c);
            if state != "running" {
                gaps.push(gap(format!("{c} {state}, wanted running"), "started"));
            }
        }
    }
    gaps
}

/// Le plan de dotbox diff et dotbox sync.
pub fn plan() -> Result<Vec<Gap>, String> {
    let dir = PathBuf::from(github::DIR);
    if catalog::app_names(&dir).is_empty() {
        return Err(format!(
            "no catalog in {}: run sudo dotbox init",
            github::DIR
        ));
    }
    let cfg = Config::load()?;
    let (domain, user) = apps::server(&cfg)?;
    let ca = cfg
        .get("ca")
        .unwrap_or_else(|| config::DEFAULT_CA.to_string());
    let list = status::containers()?;
    let mut gaps = core_gaps(&core_parts(&dir, &domain, &ca, &list)?, &list);
    for app in cfg.app_names() {
        gaps.extend(app_gaps(&dir, &cfg, &app, &domain, &user, &list));
    }
    gaps.extend(web_gaps(&domain));
    gaps.extend(backup_gaps(&cfg));
    gaps.extend(motd_gaps(&cfg));
    Ok(gaps)
}

/// La porte web (11.13): le fichier de Traefik suit le domaine, la passerelle et le secret.
fn web_gaps(domain: &str) -> Vec<Gap> {
    let gap = |what: String, sync: &str| Gap {
        app: "dotbox".into(),
        what,
        hint: "dotbox sync".into(),
        sync: sync.into(),
        info: false,
    };
    match (
        traefik::web_route_wanted(domain),
        std::fs::read_to_string(traefik::WEB_ROUTE),
    ) {
        (Err(e), _) => vec![gap(format!("the web gate: {e}"), "")],
        (Ok(_), Err(_)) => vec![gap(
            format!("{} missing", traefik::WEB_ROUTE),
            "written again",
        )],
        (Ok(want), Ok(now)) if want != now => vec![gap(
            format!("{} out of date", traefik::WEB_ROUTE),
            "written again",
        )],
        _ => Vec::new(),
    }
}

/// La planification de la sauvegarde (16.6, D97): le timer suit backup.schedule. Sous le nom
/// dotbox, réservé: aucune app ne peut le prendre.
fn backup_gaps(cfg: &Config) -> Vec<Gap> {
    let gap = |what: String, hint: &str, sync: &str| Gap {
        app: "dotbox".into(),
        what,
        hint: hint.into(),
        sync: sync.into(),
        info: false,
    };
    let units = match backup::Settings::of(cfg).and_then(|s| backup::units(&s)) {
        Ok(u) => u,
        Err(e) => return vec![gap(e, &format!("fix {}", config::PATH), "")],
    };
    let mut gaps = Vec::new();
    for (path, wanted) in &units {
        match (wanted, std::fs::read_to_string(path)) {
            (Some(_), Err(_)) => gaps.push(gap(
                format!("{path} missing"),
                "dotbox sync",
                "written again",
            )),
            (Some(text), Ok(now)) if &now != text => gaps.push(gap(
                format!("{path} out of date (backup.schedule)"),
                "dotbox sync",
                "written again",
            )),
            (None, Ok(_)) => gaps.push(gap(
                format!("{path} there, backup.schedule is off"),
                "dotbox sync",
                "removed",
            )),
            _ => {}
        }
    }
    let timer_wanted = units
        .iter()
        .any(|(p, c)| *p == backup::TIMER && c.is_some());
    if timer_wanted
        && gaps.is_empty()
        && system::run("systemctl", &["is-active", "dotbox-backup.timer"]).as_deref()
            != Ok("active")
    {
        gaps.push(gap(
            "dotbox-backup.timer inactive, wanted active".into(),
            "dotbox sync",
            "started",
        ));
    }
    gaps
}

/// Le message d'accueil SSH et le doctor de nuit (étape 17): ils suivent motd.enabled.
fn motd_gaps(cfg: &Config) -> Vec<Gap> {
    let gap = |what: String, sync: &str| Gap {
        app: "dotbox".into(),
        what,
        hint: "dotbox sync".into(),
        sync: sync.into(),
        info: false,
    };
    match motd::Settings::of(cfg) {
        Err(e) => vec![Gap {
            hint: format!("fix {}", config::PATH),
            ..gap(e, "")
        }],
        Ok(s) => motd::gaps(&s)
            .into_iter()
            .map(|(what, sync)| gap(what, sync))
            .collect(),
    }
}

fn app_gaps(
    dir: &Path,
    cfg: &Config,
    app: &str,
    domain: &str,
    user: &str,
    list: &[Container],
) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let gap = |what: String, hint: String, sync: &str| Gap {
        app: app.to_string(),
        what,
        hint,
        sync: sync.into(),
        info: false,
    };
    let c = list.iter().find(|c| c.name == app);
    let stopped = cfg.app_stopped(app);
    let runs = c.is_some_and(|c| c.state == "running");

    // le compose.yml, avec la version qui tourne
    let path = engine::compose_path(app);
    match apps::wanted(dir, app, domain, user, kept_image(list, app).as_deref()) {
        Err(e) => gaps.push(gap(
            format!("cannot be computed: {e}"),
            format!("dotbox install {app}"),
            "",
        )),
        Ok(w) => match std::fs::read_to_string(&path) {
            Err(_) => gaps.push(gap(
                "no compose.yml".into(),
                "dotbox sync".into(),
                "written again",
            )),
            Ok(now) if now != w.compose => {
                let kept = std::fs::read_to_string(apps::compose_sum_path(app)).unwrap_or_default();
                let sum = crowdsec::sha256(&path).unwrap_or_default();
                let (what, hint) = if kept.trim().is_empty() {
                    // installée avant 13.7: pas d'empreinte, la cause est inconnue
                    (
                        "compose.yml differs from what dotbox writes",
                        "dotbox sync writes it again: keep your changes in compose.override.yml",
                    )
                } else if kept.trim() != sum {
                    (
                        "compose.yml changed by hand",
                        "dotbox sync writes it again: keep your changes in compose.override.yml",
                    )
                } else {
                    (
                        "compose.yml out of date (dotbox.yml, or a new Dot Box)",
                        "dotbox sync",
                    )
                };
                // docker compose up -d ne recrée le container que si sa configuration change
                let sync = "written again";
                gaps.push(gap(what.into(), hint.into(), sync));
            }
            _ => {}
        },
    }

    // l'état voulu (D81)
    match (c, stopped) {
        (None, false) => gaps.push(gap(
            "no container, wanted running".into(),
            "dotbox sync".into(),
            "created and started",
        )),
        (Some(c), false) if c.state != "running" => gaps.push(gap(
            format!("{}, wanted running", c.state),
            "dotbox sync".into(),
            "started",
        )),
        (Some(_), true) if runs => gaps.push(gap(
            "running, stopped with dotbox stop".into(),
            "dotbox sync".into(),
            "stopped",
        )),
        (_, true) => gaps.push(Gap {
            info: true,
            ..gap(
                "stopped with dotbox stop".into(),
                format!("dotbox start {app}"),
                "left as it is",
            )
        }),
        _ => {}
    }

    // la version: sync n'y touche pas
    if let (Some(c), Ok(m)) = (c, crate::add::load(dir, app)) {
        let tag = status::tag(&c.image);
        if tag != m.version() {
            let from = if crate::add::is_yours(app) {
                "its manifest"
            } else {
                "the catalog"
            };
            gaps.push(gap(
                format!("runs {tag}, {from} has {}", m.version()),
                format!("dotbox update {app}"),
                "",
            ));
        }
    }
    gaps
}

fn width(plan: &[Gap]) -> usize {
    plan.iter().map(|g| g.app.len()).max().unwrap_or(0).max(4) + 3
}

/// dotbox diff: les écarts et la commande qui corrige chacun.
pub fn print_diff(plan: &[Gap]) {
    let n = differences(plan);
    if !clack::fancy() {
        for g in plan {
            clack::put(&format!("~ {:<16}{}  → {}", g.app, g.what, g.hint));
        }
        clack::put(&format!(
            "{n} {}",
            if n == 1 { "difference" } else { "differences" }
        ));
        return;
    }
    clack::open("dotbox diff");
    if plan.is_empty() {
        clack::close_tight(&Mark::Ok.paint("in sync: what runs is what is wanted"));
        return;
    }
    let w = width(plan);
    for g in plan {
        if g.info {
            clack::item_off(&g.app, w, &g.what);
        } else {
            clack::item(Mark::Warn, &g.app, w, &g.what);
        }
        clack::sub(&format!("→ {}", g.hint));
    }
    match n {
        0 => clack::close(&Mark::Ok.paint("in sync: what runs is what is wanted")),
        n => clack::close(&Mark::Warn.paint(&format!(
            "{n} {}",
            if n == 1 { "difference" } else { "differences" }
        ))),
    }
}

/// dotbox sync: ce qu'il va faire, et ce qu'il laisse.
pub fn print_plan(plan: &[Gap]) {
    let todo = plan.iter().filter(|g| g.applies()).count();
    let left = |g: &Gap| {
        if g.sync.is_empty() {
            format!("left as it is: {}", g.hint)
        } else {
            g.sync.clone()
        }
    };
    if !clack::fancy() {
        for g in plan {
            let sign = if g.applies() { "~" } else { "=" };
            clack::put(&format!("{sign} {:<16}{}: {}", g.app, g.what, left(g)));
        }
        clack::put(&match todo {
            0 => "nothing to do".to_string(),
            n => format!("{n} {}", if n == 1 { "change" } else { "changes" }),
        });
        return;
    }
    clack::open("dotbox sync");
    if plan.is_empty() {
        clack::close_tight(&Mark::Ok.paint("nothing to do: what runs is what is wanted"));
        return;
    }
    let w = width(plan);
    for g in plan {
        if g.applies() {
            clack::item(Mark::Ok, &g.app, w, &g.what);
            clack::sub(&format!("→ {}", g.sync));
        } else {
            clack::item_off(&g.app, w, &g.what);
            clack::sub(&format!("→ {}", left(g)));
        }
    }
    match todo {
        0 => clack::close(&Mark::Ok.paint("nothing to do: what runs is what is wanted")),
        n => clack::close(&format!(
            "{n} {} to apply {}",
            if n == 1 { "change" } else { "changes" },
            style("(◇ applied, ○ left as it is)").dim()
        )),
    }
}

/// La confirmation, avant d'appliquer quoi que ce soit (D93).
pub fn confirm(yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !prompt::is_terminal() {
        return Err("dotbox sync asks for a confirmation: run it in a terminal, or add -y".into());
    }
    if prompt::confirm("Apply this plan?", "←/→ to choose, Enter to confirm")? {
        Ok(())
    } else {
        eprintln!("  cancelled, nothing changed");
        Err(String::new())
    }
}

/// dotbox sync --local, ou la tâche du service (qui passe -y: la CLI a déjà demandé).
pub fn run(yes: bool, verbose: bool) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err("run dotbox sync --local as root: sudo dotbox sync --local".into());
    }
    let plan = plan()?;
    if !ui::events() {
        print_plan(&plan);
    }
    let targets = targets(&plan);
    if targets.is_empty() {
        return Ok(());
    }
    confirm(yes)?;
    apps::begin("sync", "", targets.len(), verbose)?;
    apps::finish(apply(&targets))
}

fn apply(targets: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(github::DIR);
    let cfg = Config::load()?;
    let (domain, user) = apps::server(&cfg)?;
    let ca = cfg
        .get("ca")
        .unwrap_or_else(|| config::DEFAULT_CA.to_string());
    let list = status::containers()?;
    // la porte web avant le core: Traefik doit trouver son dossier dynamic/ en redémarrant (J280)
    if targets.iter().any(|t| t == "dotbox") {
        traefik::write_web_route(&domain)?;
    }
    // le core d'abord: les apps passent par Traefik et Authelia
    for part in core_parts(&dir, &domain, &ca, &list)? {
        if targets.iter().any(|t| t == part.name) {
            apply_core(&part)?;
        }
    }
    for app in cfg.app_names() {
        if targets.contains(&app) {
            let image = kept_image(&list, &app);
            let start = !cfg.app_stopped(&app);
            apps::install_one(&dir, &app, &domain, &user, image.as_deref(), start)?;
        }
    }
    if targets.iter().any(|t| t == "dotbox") {
        ui::step("Dot Box", "the web gate, the timers and the MOTD");
        let done = backup::setup_timer(&backup::Settings::of(&cfg)?)?;
        let welcome = motd::setup(&motd::Settings::of(&cfg)?)?;
        ui::done(&format!("web gate, {done}, {welcome}"));
    }
    Ok(())
}

fn apply_core(p: &CorePart) -> Result<(), String> {
    ui::step(&apps::title(p.name), "from dotbox.yml");
    let mut compose_changed = false;
    let mut config_changed = false;
    for (path, text) in &p.files {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if system::write_if_changed(path, text, 0o644)? {
            say("write", path);
            if *path == p.compose {
                compose_changed = true;
            } else {
                config_changed = true;
            }
        }
    }
    let all_running = p
        .containers
        .iter()
        .all(|c| apps::running(&traefik::container_state(c)));
    let done = if compose_changed || !all_running {
        system::run(
            "docker",
            &["compose", "-f", p.compose, "up", "-d", "--quiet-pull"],
        )?;
        say("start", p.name);
        "written again, started"
    } else if config_changed {
        // la configuration n'est lue qu'au démarrage
        system::run("docker", &["restart", p.name])?;
        say("restart", &format!("{} (new configuration)", p.name));
        "written again, restarted"
    } else {
        "nothing to change"
    };
    let state = apps::wait_ready(p.name, true, Duration::from_secs(240))?;
    say("check", &format!("{} {state}", p.name));
    ui::done(&format!("{done}, {state}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(app: &str, sync: &str, info: bool) -> Gap {
        Gap {
            app: app.into(),
            what: "x".into(),
            hint: "y".into(),
            sync: sync.into(),
            info,
        }
    }

    #[test]
    fn what_sync_touches() {
        let plan = vec![
            g("traefik", "written again, restarted", false),
            g("radarr", "written again", false),
            g("radarr", "started", false),
            g("prowlarr", "", false),
            g("qbittorrent", "left as it is", true),
        ];
        assert_eq!(targets(&plan), ["traefik", "radarr"]);
        // une version en retard compte pour diff, une app arrêtée exprès non
        assert_eq!(differences(&plan), 4);
        assert!(targets(&[g("qbittorrent", "left as it is", true)]).is_empty());
    }

    #[test]
    fn images_kept() {
        assert_eq!(
            compose_image(
                "# x\nname: radarr\nservices:\n  radarr:\n    image: lscr.io/linuxserver/radarr:7.0.3\n"
            ),
            Some("lscr.io/linuxserver/radarr:7.0.3".into())
        );
        assert_eq!(compose_image("services: {}\n"), None);
        assert_eq!(
            image_ref("traefik:v3.7.19").as_deref(),
            Some("traefik:v3.7.19")
        );
        assert_eq!(image_ref("4f2c9a1b7e3d"), None);
        assert_eq!(image_ref("sha256:4f2c9a1b7e3d"), None);
    }

    #[test]
    fn an_old_plan_still_reads() {
        // la forme de dotbox diff --json avant l'étape 15
        let old =
            r#"[{"app":"sonarr","what":"exited, wanted running","hint":"dotbox start sonarr"}]"#;
        let p: Vec<Gap> = serde_json::from_str(old).unwrap();
        assert!(!p[0].applies());
        assert_eq!(differences(&p), 1);
    }
}
