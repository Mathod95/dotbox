//! Les autres commandes de la CLI (Journal, étape 13): list, info, start, stop, restart, logs
//! et update (diff est dans sync.rs). Ce qui lit répond tout de suite (une route GET du service), ce qui agit est
//! une tâche du service, avec ses étapes en direct, comme install. Avec --local, en root, sans
//! le service.

use std::path::{Path, PathBuf};
use std::time::Duration;

use console::style;
use serde::{Deserialize, Serialize};

use crate::catalog;
use crate::clack::{self, Mark};
use crate::config::Config;
use crate::out::say;
use crate::status::{self, CORE};
use crate::{apps, engine, github, system, ui};

fn catalog_dir() -> PathBuf {
    PathBuf::from(github::DIR)
}

fn no_catalog() -> String {
    format!("no catalog in {}: run sudo dotbox init", github::DIR)
}

// ---------- 13.1 dotbox list

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CatalogApp {
    pub name: String,
    pub role: String,
    pub version: String,
    pub installed: bool,
    /// ajoutée avec dotbox add (Q80)
    #[serde(default)]
    pub yours: bool,
}

/// Le catalogue rangé par le dernier init, filtré sur le nom ou le rôle.
pub fn list(search: &str) -> Result<Vec<CatalogApp>, String> {
    let dir = catalog_dir();
    let names = catalog::app_names(&dir);
    if names.is_empty() {
        return Err(no_catalog());
    }
    let cfg = Config::load()?;
    let s = search.to_lowercase();
    let names = with_yours(names, catalog::app_names(Path::new(crate::add::LOCAL)));
    Ok(names
        .into_iter()
        .filter_map(|n| {
            // un manifest pour un Dot Box plus récent est passé, pas une erreur
            let m = crate::add::load(&dir, &n).ok()?;
            let hit = s.is_empty() || n.contains(&s) || m.description.to_lowercase().contains(&s);
            hit.then(|| CatalogApp {
                installed: cfg.app(&n).is_some(),
                version: m.version().to_string(),
                role: m.description.clone(),
                yours: crate::add::is_yours(&n),
                name: n,
            })
        })
        .collect())
}

/// Le catalogue et les apps ajoutées avec dotbox add (Q80), chaque nom une fois, triés.
fn with_yours(mut names: Vec<String>, yours: Vec<String>) -> Vec<String> {
    for y in yours {
        if !names.contains(&y) {
            names.push(y);
        }
    }
    names.sort();
    names
}

pub fn print_list(list: &[CatalogApp]) {
    let width = list.iter().map(|a| a.name.len()).max().unwrap_or(0).max(4) + 3;
    let installed = list.iter().filter(|a| a.installed).count();
    if !clack::fancy() {
        for a in list {
            let state = match (a.installed, a.yours) {
                (true, _) => "installed",
                (false, true) => "yours",
                (false, false) => "-",
            };
            clack::put(&format!(
                "{:<width$}{:<20}{:<11}{}",
                a.name,
                ui::fit(&a.version, 19),
                state,
                a.role
            ));
        }
        return;
    }
    clack::open("dotbox list");
    if list.is_empty() {
        clack::sub("no app matches");
    }
    for a in list {
        let role = if a.yours {
            format!("{} (yours)", a.role)
        } else {
            a.role.clone()
        };
        let text = format!("{:<20}{}", ui::fit(&a.version, 19), ui::fit(&role, 48));
        if a.installed {
            clack::item(Mark::Ok, &a.name, width, &text);
        } else {
            clack::item_off(&a.name, width, &text);
        }
    }
    clack::close(&format!(
        "{} {}, {installed} installed {}",
        list.len(),
        if list.len() == 1 { "app" } else { "apps" },
        style("(◇ installed, ○ not yet: dotbox install <app>)").dim()
    ));
}

// ---------- 13.2 dotbox info

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    pub role: String,
    /// l'image du catalogue
    pub image: String,
    /// la version qui tourne, si elle n'est pas celle du catalogue
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub running: String,
    pub installed: bool,
    pub state: String,
    pub health: String,
    pub url: String,
    pub access: String,
    pub setup_pending: bool,
    pub volumes: Vec<String>,
    pub ports: Vec<String>,
}

