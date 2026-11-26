//! `dotbox doctor` (Journal, 12.2 et 12.3): cherche ce qui cloche sur le serveur et dit quoi faire.
//! En lecture seule: il ne change rien, sauf /opt/dotbox/doctor.json, son dernier résultat, que
//! `dotbox status` résume. Appelé par la route POST /api/v1/doctor, ou directement avec --local.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::clack::{self, Mark};
use crate::config::Config;
use crate::status::{self, Entry};
use crate::{card, crowdsec, system, traefik};

pub const PATH: &str = "/opt/dotbox/doctor.json";
pub const ACME: &str = "/opt/traefik/config/acme.json";
/// Traefik renouvelle 30 jours avant la fin: à 14 jours, il échoue depuis deux semaines.
const CERT_WARN_DAYS: i64 = 14;
const DISK_WARN: u32 = 85;
const DISK_ERROR: u32 = 95;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Check {
    /// la section de la sortie: System, Docker, DNS, Containers...
    #[serde(default)]
    pub section: String,
    pub level: Level,
    pub detail: String,
    /// quoi faire, pour un warn ou un error
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Report {
    /// UTC, 2026-11-20T14:02:00Z
    pub date: String,
    pub checks: Vec<Check>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Summary {
    pub date: String,
    pub problems: usize,
    /// les sections à regarder (0 dans un doctor.json d'avant les sections)
    #[serde(default)]
    pub sections: usize,
}

impl Summary {
    /// « 20 Nov 14:02 », à l'heure du serveur.
    pub fn date_short(&self) -> String {
        system::run("date", &["-d", &self.date, "+%d %b %H:%M"]).unwrap_or(self.date.clone())
    }
}

impl Report {
    pub fn problems(&self) -> usize {
        self.checks.iter().filter(|c| c.level != Level::Ok).count()
    }
    pub fn summary(&self) -> Summary {
        Summary {
            date: self.date.clone(),
            problems: self.problems(),
            sections: self
                .sections()
                .iter()
                .filter(|(_, c)| c.iter().any(|c| c.level != Level::Ok))
                .count(),
        }
    }

    /// Les vérifications rangées par section, dans l'ordre de la sortie.
    pub fn sections(&self) -> Vec<(String, Vec<&Check>)> {
        let mut out: Vec<(String, Vec<&Check>)> = Vec::new();
        for c in &self.checks {
            match out.iter_mut().find(|(s, _)| *s == c.section) {
                Some((_, list)) => list.push(c),
                None => out.push((c.section.clone(), vec![c])),
            }
        }
        out
    }
}

fn ok(detail: impl Into<String>) -> Check {
    Check {
        section: String::new(),
        level: Level::Ok,
        detail: detail.into(),
        hint: String::new(),
    }
}
fn warn(detail: impl Into<String>, hint: impl Into<String>) -> Check {
    Check {
        section: String::new(),
        level: Level::Warn,
        detail: detail.into(),
        hint: hint.into(),
    }
}
fn error(detail: impl Into<String>, hint: impl Into<String>) -> Check {
    Check {
        section: String::new(),
        level: Level::Error,
        detail: detail.into(),
        hint: hint.into(),
    }
}

fn page(slug: &str) -> String {
    format!("see {}{slug}/", card::DOCS)
}

/// La sauvegarde (D97): ratée, trop vieille, le timer arrêté, ou seulement sur ce serveur.
fn backup_checks(cfg: &crate::config::Config) -> Vec<Check> {
    use crate::backup;
    let mut out = Vec::new();
    let settings = match backup::Settings::of(cfg) {
        Ok(s) => s,
        Err(e) => return vec![error(e, format!("fix {}", crate::config::PATH))],
    };
    match backup::last() {
        None => out.push(warn("no backup yet", "dotbox backup")),
        Some(r) if !r.ok => out.push(error(
            format!(
                "the last one failed ({}): {}",
                backup::short_date(&r.started),
                r.error.lines().next().unwrap_or_default()
            ),
            "dotbox backup, then its log in /opt/dotbox/logs",
        )),
        Some(r) => {
            let age = system::run("date", &["-d", &r.ended, "+%s"])
                .ok()
                .zip(system::run("date", &["+%s"]).ok())
                .and_then(|(t, n)| Some(n.parse::<i64>().ok()? - t.parse::<i64>().ok()?));
            let total: u64 = r.archives.iter().map(|a| a.bytes).sum();
            let text = format!(
                "{}, {}, {}",
                backup::short_date(&r.started),
                backup::count(r.archives.len()),
                backup::human(total)
            );
            if age.is_some_and(|a| a > 2 * 24 * 3600) {
                out.push(warn(
                    format!("{text}: more than two days ago"),
                    "systemctl status dotbox-backup.timer",
                ));
            } else {
                out.push(ok(text));
            }
        }
    }
    if settings.schedule != "off"
        && system::run("systemctl", &["is-active", "dotbox-backup.timer"]).as_deref()
            != Ok("active")
    {
        out.push(warn("dotbox-backup.timer is not running", "dotbox sync"));
    }
    if settings
        .destinations
        .iter()
        .all(|d| matches!(d, backup::Dest::Local(_)))
    {
        out.push(warn(
            "only on this server: a dead disk takes the backups with it",
            format!("add a remote destination, {}", page("backup")),
        ));
    }
    out
}

/// Le dernier résultat, s'il existe.
pub fn last() -> Option<Report> {
    serde_json::from_str(&std::fs::read_to_string(PATH).ok()?).ok()
}

/// Toutes les vérifications, puis le résultat gardé dans /opt/dotbox/doctor.json.
pub fn run() -> Result<Report, String> {
    let cfg = Config::load()?;
    let domain = cfg.get("domain").unwrap_or_default();
    let mut checks = Vec::new();
    let mut add = |section: &str, list: Vec<Check>| {
        checks.extend(list.into_iter().map(|mut c| {
            c.section = section.to_string();
            c
        }));
    };
    add("System", system_check());
    add("Docker", docker_checks());
    add(
        "DNS",
        vec![if domain.is_empty() {
            error("no domain in /opt/dotbox/dotbox.yml", "sudo dotbox init")
        } else {
            match traefik::dns_problem(&domain) {
                None => ok(format!("*.{domain} points to this server")),
                Some(why) => error(
                    why,
                    format!("point *.{domain} to this server, {}", page("prerequisites")),
                ),
            }
        }],
    );
    let st = status::collect()?;
    let mut containers = container_checks(&st.core, &st.apps);
    containers.extend(service_checks(&st.apps, &status::containers()?));
    add("Containers", containers);
    add(
        "Certificates",
        cert_checks(&domain, &cfg.get("ca").unwrap_or_default(), &st.apps),
    );
    add("CrowdSec", vec![crowdsec_check()]);
    let setup = setup_checks(&st.apps);
    if setup.is_empty() {
        add("Apps", vec![ok("none waiting for their first-run setup")]);
    } else {
        add("Apps", setup);
    }
    add("Updates", update_checks());
    add("SSH", ssh_checks());
    add("Disk", disk_checks());
    add("Backup", backup_checks(&cfg));
    add("Service", vec![service_check(), gate_check()]);
    let report = Report {
        date: system::run("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])?,
        checks,
    };
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    system::write_if_changed(PATH, &format!("{text}\n"), 0o644)?;
    Ok(report)
}

fn system_check() -> Vec<Check> {
    let os = match system::os_release() {
        Ok(os) => os,
        Err(e) => return vec![error(e, "")],
    };
    let arch = system::architecture().unwrap_or_default();
    if system::supported(&os) && arch == "amd64" {
        vec![ok(format!("{} {arch}", os.pretty))]
    } else {
        vec![error(
            format!("{} {arch} is not supported", os.pretty),
            "Debian 12 or 13, Ubuntu 24.04 or 26.04, amd64 only",
        )]
    }
}

fn docker_checks() -> Vec<Check> {
    if system::run("systemctl", &["is-active", "docker"]).as_deref() != Ok("active") {
        return vec![error(
            "docker is not running",
            "sudo systemctl start docker",
        )];
    }
    let mut out = Vec::new();
    let version = crate::docker::version();
    let network = system::run(
        "docker",
        &["network", "inspect", "-f", "{{.Name}}", "dotbox"],
    )
    .is_ok();
    let daemon = std::fs::read_to_string("/etc/docker/daemon.json").unwrap_or_default();
    let (local_logs, live) = daemon_settings(&daemon);
    if network && local_logs && live {
        out.push(ok(format!(
            "{version}, network dotbox, local logs, live-restore"
        )));
    } else {
        if !network {
            out.push(error("the dotbox network is missing", "sudo dotbox init"));
        }
        if !local_logs || !live {
            out.push(warn(
                "/etc/docker/daemon.json: local logs or live-restore missing",
                "sudo dotbox init",
            ));
        }
    }
    out
}

/// log-driver local et live-restore dans daemon.json.
pub fn daemon_settings(text: &str) -> (bool, bool) {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    (
        v.get("log-driver").and_then(|d| d.as_str()) == Some("local"),
        v.get("live-restore").and_then(|d| d.as_bool()) == Some(true),
    )
}

fn container_checks(core: &[Entry], apps: &[Entry]) -> Vec<Check> {
    let mut out = Vec::new();
    let mut fine = 0;
    for (e, is_core) in core
        .iter()
        .map(|e| (e, true))
        .chain(apps.iter().map(|e| (e, false)))
    {
        let again = if is_core {
            "sudo dotbox init".to_string()
        } else {
            format!("dotbox install {}", e.name)
        };
        match (e.state.as_str(), e.health.as_str()) {
            ("missing", _) => out.push(error(format!("{}: no container", e.name), again)),
            ("running", "unhealthy") => out.push(error(
                format!("{}: unhealthy", e.name),
                format!("docker logs {}", e.name),
            )),
            ("running", "starting") => out.push(warn(
                format!("{}: still starting", e.name),
                "run dotbox doctor again in a minute",
            )),
            ("running", _) => fine += 1,
            ("restarting", _) => out.push(error(
                format!("{}: restarts in a loop", e.name),
                format!("docker logs {}", e.name),
            )),
            // arrêtée: elle se relance, sans réinstaller
            (state @ ("exited" | "created"), _) if !is_core => out.push(error(
                format!("{}: {state}", e.name),
                format!("dotbox start {}", e.name),
            )),
            (state, _) => out.push(error(format!("{}: {state}", e.name), again)),
        }
    }
    if out.is_empty() {
        out.push(ok(format!("{fine} running")));
    }
    out
}

/// Les services d'une app qui tourne (sa base, son cache, D159): chacun doit tourner, healthy.
/// Une app arrêtée a ses services arrêtés avec elle: rien à dire.
fn service_checks(apps: &[Entry], list: &[status::Container]) -> Vec<Check> {
    let mut out = Vec::new();
    for e in apps.iter().filter(|e| e.state == "running") {
        let text =
            std::fs::read_to_string(crate::engine::compose_path(&e.name)).unwrap_or_default();
        for (name, _) in crate::sync::compose_images(&text)
            .into_iter()
            .filter(|(c, _)| c != &e.name)
        {
            let c = list.iter().find(|c| c.name == name);
            match c.map(|c| (c.state.as_str(), c.health.as_str())) {
                Some(("running", "healthy" | "")) => {}
                Some(("running", "starting")) => out.push(warn(
                    format!("{name}: still starting"),
                    "run dotbox doctor again in a minute",
                )),
                Some(("running", h)) => {
                    out.push(error(format!("{name}: {h}"), format!("dotbox logs {name}")))
                }
                Some((state, _)) => out.push(error(
                    format!("{name}: {state}, {} needs it", e.name),
                    format!("dotbox restart {}", e.name),
                )),
                None => out.push(error(
                    format!("{name}: no container, {} needs it", e.name),
                    format!("dotbox install {}", e.name),
                )),
            }
        }
    }
    out
}

/// Un certificat de acme.json: ses noms, sa fin, s'il vient du staging.
#[derive(Debug, PartialEq)]
pub struct Cert {
    pub names: Vec<String>,
    /// secondes depuis 1970
    pub not_after: i64,
    pub staging: bool,
}

/// Les certificats de acme.json (un objet par résolveur, chacun avec sa liste Certificates).
pub fn parse_acme(text: &str) -> Result<Vec<Cert>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("acme.json is not valid JSON: {e}"))?;
    let mut out = Vec::new();
    for resolver in v.as_object().into_iter().flat_map(|o| o.values()) {
        let list = resolver.get("Certificates").and_then(|c| c.as_array());
        for c in list.into_iter().flatten() {
            let mut names = Vec::new();
            if let Some(m) = c.pointer("/domain/main").and_then(|m| m.as_str()) {
                names.push(m.to_string());
            }
            for s in c
                .pointer("/domain/sans")
                .and_then(|s| s.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(s) = s.as_str() {
                    names.push(s.to_string());
                }
            }
            let b64 = c
                .get("certificate")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            let pem = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("acme.json: a certificate is not base64: {e}"))?;
            let (_, pem) = x509_parser::pem::parse_x509_pem(&pem)
                .map_err(|e| format!("acme.json: a certificate is not PEM: {e}"))?;
            let x509 = pem
                .parse_x509()
                .map_err(|e| format!("acme.json: a certificate cannot be read: {e}"))?;
            out.push(Cert {
                names,
                not_after: x509.validity().not_after.timestamp(),
                staging: x509.issuer().to_string().contains("STAGING"),
            });
        }
    }
    Ok(out)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Les noms qui doivent avoir un certificat: Authelia, le tableau de bord de Traefik, les apps.
pub fn expected_names(domain: &str, apps: &[Entry]) -> Vec<String> {
    let mut names = vec![format!("auth.{domain}"), format!("traefik.{domain}")];
    names.extend(
        apps.iter()
            .filter_map(|a| a.url.strip_prefix("https://").map(String::from)),
    );
    names
}

pub fn judge_certs(
    certs: &[Cert],
    expected: &[String],
    staging_wanted: bool,
    now: i64,
) -> Vec<Check> {
    let mut out = Vec::new();
    for name in expected {
        let Some(c) = certs.iter().find(|c| c.names.iter().any(|n| n == name)) else {
            out.push(warn(
                format!("{name}: no certificate yet"),
                format!("open https://{name} once, then see docker logs traefik"),
            ));
            continue;
        };
        let days = (c.not_after - now) / 86_400;
        if days < 0 {
            out.push(error(
                format!("{name}: certificate expired"),
                "docker logs traefik",
            ));
        } else if days < CERT_WARN_DAYS {
            out.push(warn(
                format!("{name}: certificate ends in {days} days, its renewal fails"),
                "docker logs traefik",
            ));
        }
        if c.staging && !staging_wanted {
            out.push(warn(
                format!("{name}: staging certificate, browsers do not trust it"),
                "ca: letsencrypt in /opt/dotbox/dotbox.yml, then sudo dotbox init",
            ));
        }
    }
    if out.is_empty() {
        let soonest = certs
            .iter()
            .filter(|c| c.names.iter().any(|n| expected.contains(n)))
            .map(|c| (c.not_after - now) / 86_400)
            .min()
            .unwrap_or_default();
        let kind = if staging_wanted { " (staging)" } else { "" };
        out.push(ok(format!(
            "{} valid{kind}, the first ends in {soonest} days",
            expected.len()
        )));
    }
    out
}

fn cert_checks(domain: &str, ca: &str, apps: &[Entry]) -> Vec<Check> {
    if domain.is_empty() {
        return Vec::new();
    }
    let text = match std::fs::read_to_string(ACME) {
        Ok(t) => t,
        Err(e) => {
            return vec![error(
                format!("cannot read {ACME}: {e}"),
                "sudo dotbox init",
            )];
        }
    };
    match parse_acme(&text) {
        Ok(certs) => judge_certs(
            &certs,
            &expected_names(domain, apps),
            ca == "letsencrypt-staging",
            now(),
        ),
        Err(e) => vec![error(e, "docker logs traefik")],
    }
}

fn crowdsec_check() -> Check {
    let state = crowdsec::state();
    if state != "healthy" {
        let shown = if state.is_empty() {
            "not installed".into()
        } else {
            state
        };
        return error(format!("crowdsec: {shown}"), "sudo dotbox init");
    }
    if !crowdsec::rules_in_place() {
        return error(
            "crowdsec: the bouncer rules are missing from INPUT or DOCKER-USER",
            "sudo systemctl restart crowdsec-firewall-bouncer",
        );
    }
    ok("running, bouncer rules in INPUT and DOCKER-USER")
}

fn setup_checks(apps: &[Entry]) -> Vec<Check> {
    apps.iter()
        .filter(|a| a.setup_pending)
        .map(|a| {
            warn(
                format!(
                    "{}: first-run setup not finished, behind Authelia until then",
                    a.name
                ),
                format!(
                    "open {}, finish its setup, then dotbox install {}",
                    a.url, a.name
                ),
            )
        })
        .collect()
}

/// `apt-get -s upgrade`: les paquets en attente, et ceux qui viennent du dépôt de sécurité.
pub fn parse_apt(text: &str) -> (usize, usize) {
    let inst: Vec<&str> = text.lines().filter(|l| l.starts_with("Inst ")).collect();
    let security = inst
        .iter()
        .filter(|l| l.contains("-security") || l.contains("Debian-Security"))
        .count();
    (inst.len(), security)
}

fn update_checks() -> Vec<Check> {
    let mut out = Vec::new();
    match system::run("apt-get", &["-s", "-o", "Debug::NoLocking=1", "upgrade"]) {
        Ok(text) => {
            let (all, security) = parse_apt(&text);
            if all == 0 {
                out.push(ok("the system is up to date"));
            } else {
                out.push(warn(
                    format!(
                        "{all} system {} pending ({security} security)",
                        if all == 1 { "update" } else { "updates" }
                    ),
                    "sudo apt update && sudo apt full-upgrade",
                ));
            }
        }
        Err(e) => out.push(warn(format!("cannot list system updates: {e}"), "")),
    }
    let auto = system::run("apt-config", &["dump", "APT::Periodic::Unattended-Upgrade"])
        .unwrap_or_default();
    if !system::package_installed("unattended-upgrades") || !auto.contains("\"1\"") {
        out.push(warn(
            "automatic security updates are off",
            "sudo apt install unattended-upgrades && sudo dpkg-reconfigure unattended-upgrades",
        ));
    }
    if std::path::Path::new("/var/run/reboot-required").exists() {
        out.push(warn("reboot required", "sudo reboot, when it suits you"));
    }
    out
}

/// `sshd -T`: le mot de passe accepté, root qui peut se connecter.
pub fn parse_sshd(text: &str) -> (bool, bool) {
    let value = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()))
            .unwrap_or_default()
    };
    (
        value("passwordauthentication ") == "yes",
        value("permitrootlogin ") == "yes",
    )
}

