//! `dotbox status` (Journal, 12.1): ce qui tourne et où. Un seul `docker ps` et dotbox.yml, sans
//! réseau: la réponse est immédiate. Appelé par la route GET /api/v1/status du service, ou
//! directement avec --local.

use console::style;
use serde::{Deserialize, Serialize};

use crate::clack::{self, Mark};
use crate::config::Config;
use crate::{doctor, engine, system};

/// Les containers du core, dans l'ordre de dotbox init.
pub const CORE: &[&str] = &[
    "traefik",
    "socket-proxy",
    "authelia",
    "authelia-redis",
    "crowdsec",
    "crowdsec-socket-proxy",
];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    /// le tag de l'image qui tourne
    pub version: String,
    /// running, exited, restarting... « missing » si le container n'existe pas
    pub state: String,
    /// healthy, unhealthy, starting, ou vide sans healthcheck
    pub health: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub access: String,
    /// derrière Authelia tant que son assistant de premier démarrage n'est pas fini (D71)
    #[serde(default)]
    pub setup_pending: bool,
    /// les containers de ses services, sa base ou son cache (D159, D163), vide pour une app seule
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Status {
    pub core: Vec<Entry>,
    pub apps: Vec<Entry>,
    /// le résumé du dernier doctor, s'il a déjà tourné
    pub doctor: Option<doctor::Summary>,
    /// la dernière sauvegarde et la prochaine (D97)
    #[serde(default)]
    pub backup: Option<crate::backup::Summary>,
}

/// Une ligne de `docker ps -a --format '{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}'`.
#[derive(Debug, PartialEq)]
pub struct Container {
    pub name: String,
    pub image: String,
    pub state: String,
    pub health: String,
}

pub fn parse_ps(text: &str) -> Vec<Container> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?.trim().to_string();
            let image = f.next()?.trim().to_string();
            let state = f.next()?.trim().to_string();
            let status = f.next().unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            Some(Container {
                name,
                image,
                state,
                health: health_of(status),
            })
        })
        .collect()
}

/// « Up 2 hours (healthy) » donne healthy, « Up 5 seconds (health: starting) » starting.
pub fn health_of(status: &str) -> String {
    if status.contains("(healthy)") {
        "healthy".into()
    } else if status.contains("(unhealthy)") {
        "unhealthy".into()
    } else if status.contains("health: starting") {
        "starting".into()
    } else {
        String::new()
    }
}

/// Le tag d'une image: « lscr.io/linuxserver/radarr:7.0.3.10918-ls328 » donne la version, un
/// port de registre (« host:5000/x ») n'en est pas une.
pub fn tag(image: &str) -> String {
    let image = image.split('@').next().unwrap_or(image);
    match image.rsplit_once(':') {
        Some((_, t)) if !t.contains('/') => t.to_string(),
        _ => "latest".into(),
    }
}

pub fn containers() -> Result<Vec<Container>, String> {
    let out = system::run(
        "docker",
        &[
            "ps",
            "-a",
            "--format",
            "{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}",
        ],
    )
    .map_err(|e| format!("Docker does not answer: {e}"))?;
    Ok(parse_ps(&out))
}

/// Une app ouverte à Internet (app, public) dont la route principale passe encore par Authelia:
/// son assistant de premier démarrage n'est pas fini (D71). Lu dans le compose, sans rien lancer.
/// Seule la route de l'app compte: celle de /admin de Vaultwarden passe toujours par Authelia.
pub fn setup_pending(app: &str, access: &str) -> bool {
    if !matches!(access, "app" | "public") {
        return false;
    }
    let compose = std::fs::read_to_string(engine::compose_path(app)).unwrap_or_default();
    main_route_behind_authelia(&compose, app)
}

pub fn main_route_behind_authelia(compose: &str, app: &str) -> bool {
    let key = format!("traefik.http.routers.{app}.middlewares:");
    compose
        .lines()
        .any(|l| l.trim_start().starts_with(&key) && l.contains("authelia@docker"))
}

fn entry(name: &str, list: &[Container]) -> Entry {
    match list.iter().find(|c| c.name == name) {
        Some(c) => Entry {
            name: name.into(),
            version: tag(&c.image),
            state: c.state.clone(),
            health: c.health.clone(),
            url: String::new(),
            access: String::new(),
            setup_pending: false,
            services: Vec::new(),
        },
        None => Entry {
            name: name.into(),
            version: String::new(),
            state: "missing".into(),
            health: String::new(),
            url: String::new(),
            access: String::new(),
            setup_pending: false,
            services: Vec::new(),
        },
    }
}