pub fn info(app: &str) -> Result<AppInfo, String> {
    let dir = catalog_dir();
    if catalog::app_names(&dir).is_empty() {
        return Err(no_catalog());
    }
    let m = catalog::load(&crate::add::manifest_dir(&dir, app), app)?;
    let cfg = Config::load()?;
    let domain = cfg.get("domain").unwrap_or_default();
    let (sub, access) = match cfg.app(app) {
        Some((s, a)) => (if s.is_empty() { app.to_string() } else { s }, a),
        None => (app.to_string(), m.access.as_str().to_string()),
    };
    let list = status::containers().unwrap_or_default();
    let c = list.iter().find(|c| c.name == app);
    let running = c.map(|c| status::tag(&c.image)).unwrap_or_default();
    Ok(AppInfo {
        name: app.to_string(),
        role: m.description.clone(),
        image: m.image.clone(),
        running: if running == m.version() {
            String::new()
        } else {
            running
        },
        installed: cfg.app(app).is_some(),
        state: c
            .map(|c| c.state.clone())
            .unwrap_or_else(|| "missing".into()),
        health: c.map(|c| c.health.clone()).unwrap_or_default(),
        url: format!("https://{sub}.{domain}"),
        setup_pending: status::setup_pending(app, &access),
        access,
        volumes: m
            .volumes
            .iter()
            .map(|v| {
                format!(
                    "{} → {}{}",
                    v.host,
                    v.container,
                    if v.read_only { " (read-only)" } else { "" }
                )
            })
            .collect(),
        ports: m
            .ports
            .iter()
            .map(|p| format!("{}/{}", p.number, p.proto))
            .collect(),
    })
}

pub fn print_info(i: &AppInfo) {
    let state = if !i.installed {
        "not installed".to_string()
    } else if i.state == "running" && !i.health.is_empty() {
        i.health.clone()
    } else {
        i.state.clone()
    };
    let access = if i.setup_pending {
        format!(
            "{} after its first-run setup (two_factor until then)",
            i.access
        )
    } else {
        i.access.clone()
    };
    if !clack::fancy() {
        clack::put(&format!("name       {}", i.name));
        clack::put(&format!("role       {}", i.role));
        clack::put(&format!("image      {}", i.image));
        if !i.running.is_empty() {
            clack::put(&format!("running    {}", i.running));
        }
        clack::put(&format!("state      {state}"));
        clack::put(&format!("address    {}", i.url));
        clack::put(&format!("access     {access}"));
        for v in &i.volumes {
            clack::put(&format!("folder     {v}"));
        }
        for p in &i.ports {
            clack::put(&format!("port       {p}"));
        }
        return;
    }
    clack::open(&format!("dotbox info {}", i.name));
    clack::field("Image", &i.image);
    if !i.running.is_empty() {
        clack::field(
            "Running",
            &Mark::Warn.paint(&format!("{}, run dotbox update {}", i.running, i.name)),
        );
    }
    let painted = match state.as_str() {
        "healthy" | "running" => Mark::Ok.paint(&state),
        "starting" | "not installed" => Mark::Warn.paint(&state),
        _ => Mark::Error.paint(&state),
    };
    clack::field("State", &painted);
    clack::field("Address", &i.url);
    clack::field("Access", &access);
    for (n, v) in i.volumes.iter().enumerate() {
        clack::field(if n == 0 { "Folders" } else { "" }, v);
    }
    if !i.ports.is_empty() {
        clack::field("Ports", &i.ports.join(", "));
    }
    let end = if i.installed {
        i.role.clone()
    } else {
        format!("{} · dotbox install {}", i.role, i.name)
    };
    clack::close(&style(end).dim().to_string());
}

// ---------- 13.3 dotbox start, stop, restart

fn installed_app(app: &str) -> Result<(), String> {
    if CORE.contains(&app) {
        return Err(format!(
            "{app} is part of the core: sudo dotbox init manages it"
        ));
    }
    if !catalog::valid_name(app) {
        return Err(format!("{app:?} is not an app name"));
    }
    Ok(())
}

/// Le container a-t-il un healthcheck (le sien ou celui de son image)?
pub fn has_health(app: &str) -> bool {
    system::run(
        "docker",
        &["inspect", "-f", "{{if .Config.Healthcheck}}yes{{end}}", app],
    )
    .is_ok_and(|s| s == "yes")
}