fn ssh_checks() -> Vec<Check> {
    let text = match system::run("sshd", &["-T"]) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let (password, root) = parse_sshd(&text);
    let mut out = Vec::new();
    if password {
        out.push(warn("accepts passwords", page("post-installation")));
    }
    if root {
        out.push(warn("allows root login", page("post-installation")));
    }
    if out.is_empty() {
        out.push(ok("keys only, no root login"));
    }
    out
}

/// `df -P`: le point de montage et son pourcentage d'occupation, une fois par système de fichiers.
pub fn parse_df(text: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for l in text.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        let pct = f[4].trim_end_matches('%').parse().unwrap_or(0);
        let mount = f[5..].join(" ");
        if !out.iter().any(|(m, _)| *m == mount) {
            out.push((mount, pct));
        }
    }
    out
}

fn disk_checks() -> Vec<Check> {
    let paths: Vec<&str> = ["/", "/opt", "/data"]
        .into_iter()
        .filter(|p| std::path::Path::new(p).exists())
        .collect();
    let mut args = vec!["-P"];
    args.extend(paths);
    let Ok(text) = system::run("df", &args) else {
        return Vec::new();
    };
    parse_df(&text)
        .into_iter()
        .map(|(mount, pct)| {
            if pct >= DISK_ERROR {
                error(format!("{mount} {pct}% used"), "free some space")
            } else if pct >= DISK_WARN {
                warn(format!("{mount} {pct}% used"), "free some space")
            } else {
                ok(format!("{mount} {pct}% used"))
            }
        })
        .collect()
}