/// Les services d'une app, lus dans son compose.yml: chaque container sauf l'app elle-même.
pub fn services_of(app: &str, list: &[Container]) -> Vec<Entry> {
    let text = std::fs::read_to_string(engine::compose_path(app)).unwrap_or_default();
    crate::sync::compose_images(&text)
        .into_iter()
        .filter(|(c, _)| c != app)
        .map(|(c, _)| entry(&c, list))
        .collect()
}

/// Le nom court d'un service sous son app: « db » pour jellystat-db.
pub fn short_name<'a>(app: &str, service: &'a str) -> &'a str {
    service
        .strip_prefix(app)
        .and_then(|s| s.strip_prefix('-'))
        .unwrap_or(service)
}

pub fn collect() -> Result<Status, String> {
    let list = containers()?;
    let cfg = Config::load()?;
    let domain = cfg.get("domain").unwrap_or_default();
    let core = CORE.iter().map(|n| entry(n, &list)).collect();
    let apps = cfg
        .app_names()
        .iter()
        .map(|name| {
            let (sub, access) = cfg.app(name).unwrap_or_default();
            let sub = if sub.is_empty() { name.clone() } else { sub };
            let mut e = entry(name, &list);
            e.url = format!("https://{sub}.{domain}");
            e.setup_pending = setup_pending(name, &access);
            e.access = access;
            e.services = services_of(name, &list);
            e
        })
        .collect();
    Ok(Status {
        core,
        apps,
        doctor: doctor::last().map(|r| r.summary()),
        backup: crate::backup::summary(),
    })
}

/// L'état lisible: « healthy », « running », « exited »...
fn shown_state(e: &Entry) -> String {
    if e.state == "running" && !e.health.is_empty() {
        e.health.clone()
    } else {
        e.state.clone()
    }
}

fn styled_state(e: &Entry) -> String {
    let s = shown_state(e);
    match s.as_str() {
        "healthy" | "running" => style(s).green().to_string(),
        "starting" => style(s).yellow().to_string(),
        _ => style(s).red().to_string(),
    }
}

/// Le cadre façon cliclack dans un terminal (#92), le tableau en texte simple sinon.
pub fn print(st: &Status) {
    if clack::fancy() {
        print_fancy(st)
    } else {
        print_plain(st)
    }
}

/// Le symbole d'un container: ■ arrêté ou malade, ▲ qui démarre ou attend son assistant.
pub fn mark(e: &Entry) -> Mark {
    match (e.state.as_str(), e.health.as_str()) {
        ("running", "unhealthy") => Mark::Error,
        ("running", "starting") => Mark::Warn,
        ("running", _) if e.setup_pending => Mark::Warn,
        ("running", _) => Mark::Ok,
        _ => Mark::Error,
    }
}

/// Le résumé du dernier doctor, pour la dernière ligne.
fn doctor_line(d: &Option<doctor::Summary>) -> String {
    match d {
        None => style("doctor never run: dotbox doctor").dim().to_string(),
        Some(d) if d.problems == 0 => {
            Mark::Ok.paint(&format!("doctor {}: all good", d.date_short()))
        }
        Some(d) => {
            // un doctor.json d'avant les sections: le nombre de problèmes
            let (n, what) = if d.sections > 0 {
                (
                    d.sections,
                    if d.sections == 1 {
                        "section"
                    } else {
                        "sections"
                    },
                )
            } else {
                (
                    d.problems,
                    if d.problems == 1 {
                        "problem"
                    } else {
                        "problems"
                    },
                )
            };
            Mark::Warn.paint(&format!(
                "doctor {}: {n} {what} to look at, run dotbox doctor",
                d.date_short()
            ))
        }
    }
}

/// La ligne de la sauvegarde: la dernière, son résultat, la prochaine.
fn backup_line(b: &Option<crate::backup::Summary>) -> (Mark, String) {
    use crate::backup::{human, short_date};
    let next = |b: &crate::backup::Summary| {
        if b.next.is_empty() {
            style(", no schedule").dim().to_string()
        } else {
            style(format!(", next {}", b.next)).dim().to_string()
        }
    };
    match b {
        None => (
            Mark::Warn,
            format!("none yet {}", style("dotbox backup").dim()),
        ),
        Some(b) if !b.ok => (
            Mark::Error,
            format!("{} failed: {}{}", short_date(&b.started), b.error, next(b)),
        ),
        Some(b) => (
            Mark::Ok,
            format!(
                "{}, {}, {}{}",
                short_date(&b.started),
                crate::backup::count(b.archives),
                human(b.bytes),
                next(b)
            ),
        ),
    }
}