pub fn lifecycle(action: &str, app: &str, verbose: bool) -> Result<(), String> {
    installed_app(app)?;
    apps::begin(action, app, 1, verbose)?;
    ui::step(&apps::title(app), action);
    apps::finish((|| {
        let mut cfg = Config::load()?;
        if cfg.app(app).is_none() || !Path::new(&engine::compose_path(app)).exists() {
            return Err(format!("{app} is not installed: dotbox install {app}"));
        }
        match action {
            "start" => apps::compose(app, &["up", "-d", "--quiet-pull"])?,
            "stop" => apps::compose(app, &["stop"])?,
            _ => apps::compose(app, &["restart"])?,
        };
        say(action, app);
        // arrêtée exprès: dotbox.yml le garde, diff et sync la laissent arrêtée (D81)
        if cfg.set_stopped(app, action == "stop") {
            system::write_if_changed(crate::config::PATH, &cfg.render()?, 0o644)?;
            say(
                "write",
                &format!(
                    "{} ({app}: {})",
                    crate::config::PATH,
                    if action == "stop" {
                        "stopped: true"
                    } else {
                        "no longer stopped"
                    }
                ),
            );
        }
        if action == "stop" {
            ui::done("stopped");
        } else {
            let state = apps::wait_ready(app, has_health(app), Duration::from_secs(180))?;
            ui::done(&state);
        }
        Ok(())
    })())
}

// ---------- 13.4 dotbox logs

/// Les logs d'une app, ou d'un container du core: lire n'est pas agir.
pub fn logs_allowed(app: &str) -> Result<(), String> {
    if CORE.contains(&app) {
        return Ok(());
    }
    if !catalog::valid_name(app) {
        return Err(format!("{app:?} is not an app name"));
    }
    let cfg = Config::load()?;
    if cfg.app(app).is_some() || is_service_of(&cfg, app) {
        return Ok(());
    }
    Err(format!("{app} is not installed"))
}

/// Le container d'un service d'une app installée (jellystat-db, D159): ses logs se lisent aussi,
/// par dotbox logs jellystat-db.
fn is_service_of(cfg: &Config, container: &str) -> bool {
    cfg.app_names().iter().any(|a| {
        container.starts_with(&format!("{a}-"))
            && std::fs::read_to_string(engine::compose_path(a)).is_ok_and(|t| {
                crate::sync::compose_images(&t)
                    .iter()
                    .any(|(c, _)| c == container)
            })
    })
}

pub fn logs_args(app: &str, tail: u32, follow: bool) -> Vec<String> {
    let mut a = vec!["logs".to_string(), "--tail".into(), tail.to_string()];
    if follow {
        a.push("-f".into());
    }
    a.push(app.to_string());
    a
}