fn service_check() -> Check {
    if system::run("systemctl", &["is-active", "dotbox"]).as_deref() == Ok("active") {
        ok("dotbox running")
    } else {
        error("dotbox is not running", "sudo systemctl restart dotbox")
    }
}

/// Une requête à la porte web, en direct sur la passerelle: son code HTTP.
fn gate_code(gateway: &str, headers: &str) -> Result<u16, String> {
    use std::io::{Read, Write};
    let addr = format!("{gateway}:{}", crate::serve::WEB_PORT);
    let mut s = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{addr}: {e}"))?,
        std::time::Duration::from_secs(3),
    )
    .map_err(|e| format!("{addr}: {e}"))?;
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    s.write_all(
        format!(
            "GET /api/v1/version HTTP/1.1\r\nHost: dotbox\r\n{headers}Connection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    let mut head = [0u8; 12];
    s.read_exact(&mut head).map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&head[9..12])
        .parse()
        .map_err(|_| "not an HTTP answer".to_string())
}

/// La porte web (11.15): elle refuse sans le secret de Traefik, et répond avec.
fn gate_check() -> Check {
    let hint = "dotbox sync, then sudo systemctl restart dotbox";
    let gw = match crate::serve::gateway() {
        Ok(g) => g,
        Err(e) => return error(format!("the web gate: {e}"), hint),
    };
    let secret = crate::secrets::read(crate::serve::GATE_SECRET).unwrap_or_default();
    let with = format!(
        "{}: {}\r\nRemote-Groups: {}\r\n",
        crate::serve::GATE_HEADER_NAME,
        secret.trim(),
        crate::serve::WEB_GROUP
    );
    match (gate_code(&gw, ""), gate_code(&gw, &with)) {
        (Ok(403), Ok(200)) => ok("web gate: refuses without Traefik's secret, answers with it"),
        (Ok(c), _) if c != 403 => {
            error(format!("the web gate answers {c} without the secret"), hint)
        }
        (Err(e), _) | (_, Err(e)) => error(format!("the web gate does not answer: {e}"), hint),
        (_, Ok(c)) => error(format!("the web gate answers {c} with the secret"), hint),
    }
}