fn print_fancy(st: &Status) {
    clack::open("dotbox status");
    let width = st
        .apps
        .iter()
        .flat_map(|a| {
            std::iter::once(a.name.len()).chain(
                a.services
                    .iter()
                    .map(|s| short_name(&a.name, &s.name).len() + 2),
            )
        })
        .max()
        .unwrap_or(0)
        .max(6)
        + 3;
    // le core en une ligne, ses versions dessous
    let bad: Vec<&Entry> = st.core.iter().filter(|e| mark(e) != Mark::Ok).collect();
    let core_mark = bad.iter().map(|e| mark(e)).max().unwrap_or(Mark::Ok);
    let text = if bad.is_empty() {
        format!("{} containers, all healthy", st.core.len())
    } else {
        format!("{} of {} to look at", bad.len(), st.core.len())
    };
    clack::item(core_mark, "Core", width, &text);
    for e in &bad {
        clack::sub_mark(mark(e), &format!("{}: {}", e.name, shown_state(e)));
    }
    let versions: Vec<String> = st
        .core
        .iter()
        .filter(|e| !e.name.ends_with("socket-proxy") && !e.version.is_empty())
        .map(|e| format!("{} {}", e.name, e.version.trim_end_matches("-debian")))
        .collect();
    clack::sub(&versions.join(" · "));
    clack::bar();
    if st.apps.is_empty() {
        clack::sub("no app yet: dotbox install <app>");
    }
    let access_of = |e: &Entry| {
        if mark(e) == Mark::Error {
            shown_state(e)
        } else if e.setup_pending {
            format!("{} after setup", e.access)
        } else {
            e.access.clone()
        }
    };
    let access_w = st
        .apps
        .iter()
        .map(|e| access_of(e).len())
        .max()
        .unwrap_or(0)
        + 2;
    for e in &st.apps {
        let m = mark(e);
        // une app arrêtée montre son état à la place de son accès
        let access = if m == Mark::Error {
            shown_state(e)
        } else if e.setup_pending {
            format!("{} after setup", e.access)
        } else {
            e.access.clone()
        };
        let line = format!(
            "{:<19}{:<access_w$}{}",
            crate::ui::fit(&e.version, 18),
            access,
            style(e.url.trim_start_matches("https://")).dim()
        );
        clack::item(m, &e.name, width, &line);
        // ses services dessous, chacun avec son symbole: la base, le cache (D163)
        for sv in &e.services {
            let sm = mark(sv);
            clack::sub_item(
                sm,
                short_name(&e.name, &sv.name),
                width - 2,
                &format!(
                    "{:<19}{}",
                    crate::ui::fit(&sv.version, 18),
                    sm.paint(&shown_state(sv))
                ),
            );
        }
    }
    clack::bar();
    let (m, text) = backup_line(&st.backup);
    clack::item(m, "Backup", width, &text);
    clack::close(&doctor_line(&st.doctor));
}