pub fn logs_local(app: &str, tail: u32, follow: bool) -> Result<(), String> {
    logs_allowed(app)?;
    let status = std::process::Command::new("docker")
        .args(logs_args(app, tail, follow))
        .status()
        .map_err(|e| format!("cannot run docker logs: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(String::new())
    }
}

// ---------- 13.5 et 13.6 dotbox update

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Version {
    pub app: String,
    /// le tag qui tourne, vide sans container
    pub current: String,
    /// le tag du catalogue
    pub latest: String,
    /// ses services en retard sur le catalogue (D159): « jellystat-db 18.1 → 18.2 »
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

impl Version {
    pub fn up_to_date(&self) -> bool {
        self.current == self.latest && self.services.is_empty()
    }

    /// Ce qui est en retard: « → 1.2 », et ses services (« jellystat-db 18.1 → 18.2 »).
    fn behind(&self) -> String {
        let mut parts = Vec::new();
        if self.current != self.latest {
            parts.push(format!("→ {}", self.latest));
        }
        parts.extend(self.services.iter().cloned());
        parts.join(", ")
    }
}

/// Les apps installées, leur version et celle du catalogue rangé.
pub fn versions(dir: &Path, cfg: &Config) -> Result<Vec<Version>, String> {
    let list = status::containers()?;
    Ok(cfg
        .app_names()
        .into_iter()
        .filter_map(|app| {
            // une app à soi se compare à son manifest (Q80)
            let m = crate::add::load(dir, &app).ok()?;
            let current = list
                .iter()
                .find(|c| c.name == app)
                .map(|c| status::tag(&c.image))
                .unwrap_or_default();
            let services = m
                .services
                .iter()
                .filter_map(|s| {
                    let name = s.container(&app);
                    let tag = list
                        .iter()
                        .find(|c| c.name == name)
                        .map(|c| status::tag(&c.image))?;
                    (tag != s.version()).then(|| format!("{name} {tag} → {}", s.version()))
                })
                .collect();
            Some(Version {
                latest: m.version().to_string(),
                current,
                app,
                services,
            })
        })
        .collect())
}

/// Les images d'avant la dernière mise à jour, pour --rollback: une ligne par container
/// (« jellystat-db postgres:18.1 »). Une ligne sans nom de container est l'image de l'app (le
/// fichier d'avant les services, D159).
fn previous_path(app: &str) -> String {
    format!("{}/.dotbox-previous", engine::dir(app))
}

fn parse_previous(app: &str, text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| match l.split_once(' ') {
            Some((c, i)) => (c.to_string(), i.trim().to_string()),
            None => (app.to_string(), l.to_string()),
        })
        .collect()
}

/// Les containers d'une app qui tournent, et leur image, pour --rollback.
fn running_images(app: &str, list: &[status::Container]) -> String {
    let services = std::fs::read_to_string(engine::compose_path(app))
        .map(|t| crate::sync::compose_images(&t))
        .unwrap_or_default();
    let mut names = vec![app.to_string()];
    names.extend(services.into_iter().map(|(c, _)| c).filter(|c| c != app));
    names
        .iter()
        .filter_map(|n| list.iter().find(|c| &c.name == n))
        .map(|c| format!("{} {}\n", c.name, c.image))
        .collect()
}

pub fn update(
    apps_wanted: &[String],
    all: bool,
    rollback: bool,
    verbose: bool,
) -> Result<(), String> {
    for a in apps_wanted {
        installed_app(a)?;
    }
    if rollback {
        let [app] = apps_wanted else {
            return Err("--rollback takes one app: dotbox update <app> --rollback".into());
        };
        return roll_back(app, verbose);
    }
    let what = if all {
        "--all".to_string()
    } else {
        apps_wanted.join(" ")
    };
    apps::begin("update", &what, 1, verbose)?;
    ui::step("Catalog", "the versions on GitHub");
    let mut report = Vec::new();
    let result = (|| {
        let src = github::fetch()?;
        ui::done(&format!("{} of {}", src.short(), src.date));
        let dir = catalog_dir();
        let cfg = Config::load()?;
        let (domain, user) = apps::server(&cfg)?;
        report = versions(&dir, &cfg)?;
        for a in apps_wanted {
            if !report.iter().any(|v| &v.app == a) {
                return Err(format!("{a} is not installed: dotbox install {a}"));
            }
        }
        let targets: Vec<Version> = report
            .iter()
            .filter(|v| !v.up_to_date())
            .filter(|v| all || apps_wanted.contains(&v.app))
            .cloned()
            .collect();
        ui::more_steps(targets.len());
        // une app à la fois: la suivante seulement quand la précédente tourne (spec: Update)
        for t in &targets {
            let kept = running_images(&t.app, &status::containers()?);
            if !kept.is_empty() {
                system::write_if_changed(&previous_path(&t.app), &kept, 0o644)?;
            }
            // arrêtée exprès: la nouvelle version est prête, l'app reste arrêtée (D81)
            apps::install_one(&dir, &t.app, &domain, &user, None, !cfg.app_stopped(&t.app))?;
        }
        report = versions(&dir, &cfg)?;
        Ok(())
    })();
    apps::finish(result)?;
    if ui::events() {
        ui::result(serde_json::to_value(&report).unwrap_or_default());
    } else {
        print_versions(&report);
    }
    Ok(())
}

fn roll_back(app: &str, verbose: bool) -> Result<(), String> {
    apps::begin("update", &format!("{app} --rollback"), 1, verbose)?;
    let result = (|| {
        let text = std::fs::read_to_string(previous_path(app))
            .map_err(|_| format!("no previous image for {app}: dotbox update keeps one"))?;
        let pins = parse_previous(app, &text);
        if pins.is_empty() {
            return Err(format!(
                "no previous image for {app}: dotbox update keeps one"
            ));
        }
        let cfg = Config::load()?;
        let (domain, user) = apps::server(&cfg)?;
        let start = !cfg.app_stopped(app);
        apps::install_one(&catalog_dir(), app, &domain, &user, Some(&pins), start)?;
        // une seule marche en arrière
        let _ = std::fs::remove_file(previous_path(app));
        Ok(())
    })();
    apps::finish(result)
}

pub fn print_versions(report: &[Version]) {
    let behind: Vec<&Version> = report.iter().filter(|v| !v.up_to_date()).collect();
    if !clack::fancy() {
        let w = report.iter().map(|v| v.current.len()).max().unwrap_or(0) + 2;
        for v in report {
            let state = if v.up_to_date() {
                "up to date".to_string()
            } else {
                v.behind()
            };
            clack::put(&format!("{:<16}{:<w$}{state}", v.app, v.current));
        }
        return;
    }
    let width = report.iter().map(|v| v.app.len()).max().unwrap_or(0).max(4) + 3;
    clack::put("");
    for v in report {
        if v.up_to_date() {
            clack::item(
                Mark::Ok,
                &v.app,
                width,
                &format!(
                    "{:<22}{}",
                    ui::fit(&v.current, 21),
                    style("up to date").dim()
                ),
            );
        } else {
            let current = if v.current.is_empty() {
                "not running"
            } else {
                &v.current
            };
            clack::item(
                Mark::Warn,
                &v.app,
                width,
                &format!("{:<22}{}", ui::fit(current, 21), v.behind()),
            );
        }
    }
    clack::put("");
    match behind.len() {
        0 => clack::put(&format!("  {}", Mark::Ok.paint("all up to date"))),
        n => clack::put(&format!(
            "  {}",
            Mark::Warn.paint(&format!(
                "{n} {} to update: dotbox update --all, or dotbox update <app>",
                if n == 1 { "app" } else { "apps" }
            ))
        )),
    }
}

// ---------- 13.7 dotbox diff: dans sync.rs, avec dotbox sync (étape 15)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_with_your_apps() {
        let cat = vec!["radarr".to_string(), "sonarr".to_string()];
        let yours = vec!["notes".to_string(), "radarr".to_string()];
        assert_eq!(with_yours(cat, yours), ["notes", "radarr", "sonarr"]);
    }

    #[test]
    fn versions_behind() {
        let v = Version {
            app: "radarr".into(),
            current: "7.0.3.10918-ls328".into(),
            latest: "7.0.4.10950-ls329".into(),
            services: vec![],
        };
        assert!(!v.up_to_date());
        assert_eq!(v.behind(), "→ 7.0.4.10950-ls329");
        let w = Version {
            latest: v.current.clone(),
            ..v.clone()
        };
        assert!(w.up_to_date());
        // la base en retard, l'app à jour (D159)
        let db = Version {
            services: vec!["radarr-db 18.1 → 18.2".into()],
            ..w.clone()
        };
        assert!(!db.up_to_date());
        assert_eq!(db.behind(), "radarr-db 18.1 → 18.2");
    }

    #[test]
    fn previous_images() {
        // le fichier d'avant les services: l'image de l'app seule
        assert_eq!(
            parse_previous("radarr", "lscr.io/linuxserver/radarr:7.0.3\n"),
            [(
                "radarr".to_string(),
                "lscr.io/linuxserver/radarr:7.0.3".to_string()
            )]
        );
        assert_eq!(
            parse_previous(
                "stats",
                "stats example/stats:1.2\nstats-db postgres:18.1\n\n"
            ),
            [
                ("stats".to_string(), "example/stats:1.2".to_string()),
                ("stats-db".to_string(), "postgres:18.1".to_string())
            ]
        );
    }

    #[test]
    fn logs_arguments() {
        assert_eq!(
            logs_args("radarr", 100, false),
            ["logs", "--tail", "100", "radarr"]
        );
        assert_eq!(
            logs_args("radarr", 5, true),
            ["logs", "--tail", "5", "-f", "radarr"]
        );
    }

    #[test]
    fn the_core_is_not_an_app_here() {
        assert!(installed_app("traefik").is_err());
        assert!(installed_app("../etc").is_err());
        assert!(installed_app("radarr").is_ok());
        assert!(logs_allowed("traefik").is_ok());
    }
}