/// Le cadre façon cliclack dans un terminal (#93), du texte simple sinon.
pub fn print(r: &Report) {
    if clack::fancy() {
        print_fancy(r)
    } else {
        print_plain(r)
    }
}

fn mark(level: Level) -> Mark {
    match level {
        Level::Ok => Mark::Ok,
        Level::Warn => Mark::Warn,
        Level::Error => Mark::Error,
    }
}

/// Le texte d'une section: ce qui va bien en une ligne, sinon le problème, ou leur nombre.
pub fn headline(checks: &[&Check]) -> String {
    let problems: Vec<&&Check> = checks.iter().filter(|c| c.level != Level::Ok).collect();
    match problems.len() {
        0 => checks
            .iter()
            .map(|c| c.detail.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        1 => problems[0].detail.clone(),
        n => format!("{n} to look at"),
    }
}

fn print_fancy(r: &Report) {
    clack::open("dotbox doctor");
    let sections = r.sections();
    let width = sections.iter().map(|(s, _)| s.len()).max().unwrap_or(0) + 3;
    let mut to_look = 0;
    let mut worst = Level::Ok;
    for (name, checks) in &sections {
        let level = checks
            .iter()
            .map(|c| c.level)
            .max_by_key(|l| mark(*l))
            .unwrap_or(Level::Ok);
        if level != Level::Ok {
            to_look += 1;
        }
        if mark(level) > mark(worst) {
            worst = level;
        }
        clack::item(mark(level), name, width, &headline(checks));
        let problems: Vec<&&Check> = checks.iter().filter(|c| c.level != Level::Ok).collect();
        for c in &problems {
            if problems.len() > 1 {
                clack::sub_mark(mark(c.level), &c.detail);
            }
            if !c.hint.is_empty() {
                clack::sub(&format!("→ {}", c.hint));
            }
        }
    }
    match to_look {
        0 => clack::close(&Mark::Ok.paint("all good")),
        n => clack::close(&mark(worst).paint(&format!(
            "{n} {} to look at",
            if n == 1 { "section" } else { "sections" }
        ))),
    }
}

/// Sans terminal: une ligne par vérification, pour un script ou un log.
fn print_plain(r: &Report) {
    for c in &r.checks {
        let label = match c.level {
            Level::Ok => "ok   ",
            Level::Warn => "warn ",
            Level::Error => "error",
        };
        crate::clack::put(&format!("{label} {:<13}{}", c.section, c.detail));
        if !c.hint.is_empty() {
            crate::clack::put(&format!("      {:<13}→ {}", "", c.hint));
        }
    }
    crate::clack::put(&format!("{} problems", r.problems()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_settings() {
        assert_eq!(
            daemon_settings(r#"{"log-driver": "local", "live-restore": true}"#),
            (true, true)
        );
        assert_eq!(daemon_settings(""), (false, false));
    }

    #[test]
    fn apt_updates() {
        let text = "NOTE: This is only a simulation!\n\
            Inst libc6 [2.41-12] (2.41-12+deb13u1 Debian-Security:13/stable-security [amd64])\n\
            Inst curl [8.14.1-2] (8.14.1-2+deb13u2 Debian:13.3/stable [amd64])\n\
            Conf libc6 (2.41-12+deb13u1 Debian-Security:13/stable-security [amd64])\n";
        assert_eq!(parse_apt(text), (2, 1));
        assert_eq!(parse_apt("0 upgraded, 0 newly installed"), (0, 0));
    }

    #[test]
    fn sshd_settings() {
        assert_eq!(
            parse_sshd("port 22\npasswordauthentication yes\npermitrootlogin without-password\n"),
            (true, false)
        );
        assert_eq!(
            parse_sshd("passwordauthentication no\npermitrootlogin yes\n"),
            (false, true)
        );
    }

    #[test]
    fn disk_usage() {
        let text = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
            /dev/sda1         40000000  10000000  30000000      25% /\n\
            /dev/sda1         40000000  10000000  30000000      25% /\n\
            /dev/sdb1        900000000 800000000 100000000      89% /data\n";
        assert_eq!(parse_df(text), vec![("/".into(), 25), ("/data".into(), 89)]);
    }

    /// Un certificat de test (« (STAGING) Test », fin le 1er janvier 2030), en PEM puis en base64,
    /// comme Traefik le range dans acme.json.
    const PEM: &str = include_str!("../tests/expected/doctor/staging-cert.pem");

    fn acme(name: &str) -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(PEM);
        format!(
            r#"{{"letsencrypt": {{"Account": {{}}, "Certificates": [{{"domain": {{"main": "{name}"}}, "certificate": "{b64}", "key": "x", "Store": "default"}}]}}}}"#
        )
    }

    #[test]
    fn certificates_from_acme_json() {
        let certs = parse_acme(&acme("radarr.example.com")).unwrap();
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].names, ["radarr.example.com"]);
        assert!(certs[0].staging);
        assert_eq!(certs[0].not_after, 1_893_456_000);
        assert!(
            parse_acme(r#"{"letsencrypt": {"Certificates": null}}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn certificates_judged() {
        let certs = parse_acme(&acme("radarr.example.com")).unwrap();
        let expected = vec!["radarr.example.com".to_string()];
        let day = 86_400;
        // staging voulu, 30 jours avant la fin
        let c = judge_certs(&certs, &expected, true, 1_893_456_000 - 30 * day);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].level, Level::Ok);
        // 10 jours avant la fin, et staging alors que Let's Encrypt est voulu
        let c = judge_certs(&certs, &expected, false, 1_893_456_000 - 10 * day);
        assert_eq!(c.iter().filter(|c| c.level == Level::Warn).count(), 2);
        // un nom sans certificat
        let c = judge_certs(&certs, &["sonarr.example.com".to_string()], true, 0);
        assert_eq!(c[0].level, Level::Warn);
        assert!(c[0].detail.contains("no certificate"));
    }

    #[test]
    fn sections_and_headlines() {
        let mut a = ok("29.10.0, network dotbox");
        a.section = "Docker".into();
        let mut b = warn("3 system updates pending (1 security)", "sudo apt update");
        b.section = "Updates".into();
        let mut c = warn("automatic security updates are off", "x");
        c.section = "Updates".into();
        let r = Report {
            date: "2026-11-20T07:57:00Z".into(),
            checks: vec![a, b, c],
        };
        let s = r.sections();
        assert_eq!(s.len(), 2);
        assert_eq!(headline(&s[0].1), "29.10.0, network dotbox");
        assert_eq!(headline(&s[1].1), "2 to look at");
        assert_eq!(
            headline(&s[1].1[..1]),
            "3 system updates pending (1 security)"
        );
        assert_eq!(r.summary().sections, 1);
        assert_eq!(r.summary().problems, 2);
    }

    #[test]
    fn containers_judged() {
        let e = |name: &str, state: &str, health: &str| Entry {
            name: name.into(),
            version: "1".into(),
            state: state.into(),
            health: health.into(),
            url: String::new(),
            access: String::new(),
            setup_pending: false,
        };
        let c = container_checks(
            &[e("traefik", "running", "healthy")],
            &[e("radarr", "running", "")],
        );
        assert_eq!(c, vec![ok("2 running")]);
        let c = container_checks(
            &[e("authelia", "missing", "")],
            &[
                e("radarr", "exited", ""),
                e("sonarr", "running", "unhealthy"),
            ],
        );
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|c| c.level == Level::Error));
        assert_eq!(c[1].hint, "dotbox start radarr");
        assert_eq!(c[0].hint, "sudo dotbox init");
    }
}