/// Sans terminal: le tableau en texte simple, pour un script ou un log.
fn print_plain(st: &Status) {
    crate::clack::put(&format!(
        "  {}",
        style(format!("{:<22}{:<19}{:<10}", "CORE", "VERSION", "STATE")).dim()
    ));
    for e in &st.core {
        crate::clack::put(&format!(
            "  {:<22}{:<19}{}",
            e.name,
            crate::ui::fit(&e.version, 18),
            styled_state(e)
        ));
    }
    crate::clack::put("");
    if st.apps.is_empty() {
        crate::clack::put(&format!(
            "  {}",
            style("no app yet: dotbox install <app>").dim()
        ));
    } else {
        crate::clack::put(&format!(
            "  {}",
            style(format!(
                "{:<22}{:<19}{:<10}{:<19}{}",
                "APP", "VERSION", "STATE", "ACCESS", "ADDRESS"
            ))
            .dim()
        ));
        for e in &st.apps {
            // en attendant son assistant, l'app est en two_factor: le niveau voulu vient après
            let access = if e.setup_pending {
                format!("{} after setup", e.access)
            } else {
                e.access.clone()
            };
            // l'état complété à 10 colonnes avant les codes de couleur
            let state = shown_state(e);
            let pad = " ".repeat(10usize.saturating_sub(state.chars().count()));
            crate::clack::put(&format!(
                "  {:<22}{:<19}{}{pad}{:<19}{}",
                e.name,
                crate::ui::fit(&e.version, 18),
                styled_state(e),
                access,
                style(e.url.trim_start_matches("https://")).dim()
            ));
            for sv in &e.services {
                let state = shown_state(sv);
                let pad = " ".repeat(10usize.saturating_sub(state.chars().count()));
                crate::clack::put(&format!(
                    "    {:<20}{:<19}{}{pad}",
                    sv.name,
                    crate::ui::fit(&sv.version, 18),
                    styled_state(sv)
                ));
            }
        }
    }
    crate::clack::put("");
    // la sauvegarde, comme dans la sortie en cadre
    let (m, text) = backup_line(&st.backup);
    crate::clack::put(&format!("  {} backup {text}", m.symbol()));
    match &st.doctor {
        Some(d) if d.problems == 0 => crate::clack::put(&format!(
            "  {} doctor {}: all good",
            style("✓").green(),
            d.date_short()
        )),
        Some(d) => crate::clack::put(&format!(
            "  {} doctor {}: {} {}, run dotbox doctor",
            style("!").yellow(),
            d.date_short(),
            d.problems,
            if d.problems == 1 {
                "problem"
            } else {
                "problems"
            }
        )),
        None => crate::clack::put(&format!(
            "  {} doctor never run: dotbox doctor",
            style("·").dim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_docker_ps() {
        let text = "radarr\tlscr.io/linuxserver/radarr:7.0.3.10918-ls328\trunning\tUp 2 hours (healthy)\n\
                    wizarr\tghcr.io/wizarrrr/wizarr:v2026.10.1\trunning\tUp 5 seconds (health: starting)\n\
                    qbittorrent\tlscr.io/linuxserver/qbittorrent:5.3.0\texited\tExited (0) 3 minutes ago\n\
                    socket-proxy\tlscr.io/linuxserver/socket-proxy:3.4.7-r0-ls106\trunning\tUp 3 days\n";
        let list = parse_ps(text);
        assert_eq!(list.len(), 4);
        assert_eq!(list[0].health, "healthy");
        assert_eq!(list[1].health, "starting");
        assert_eq!(list[2].state, "exited");
        assert_eq!(list[3].health, "");
        assert_eq!(health_of("Up 1 minute (unhealthy)"), "unhealthy");
    }

    #[test]
    fn image_tags() {
        assert_eq!(
            tag("lscr.io/linuxserver/radarr:7.0.3.10918-ls328"),
            "7.0.3.10918-ls328"
        );
        assert_eq!(tag("traefik:v3.7.19"), "v3.7.19");
        assert_eq!(tag("registry:5000/app"), "latest");
        assert_eq!(tag("jellyfin/jellyfin:12.4@sha256:abc"), "12.4");
    }

    #[test]
    fn setup_read_from_the_main_route() {
        let vw = "      traefik.http.routers.vaultwarden.middlewares: crowdsec@docker\n      traefik.http.routers.vaultwarden-admin.middlewares: authelia@docker\n";
        assert!(!main_route_behind_authelia(vw, "vaultwarden"));
        let jf = "      traefik.http.routers.jellyfin.middlewares: authelia@docker\n";
        assert!(main_route_behind_authelia(jf, "jellyfin"));
    }

    #[test]
    fn marks() {
        let mut e = entry("radarr", &[]);
        assert_eq!(mark(&e), Mark::Error);
        e.state = "running".into();
        assert_eq!(mark(&e), Mark::Ok);
        e.setup_pending = true;
        assert_eq!(mark(&e), Mark::Warn);
        e.health = "unhealthy".into();
        assert_eq!(mark(&e), Mark::Error);
    }

    #[test]
    fn services_under_their_app() {
        assert_eq!(short_name("jellystat", "jellystat-db"), "db");
        assert_eq!(short_name("tracearr", "tracearr-cache"), "cache");
        assert_eq!(short_name("stats", "other-db"), "other-db");
        // la réponse d'un service d'avant D163, sans services: lue quand même
        let e: Entry = serde_json::from_str(
            r#"{"name":"radarr","version":"6.0","state":"running","health":"healthy"}"#,
        )
        .unwrap();
        assert!(e.services.is_empty());
        assert!(!serde_json::to_string(&e).unwrap().contains("services"));
    }

    #[test]
    fn a_missing_container() {
        let e = entry("authelia", &[]);
        assert_eq!(e.state, "missing");
        assert_eq!(shown_state(&e), "missing");
    }
}
