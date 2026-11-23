//! `dotbox motd` (Journal, étape 17, D86, D89, D90, D107): le message d'accueil SSH. À chaque
//! connexion, pam_motd lance /etc/update-motd.d/60-dotbox en root, qui appelle `dotbox motd
//! --login`. Ce qui est rapide est lu en direct (/proc, df, docker ps, acme.json, backup.json).
//! Ce qui est lent est lu dans /opt/dotbox/motd.json, écrit par `dotbox motd --refresh` (chaque
//! nuit, par dotbox-doctor.timer, qui lance aussi doctor et relit le catalogue) et, pour CrowdSec,
//! par le service toutes les dix minutes. Chaque ligne en cache dit son âge.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use console::style;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::doctor::{self, Level};
use crate::out::say;
use crate::{backup, github, status, system, upgrade};

pub const SCRIPT: &str = "/etc/update-motd.d/60-dotbox";
pub const UNAME: &str = "/etc/update-motd.d/10-uname";
pub const ETC_MOTD: &str = "/etc/motd";
/// La copie de /etc/motd, remise quand motd.enabled passe à false.
pub const KEPT: &str = "/opt/dotbox/motd/etc-motd";
/// Présent quand Dot Box a rendu 10-uname non exécutable: il le remet en partant.
pub const UNAME_MARK: &str = "/opt/dotbox/motd/10-uname-was-executable";
pub const CACHE: &str = "/opt/dotbox/motd.json";
pub const TIMER: &str = "/etc/systemd/system/dotbox-doctor.timer";
pub const SERVICE: &str = "/etc/systemd/system/dotbox-doctor.service";
pub const STYLES: &[&str] = &["compact", "fastfetch", "banner", "ascii"];
pub const DEFAULT_STYLE: &str = "compact";
const WARN_PCT: u64 = 90;
const CERT_WARN_DAYS: i64 = 14;
const OLD: i64 = 2 * 86_400;

/// Le script de pam_motd. timeout: un Docker qui ne répond plus ne bloque pas la connexion.
pub const SCRIPT_TEXT: &str = "#!/bin/sh
# Écrit par dotbox (init, sync): le message d'accueil de Dot Box, motd de /opt/dotbox/dotbox.yml (D86).
[ -x /usr/local/bin/dotbox ] || exit 0
exec timeout 5 /usr/local/bin/dotbox motd --login
";

// ---------- les réglages

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub style: String,
}

impl Settings {
    pub fn of(cfg: &Config) -> Result<Settings, String> {
        let mut s = Settings {
            enabled: true,
            style: DEFAULT_STYLE.into(),
        };
        let Some(m) = cfg.section("motd") else {
            return Ok(s);
        };
        if let Some(v) = m.get("enabled") {
            s.enabled = v.as_bool().ok_or("motd.enabled: true or false")?;
        }
        if let Some(v) = m.get("style") {
            let text = v.as_str().unwrap_or_default();
            check_style(text)?;
            s.style = text.into();
        }
        Ok(s)
    }
}

pub fn check_style(s: &str) -> Result<(), String> {
    if STYLES.contains(&s) {
        Ok(())
    } else {
        Err(format!("motd.style {s:?}: {}", STYLES.join(", ")))
    }
}

// ---------- le cache (D89)

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Stamp<T> {
    /// secondes depuis 1970
    pub at: i64,
    pub value: T,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Big {
    pub mount: String,
    pub folder: String,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Cache {
    /// les IP bannies par CrowdSec ces dernières 24 h
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bans: Option<Stamp<usize>>,
    /// la dernière release de Dot Box
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Stamp<String>>,
    /// les paquets Debian en attente, et ceux de sécurité
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apt: Option<Stamp<[usize; 2]>>,
    /// le plus gros dossier d'un disque plein à plus de 90 %
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biggest: Option<Stamp<Vec<Big>>>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub fn load_cache() -> Cache {
    std::fs::read_to_string(CACHE)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Relit le fichier juste avant d'écrire: le service et la nuit y écrivent chacun leur part.
fn save_cache(change: impl FnOnce(&mut Cache)) -> Result<(), String> {
    let mut c = load_cache();
    change(&mut c);
    let text = serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?;
    system::write_if_changed(CACHE, &format!("{text}\n"), 0o644).map(|_| ())
}

/// `cscli alerts list -o json`: les IP qui ont reçu une décision.
pub fn parse_bans(json: &str) -> Result<usize, String> {
    let alerts: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|e| format!("cscli alerts list: {e}"))?;
    let mut ips: Vec<&str> = alerts
        .iter()
        .filter(|a| {
            a.get("decisions")
                .and_then(|d| d.as_array())
                .is_some_and(|d| !d.is_empty())
        })
        .filter_map(|a| a.get("source")?.get("value")?.as_str())
        .collect();
    ips.sort_unstable();
    ips.dedup();
    Ok(ips.len())
}

/// Les bannissements des dernières 24 h (le service, toutes les dix minutes).
pub fn refresh_bans() -> Result<usize, String> {
    let json = system::run(
        "docker",
        &[
            "exec", "crowdsec", "cscli", "alerts", "list", "--since", "24h", "-l", "0", "-o",
            "json",
        ],
    )?;
    // cscli écrit « null » quand il n'y a rien
    let n = if json.trim() == "null" {
        0
    } else {
        parse_bans(&json)?
    };
    save_cache(|c| {
        c.bans = Some(Stamp {
            at: now(),
            value: n,
        })
    })?;
    Ok(n)
}

/// `du -x -d1 -B1 <point de montage>`: le plus gros dossier, sans le total.
pub fn parse_du(text: &str, mount: &str) -> Option<(String, u64)> {
    text.lines()
        .filter_map(|l| {
            let (size, path) = l.split_once('\t')?;
            Some((path.trim().to_string(), size.trim().parse().ok()?))
        })
        .filter(|(p, _)| p != mount)
        .max_by_key(|(_, b)| *b)
}

/// dotbox motd --refresh, et la nuit: doctor, le catalogue, la dernière release, apt, CrowdSec,
/// et le plus gros dossier d'un disque plein. Chaque morceau qui échoue laisse l'ancien.
pub fn refresh() -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(e) = doctor::run() {
        errors.push(format!("doctor: {e}"));
    }
    if let Err(e) = github::fetch() {
        errors.push(format!("catalog: {e}"));
    }
    let api = format!(
        "https://api.github.com/repos/{}/releases/latest",
        upgrade::REPO
    );
    match github::get(&api).map(|j| upgrade::tag_of(&j)) {
        Ok(Some(tag)) => save_cache(|c| {
            c.release = Some(Stamp {
                at: now(),
                value: tag,
            })
        })?,
        Ok(None) => errors.push("release: GitHub gave no latest release".into()),
        Err(e) => errors.push(format!("release: {e}")),
    }
    match system::run("apt-get", &["-s", "-o", "Debug::NoLocking=1", "upgrade"]) {
        Ok(text) => {
            let (all, security) = doctor::parse_apt(&text);
            save_cache(|c| {
                c.apt = Some(Stamp {
                    at: now(),
                    value: [all, security],
                })
            })?;
        }
        Err(e) => errors.push(format!("apt: {e}")),
    }
    if let Err(e) = refresh_bans() {
        errors.push(format!("crowdsec: {e}"));
    }
    let mut big = Vec::new();
    for (mount, pct) in disks() {
        if pct >= WARN_PCT {
            if let Some((folder, bytes)) = system::run("du", &["-x", "-d1", "-B1", &mount])
                .ok()
                .and_then(|t| parse_du(&t, &mount))
            {
                big.push(Big {
                    mount,
                    folder,
                    bytes,
                });
            }
        }
    }
    save_cache(|c| {
        c.biggest = Some(Stamp {
            at: now(),
            value: big,
        })
    })?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(", "))
    }
}

// ---------- les lignes (D90)

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Line {
    pub level: Level,
    /// vide pour une alerte: elle n'apparaît que quand il y a quelque chose à faire
    #[serde(default)]
    pub name: String,
    pub text: String,
    /// en gris après le texte: les apps arrêtées exprès
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra: String,
    /// la commande à lancer
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Motd {
    pub version: String,
    pub domain: String,
    /// motd.style de dotbox.yml
    pub style: String,
    pub lines: Vec<Line>,
}

fn state(name: &str, text: String) -> Line {
    Line {
        level: Level::Ok,
        name: name.into(),
        text,
        extra: String::new(),
        hint: String::new(),
    }
}

fn alert(level: Level, text: String, hint: &str) -> Line {
    Line {
        level,
        name: String::new(),
        text,
        extra: String::new(),
        hint: hint.into(),
    }
}

/// « 5 min ago », « 2 h ago », « 3 days ago ».
pub fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 2 * 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// « up 3 days », « up 5 h », « up 12 min ».
pub fn uptime(secs: i64) -> String {
    match secs.max(0) {
        s if s < 3600 => format!("up {} min", s / 60),
        s if s < 2 * 86_400 => format!("up {} h", s / 3600),
        s => format!("up {} days", s / 86_400),
    }
}

/// « 2.1 / 7.8 GiB », un chiffre après la virgule.
fn gib(kb: u64) -> String {
    format!("{:.1}", kb as f64 / 1_048_576.0)
}

/// /proc/meminfo: (total, disponible, swap totale, swap libre), en kB.
pub fn parse_meminfo(text: &str) -> (u64, u64, u64, u64) {
    let v = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|r| r.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    (
        v("MemTotal:"),
        v("MemAvailable:"),
        v("SwapTotal:"),
        v("SwapFree:"),
    )
}

pub fn memory_line(meminfo: &str) -> Line {
    let (total, avail, swap_total, swap_free) = parse_meminfo(meminfo);
    let used = total.saturating_sub(avail);
    let mut text = format!("{} / {} GiB", gib(used), gib(total));
    let swap = swap_total.saturating_sub(swap_free);
    if swap > 0 {
        text.push_str(&format!(" · swap {} GiB", gib(swap)));
    }
    let mut l = state("Memory", text);
    if total > 0 && used * 100 / total >= WARN_PCT {
        l.level = Level::Warn;
        l.text.push_str(&format!(" ({} %)", used * 100 / total));
    }
    l
}

/// « Debian 13 · up 3 days · load 0.42 », jaune quand la charge sur 5 minutes dépasse les CPU.
pub fn system_line(os: &str, uptime_secs: i64, loadavg: &str, cpus: usize) -> Line {
    let f: Vec<f64> = loadavg
        .split_whitespace()
        .take(2)
        .filter_map(|x| x.parse().ok())
        .collect();
    let (one, five) = (
        f.first().copied().unwrap_or(0.0),
        f.get(1).copied().unwrap_or(0.0),
    );
    let mut l = state(
        "System",
        format!("{os} · {} · load {one:.2}", uptime(uptime_secs)),
    );
    if five > cpus as f64 {
        l.level = Level::Warn;
        l.text.push_str(&format!(
            " ({five:.2} over 5 min, {cpus} {})",
            if cpus == 1 { "CPU" } else { "CPUs" }
        ));
    }
    l
}

fn disks() -> Vec<(String, u64)> {
    let paths: Vec<&str> = ["/", "/opt", "/data"]
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect();
    let mut args = vec!["-P"];
    args.extend(paths);
    system::run("df", &args)
        .map(|t| {
            doctor::parse_df(&t)
                .into_iter()
                .map(|(m, p)| (m, p as u64))
                .collect()
        })
        .unwrap_or_default()
}

pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e12 {
        format!("{:.1} TB", b / 1e12)
    } else {
        backup::human(bytes)
    }
}

pub fn disks_line(list: &[(String, u64)], cache: &Cache, now: i64) -> Line {
    let text = list
        .iter()
        .map(|(m, p)| format!("{m} {p} %"))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut l = state("Disks", text);
    let full: Vec<&(String, u64)> = list.iter().filter(|(_, p)| *p >= WARN_PCT).collect();
    if !full.is_empty() {
        l.level = Level::Warn;
        if let Some(b) = &cache.biggest {
            for (m, _) in &full {
                if let Some(big) = b.value.iter().find(|x| &x.mount == m) {
                    l.text.push_str(&format!(
                        " · biggest {} {}, {}",
                        big.folder,
                        human(big.bytes),
                        ago(now - b.at)
                    ));
                }
            }
        }
        l.hint = "free some space".into();
    }
    l
}

/// Une date de doctor.json ou de backup.json en secondes, par `date` (le fuseau du serveur).
fn epoch(date: &str) -> Option<i64> {
    system::run("date", &["-d", date, "+%s"]).ok()?.parse().ok()
}

pub fn doctor_line(d: Option<&doctor::Summary>, at: Option<i64>, now: i64) -> Line {
    let mut l = state("Doctor", String::new());
    let Some(d) = d else {
        l.level = Level::Warn;
        l.text = "never run".into();
        l.hint = "dotbox doctor".into();
        return l;
    };
    let age = at.map(|t| ago(now - t)).unwrap_or_default();
    let n = if d.sections > 0 {
        d.sections
    } else {
        d.problems
    };
    if d.problems == 0 {
        l.text = format!("nothing to report, {age}");
    } else {
        l.level = Level::Warn;
        l.text = format!(
            "{n} {} to look at, {age}",
            if n == 1 { "section" } else { "sections" }
        );
        l.hint = "dotbox doctor".into();
    }
    if at.is_some_and(|t| now - t > OLD) {
        l.level = Level::Warn;
        l.hint = "dotbox doctor".into();
    }
    l
}

pub fn backup_line(b: &backup::Summary, at: Option<i64>, now: i64) -> Line {
    let mut l = state("Backup", String::new());
    let when = at
        .map(|t| ago(now - t))
        .unwrap_or_else(|| backup::short_date(&b.started));
    if !b.ok {
        l.level = Level::Error;
        l.text = format!("failed {when}: {}", b.error);
        l.hint = "dotbox backup".into();
        return l;
    }
    l.text = format!(
        "{when} · {} · {}",
        backup::count(b.archives),
        backup::human(b.bytes)
    );
    if at.is_some_and(|t| now - t > OLD) {
        l.level = Level::Warn;
        l.hint = "systemctl status dotbox-backup.timer".into();
    }
    l
}

pub fn crowdsec_line(cache: &Cache, now: i64) -> Line {
    match &cache.bans {
        None => {
            let mut l = state("CrowdSec", "bans not counted yet".into());
            l.extra = " (the service counts them every 10 min)".into();
            l
        }
        Some(b) => state(
            "CrowdSec",
            format!(
                "{} {} banned in 24 h, {}",
                b.value,
                if b.value == 1 { "IP" } else { "IPs" },
                ago(now - b.at)
            ),
        ),
    }
}

/// Les apps: « 14 running · 1 stopped (sonarr) », et une alerte par app qui devrait tourner.
pub fn apps_lines(cfg: &Config, list: &[status::Container]) -> Vec<Line> {
    let names = cfg.app_names();
    if names.is_empty() {
        let mut l = state("Apps", "none yet".into());
        l.extra = " (dotbox list)".into();
        return vec![l];
    }
    let mut running = 0;
    let mut stopped = Vec::new();
    let mut alerts = Vec::new();
    for app in &names {
        let c = list.iter().find(|c| &c.name == app);
        let st = c.map(|c| c.state.as_str()).unwrap_or("missing");
        if cfg.app_stopped(app) {
            stopped.push(app.clone());
        } else if st != "running" {
            alerts.push(alert(
                Level::Error,
                format!("{app} {st}"),
                &format!("dotbox start {app}"),
            ));
        } else if c.is_some_and(|c| c.health == "unhealthy") {
            alerts.push(alert(
                Level::Warn,
                format!("{app} unhealthy"),
                &format!("dotbox logs {app}"),
            ));
            running += 1;
        } else {
            running += 1;
        }
    }
    let mut l = state("Apps", format!("{running} running"));
    if !stopped.is_empty() {
        l.extra = format!(" · {} stopped ({})", stopped.len(), stopped.join(", "));
    }
    let mut out = vec![l];
    out.extend(alerts);
    out
}

/// Le core: rien quand tout tourne, une alerte par morceau arrêté.
pub fn core_lines(list: &[status::Container], service_active: bool) -> Vec<Line> {
    let mut out = Vec::new();
    for name in status::CORE {
        let c = list.iter().find(|c| &c.name == name);
        let st = c.map(|c| c.state.as_str()).unwrap_or("missing");
        if st != "running" {
            out.push(alert(
                Level::Error,
                format!("{name} {st}"),
                &format!("dotbox logs {name}"),
            ));
        } else if c.is_some_and(|c| c.health == "unhealthy") {
            out.push(alert(
                Level::Warn,
                format!("{name} unhealthy"),
                &format!("dotbox logs {name}"),
            ));
        }
    }
    if !service_active {
        out.push(alert(
            Level::Error,
            "the Dot Box service is not running".into(),
            "sudo systemctl restart dotbox",
        ));
    }
    out
}

/// Les apps en retard sur le catalogue rangé (D89: il suit le dernier update, ou la nuit).
pub fn behind(cfg: &Config, list: &[status::Container], dir: &Path) -> Vec<String> {
    cfg.app_names()
        .into_iter()
        .filter(|app| {
            let Ok(m) = crate::add::load(dir, app) else {
                return false;
            };
            list.iter()
                .find(|c| &c.name == app)
                .is_some_and(|c| status::tag(&c.image) != m.version())
        })
        .collect()
}

pub fn update_lines(
    behind: &[String],
    cache: &Cache,
    current: &str,
    reboot: bool,
    now: i64,
) -> Vec<Line> {
    let mut out = Vec::new();
    match behind.len() {
        0 => {}
        1 => out.push(alert(
            Level::Warn,
            format!("{} behind", behind[0]),
            &format!("dotbox update {}", behind[0]),
        )),
        n => out.push(alert(
            Level::Warn,
            format!("{n} apps behind"),
            "dotbox update --all",
        )),
    }
    if let Some(r) = &cache.release {
        if upgrade::newer(&r.value, current) {
            out.push(alert(
                Level::Warn,
                format!(
                    "Dot Box {} is out, {}",
                    r.value.trim_start_matches('v'),
                    ago(now - r.at)
                ),
                "sudo dotbox upgrade",
            ));
        }
    }
    if let Some(a) = &cache.apt {
        let [all, security] = a.value;
        if all > 0 {
            out.push(alert(
                Level::Warn,
                format!(
                    "{all} Debian {} ({security} security), {}",
                    if all == 1 { "update" } else { "updates" },
                    ago(now - a.at)
                ),
                "sudo apt update && sudo apt full-upgrade",
            ));
        }
    }
    if reboot {
        out.push(alert(
            Level::Warn,
            "reboot required".into(),
            "sudo reboot, when it suits you",
        ));
    }
    out
}

/// Le certificat le plus proche de sa fin, seulement sous 14 jours.
pub fn cert_lines(certs: &[doctor::Cert], expected: &[String], now: i64) -> Vec<Line> {
    let soonest = certs
        .iter()
        .filter_map(|c| {
            let name = c.names.iter().find(|n| expected.contains(n))?;
            Some((name.clone(), (c.not_after - now) / 86_400))
        })
        .min_by_key(|(_, d)| *d);
    match soonest {
        Some((name, d)) if d < 0 => vec![alert(
            Level::Error,
            format!("{name}: certificate expired"),
            "docker logs traefik",
        )],
        Some((name, d)) if d < CERT_WARN_DAYS => vec![alert(
            Level::Warn,
            format!("{name}: certificate ends in {d} days"),
            "docker logs traefik",
        )],
        _ => Vec::new(),
    }
}

/// « Debian 13 », comme l'étape System de dotbox init.
fn short_os() -> String {
    match system::os_release() {
        Ok(os) => {
            let mut id = os.id.clone();
            if let Some(c) = id.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            format!("{id} {}", os.version_id)
        }
        Err(_) => "Linux".into(),
    }
}

/// Tout ce que montre le message, en root: rien n'attend le réseau.
pub fn collect() -> Result<Motd, String> {
    let cfg = Config::load()?;
    let settings = Settings::of(&cfg)?;
    let domain = cfg.get("domain").unwrap_or_default();
    let t = now();
    let cache = load_cache();
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let up: i64 = read("/proc/uptime")
        .split('.')
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let mut lines = vec![
        system_line(&short_os(), up, &read("/proc/loadavg"), cpus),
        memory_line(&read("/proc/meminfo")),
        disks_line(&disks(), &cache, t),
    ];
    let list = status::containers().unwrap_or_default();
    let mut alerts = core_lines(
        &list,
        system::run("systemctl", &["is-active", "dotbox"]).as_deref() == Ok("active"),
    );
    let apps = apps_lines(&cfg, &list);
    lines.push(apps[0].clone());
    alerts.extend(apps.into_iter().skip(1));
    let last = doctor::last();
    lines.push(doctor_line(
        last.as_ref().map(|r| r.summary()).as_ref(),
        last.as_ref().and_then(|r| epoch(&r.date)),
        t,
    ));
    if let Some(b) = backup::summary() {
        let at = epoch(&b.started);
        lines.push(backup_line(&b, at, t));
    }
    lines.push(crowdsec_line(&cache, t));
    alerts.extend(update_lines(
        &behind(&cfg, &list, Path::new(github::DIR)),
        &cache,
        env!("CARGO_PKG_VERSION"),
        Path::new("/run/reboot-required").exists(),
        t,
    ));
    if !domain.is_empty() {
        if let Ok(certs) = doctor::parse_acme(&read(doctor::ACME)) {
            let entries: Vec<status::Entry> = cfg
                .app_names()
                .iter()
                .map(|app| {
                    let (sub, _) = cfg.app(app).unwrap_or_default();
                    let sub = if sub.is_empty() { app.clone() } else { sub };
                    status::Entry {
                        name: app.clone(),
                        version: String::new(),
                        state: String::new(),
                        health: String::new(),
                        url: format!("https://{sub}.{domain}"),
                        access: String::new(),
                        setup_pending: false,
                    }
                })
                .collect();
            alerts.extend(cert_lines(
                &certs,
                &doctor::expected_names(&domain, &entries),
                t,
            ));
        }
    }
    lines.extend(alerts);
    Ok(Motd {
        version: env!("CARGO_PKG_VERSION").into(),
        domain,
        style: settings.style,
        lines,
    })
}

// ---------- le dessin (D86, D107)

const PICTO: [&str; 7] = [
    "● ● ● ● ● ● ●",
    "●           ●",
    "●   ● ● ●   ●",
    "●   ●   ●   ●",
    "●   ● ● ●   ●",
    "●           ●",
    "● ● ● ● ● ● ●",
];

const BANNER: [&str; 7] = [
    "●●●●   ●●●  ●●●●●   ●●●●   ●●●  ●   ●",
    "●   ● ●   ●   ●     ●   ● ●   ● ●   ●",
    "●   ● ●   ●   ●     ●   ● ●   ●  ● ●",
    "●   ● ●   ●   ●     ●●●●  ●   ●   ●",
    "●   ● ●   ●   ●     ●   ● ●   ●  ● ●",
    "●   ● ●   ●   ●     ●   ● ●   ● ●   ●",
    "●●●●   ●●●    ●     ●●●●   ●●●  ●   ●",
];

/// « dotbox » dans la police Standard de figlet.
const FIGLET: [&str; 5] = [
    r"     _       _   _",
    r"  __| | ___ | |_| |__   _____  __",
    r" / _` |/ _ \| __| '_ \ / _ \ \/ /",
    r"| (_| | (_) | |_| |_) | (_) >  <",
    r" \__,_|\___/ \__|_.__/ \___/_/\_\",
];

const WIDTH: usize = 10;

fn paint(level: Level, text: &str) -> String {
    match level {
        Level::Ok => text.to_string(),
        Level::Warn => style(text).yellow().to_string(),
        Level::Error => style(text).red().to_string(),
    }
}

fn symbol(level: Level, ascii: bool) -> String {
    match (level, ascii) {
        (Level::Ok, false) => style("◇").green().to_string(),
        (Level::Warn, false) => style("▲").yellow().to_string(),
        (Level::Error, false) => style("■").red().to_string(),
        (Level::Ok, true) => style("+").green().to_string(),
        (Level::Warn, true) => style("!").yellow().to_string(),
        (Level::Error, true) => style("!").red().to_string(),
    }
}

/// Le texte d'une ligne après son nom: le texte, le gris, puis « : la commande ».
fn body(l: &Line) -> String {
    let hint = if l.hint.is_empty() {
        String::new()
    } else {
        format!(": {}", l.hint)
    };
    format!(
        "{}{}{}",
        paint(l.level, &l.text),
        style(&l.extra).dim(),
        paint(l.level, &hint)
    )
}

/// Une ligne avec son symbole: « ◇  System    … », ou « ▲  2 apps behind: … » pour une alerte.
fn marked(l: &Line, ascii: bool) -> String {
    if l.name.is_empty() {
        format!("{}  {}", symbol(l.level, ascii), body(l))
    } else {
        format!("{}  {:<WIDTH$}{}", symbol(l.level, ascii), l.name, body(l))
    }
}

/// La ligne de fastfetch, sans symbole devant les lignes qui vont bien.
fn bare(l: &Line) -> String {
    let mark = match l.level {
        Level::Ok => String::new(),
        lv => format!("{} ", symbol(lv, false)),
    };
    if l.name.is_empty() {
        format!("{mark}{}", body(l))
    } else {
        format!("{:<WIDTH$}{mark}{}", l.name, body(l))
    }
}

fn footer(m: &Motd) -> String {
    let mut f = "dotbox status · dotbox --help".to_string();
    if !m.domain.is_empty() {
        f.push_str(&format!(" · https://dotbox.{}", m.domain));
    }
    f
}

/// Rien que de l'ASCII: les séparateurs et les flèches remplacés, le reste en « ? ».
pub fn ascii_only(text: &str) -> String {
    text.replace('·', "-")
        .replace('→', "->")
        .chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .collect()
}

pub fn render(m: &Motd, style_name: &str) -> Vec<String> {
    let bar = |c: &str| style(c).dim().to_string();
    let title = format!("{} · {}", m.version, m.domain);
    let title = title.trim_end_matches(" · ").to_string();
    let mut out = Vec::new();
    match style_name {
        "fastfetch" => {
            let mut right = vec![format!("{} {title}", style("Dot Box").bold())];
            right.extend(m.lines.iter().map(bare));
            right.push(style(footer(m)).dim().to_string());
            for i in 0..right.len().max(PICTO.len()) {
                let left = PICTO
                    .get(i)
                    .map(|p| style(*p).green().to_string())
                    .unwrap_or_else(|| " ".repeat(13));
                let r = right.get(i).cloned().unwrap_or_default();
                out.push(format!("{left}   {r}").trim_end().to_string());
            }
        }
        "banner" => {
            out.extend(BANNER.iter().map(|l| style(*l).green().to_string()));
            out.push(title);
            out.push(String::new());
            out.extend(m.lines.iter().map(|l| marked(l, false)));
            out.push(String::new());
            out.push(style(footer(m)).dim().to_string());
        }
        "ascii" => {
            out.extend(FIGLET.iter().map(|l| style(*l).green().to_string()));
            out.push(ascii_only(&title));
            out.push(String::new());
            for l in &m.lines {
                let l = Line {
                    text: ascii_only(&l.text),
                    extra: ascii_only(&l.extra),
                    hint: ascii_only(&l.hint),
                    ..l.clone()
                };
                out.push(marked(&l, true));
            }
            out.push(String::new());
            out.push(style(ascii_only(&footer(m))).dim().to_string());
        }
        _ => {
            out.push(format!("{}  {} {title}", bar("┌"), style("Dot Box").bold()));
            out.push(bar("│"));
            out.extend(m.lines.iter().map(|l| marked(l, false)));
            out.push(bar("│"));
            out.push(format!("{}  {}", bar("└"), style(footer(m)).dim()));
        }
    }
    out
}

/// Imprime le message. pam_motd n'est pas un terminal: --login force les couleurs.
pub fn print(m: &Motd, style_name: &str) {
    for l in render(m, style_name) {
        crate::clack::put(&l);
    }
}

// ---------- ce que init et sync posent (D86): notre script, sans la ligne du noyau ni la licence

fn executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn set_mode(path: &str, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("cannot change the rights of {path}: {e}"))
}

/// Les deux unités du doctor de nuit (D89): 05:00, après la sauvegarde de 04:00.
pub fn units() -> Vec<(&'static str, String)> {
    vec![
        (
            SERVICE,
            "# Écrit par dotbox (init, sync): doctor chaque nuit, le catalogue relu et le cache du MOTD (D89).
[Unit]
Description=Dot Box nightly doctor, catalog and MOTD cache
After=dotbox.service docker.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/dotbox motd --refresh --quiet
"
            .to_string(),
        ),
        (
            TIMER,
            "# Écrit par dotbox (init, sync): doctor chaque nuit, après la sauvegarde de 04:00 (D89).
[Unit]
Description=Dot Box nightly doctor

[Timer]
OnCalendar=*-*-* 05:00:00
# pas tous les serveurs sur GitHub à la même seconde
RandomizedDelaySec=15min
Persistent=true

[Install]
WantedBy=timers.target
"
            .to_string(),
        ),
    ]
}

/// Ce qui manque ou dépasse, pour dotbox diff: (ce qui ne va pas, ce que sync en fera).
pub fn gaps(s: &Settings) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for (path, text) in units() {
        match std::fs::read_to_string(path) {
            Err(_) => out.push((format!("{path} missing"), "written again")),
            Ok(now) if now != text => out.push((format!("{path} out of date"), "written again")),
            _ => {}
        }
    }
    if out.is_empty()
        && system::run("systemctl", &["is-active", "dotbox-doctor.timer"]).as_deref()
            != Ok("active")
    {
        out.push((
            "dotbox-doctor.timer inactive, wanted active".into(),
            "started",
        ));
    }
    let script = std::fs::read_to_string(SCRIPT).ok();
    if s.enabled {
        match script {
            None => out.push((format!("{SCRIPT} missing (motd.enabled)"), "written again")),
            Some(t) if t != SCRIPT_TEXT || !executable(SCRIPT) => {
                out.push((format!("{SCRIPT} out of date"), "written again"))
            }
            _ => {}
        }
        if executable(UNAME) {
            out.push((
                format!("{UNAME} still shows the kernel line"),
                "made non-executable",
            ));
        }
        if std::fs::read_to_string(ETC_MOTD).is_ok_and(|t| !t.trim().is_empty()) {
            out.push((
                format!("{ETC_MOTD} still shows Debian's license text"),
                "emptied, a copy kept",
            ));
        }
    } else {
        if script.is_some() {
            out.push((format!("{SCRIPT} there, motd.enabled is false"), "removed"));
        }
        if Path::new(UNAME_MARK).exists() {
            out.push((
                format!("{UNAME} made non-executable by Dot Box"),
                "made executable again",
            ));
        }
        if Path::new(KEPT).exists() {
            out.push((format!("{ETC_MOTD} emptied by Dot Box"), "put back"));
        }
    }
    out
}

/// Pose (ou retire) le MOTD et le doctor de nuit. Rend le résumé de l'étape.
pub fn setup(s: &Settings) -> Result<String, String> {
    let mut changed = false;
    for (path, text) in units() {
        if system::write_if_changed(path, &text, 0o644)? {
            say("write", path);
            changed = true;
        }
    }
    if changed {
        system::run("systemctl", &["daemon-reload"])?;
    }
    system::run("systemctl", &["enable", "dotbox-doctor.timer"])?;
    if changed {
        system::run("systemctl", &["restart", "dotbox-doctor.timer"])?;
        say("start", "dotbox-doctor.timer");
    } else {
        system::run("systemctl", &["start", "dotbox-doctor.timer"])?;
        say("check", "dotbox-doctor.timer");
    }
    if s.enabled {
        if system::write_if_changed(SCRIPT, SCRIPT_TEXT, 0o755)? {
            say("write", SCRIPT);
        }
        set_mode(SCRIPT, 0o755)?;
        if executable(UNAME) {
            system::write_if_changed(UNAME_MARK, "", 0o644)?;
            set_mode(UNAME, 0o644)?;
            say(
                "chmod",
                &format!("{UNAME} non-executable (the kernel line)"),
            );
        }
        let text = std::fs::read_to_string(ETC_MOTD).unwrap_or_default();
        if !text.trim().is_empty() {
            if !Path::new(KEPT).exists() {
                system::write_if_changed(KEPT, &text, 0o644)?;
                say("copy", &format!("{ETC_MOTD} → {KEPT}"));
            }
            std::fs::write(ETC_MOTD, "").map_err(|e| format!("cannot empty {ETC_MOTD}: {e}"))?;
            say("empty", &format!("{ETC_MOTD} (Debian's license text)"));
        }
        Ok(format!("MOTD {}", s.style))
    } else {
        if Path::new(SCRIPT).exists() {
            std::fs::remove_file(SCRIPT).map_err(|e| format!("cannot delete {SCRIPT}: {e}"))?;
            say("delete", SCRIPT);
        }
        if Path::new(UNAME_MARK).exists() {
            if Path::new(UNAME).exists() {
                set_mode(UNAME, 0o755)?;
                say("chmod", &format!("{UNAME} executable again"));
            }
            let _ = std::fs::remove_file(UNAME_MARK);
        }
        if let Ok(text) = std::fs::read_to_string(KEPT) {
            std::fs::write(ETC_MOTD, text).map_err(|e| format!("cannot write {ETC_MOTD}: {e}"))?;
            let _ = std::fs::remove_file(KEPT);
            say("restore", ETC_MOTD);
        }
        Ok("MOTD off (Debian's)".into())
    }
}

// ---------- la commande

/// dotbox motd en root: le travail ici, sans le service (la connexion SSH n'en dépend pas).
pub fn local(refresh_first: bool, style_name: Option<&str>, json: bool) -> Result<(), String> {
    if refresh_first {
        // un morceau qui échoue n'empêche pas d'afficher le reste
        if let Err(e) =
            crate::clack::waiting("refreshing doctor, the catalog and CrowdSec", refresh)
        {
            crate::out::error(&e);
        }
    }
    let m = collect()?;
    show(&m, style_name, json)
}

pub fn show(m: &Motd, style_name: Option<&str>, json: bool) -> Result<(), String> {
    if json {
        crate::clack::put(&serde_json::to_string_pretty(m).unwrap_or_default());
        return Ok(());
    }
    let s = style_name.unwrap_or(&m.style);
    check_style(s)?;
    print(m, s);
    Ok(())
}

/// Le script de pam_motd: jamais d'erreur qui gênerait la connexion, rien si motd.enabled est
/// false.
pub fn login() {
    if std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) {
        console::set_colors_enabled(true);
    }
    let enabled = Config::load()
        .and_then(|c| Settings::of(&c))
        .map(|s| s.enabled)
        .unwrap_or(true);
    if !enabled {
        return;
    }
    match collect() {
        Ok(m) => {
            let s = if STYLES.contains(&m.style.as_str()) {
                m.style.clone()
            } else {
                DEFAULT_STYLE.into()
            };
            print(&m, &s);
        }
        Err(e) => crate::clack::put(&format!("Dot Box: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(lines: Vec<Line>) -> Motd {
        Motd {
            version: "0.0.6".into(),
            domain: "example.com".into(),
            style: "compact".into(),
            lines,
        }
    }

    #[test]
    fn settings() {
        let cfg = Config::parse("domain: example.com\n").unwrap();
        assert_eq!(
            Settings::of(&cfg).unwrap(),
            Settings {
                enabled: true,
                style: "compact".into()
            }
        );
        let cfg = Config::parse("motd:\n  enabled: false\n  style: ascii\n").unwrap();
        let s = Settings::of(&cfg).unwrap();
        assert!(!s.enabled);
        assert_eq!(s.style, "ascii");
        let cfg = Config::parse("motd:\n  style: fancy\n").unwrap();
        assert!(Settings::of(&cfg).is_err());
        let cfg = Config::parse("motd:\n  enabled: yes please\n").unwrap();
        assert!(Settings::of(&cfg).is_err());
    }

    #[test]
    fn ages() {
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(300), "5 min ago");
        assert_eq!(ago(7300), "2 h ago");
        assert_eq!(ago(3 * 86_400 + 5), "3 days ago");
        assert_eq!(uptime(600), "up 10 min");
        assert_eq!(uptime(3 * 86_400), "up 3 days");
    }

    #[test]
    fn system_and_memory() {
        let l = system_line("Debian 13", 3 * 86_400, "0.42 0.38 0.30 1/200 999", 4);
        assert_eq!(l.text, "Debian 13 · up 3 days · load 0.42");
        assert_eq!(l.level, Level::Ok);
        let l = system_line("Debian 13", 60, "6.00 5.10 3.00 1/200 999", 4);
        assert_eq!(l.level, Level::Warn);
        assert!(l.text.ends_with("(5.10 over 5 min, 4 CPUs)"), "{}", l.text);

        let info = "MemTotal:        8000000 kB\nMemAvailable:    6000000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n";
        let l = memory_line(info);
        assert_eq!(l.text, "1.9 / 7.6 GiB");
        assert_eq!(l.level, Level::Ok);
        let info = "MemTotal: 8000000 kB\nMemAvailable: 400000 kB\nSwapTotal: 2000000 kB\nSwapFree: 1000000 kB\n";
        let l = memory_line(info);
        assert_eq!(l.level, Level::Warn);
        assert_eq!(l.text, "7.2 / 7.6 GiB · swap 1.0 GiB (95 %)");
    }

    #[test]
    fn disks() {
        let list = vec![("/".to_string(), 12), ("/data".to_string(), 93)];
        let cache = Cache {
            biggest: Some(Stamp {
                at: 1000,
                value: vec![Big {
                    mount: "/data".into(),
                    folder: "/data/media".into(),
                    bytes: 1_500_000_000_000,
                }],
            }),
            ..Cache::default()
        };
        let l = disks_line(&list, &cache, 1000 + 7200);
        assert_eq!(l.level, Level::Warn);
        assert_eq!(
            l.text,
            "/ 12 % · /data 93 % · biggest /data/media 1.5 TB, 2 h ago"
        );
        let l = disks_line(&list[..1], &Cache::default(), 0);
        assert_eq!((l.level, l.text.as_str()), (Level::Ok, "/ 12 %"));
    }

    fn c(name: &str, state: &str, health: &str, image: &str) -> status::Container {
        status::Container {
            name: name.into(),
            image: image.into(),
            state: state.into(),
            health: health.into(),
        }
    }

    #[test]
    fn apps_and_core() {
        let cfg = Config::parse(
            "apps:\n  radarr: {}\n  sonarr:\n    stopped: true\n  prowlarr: {}\n  qbittorrent: {}\n",
        )
        .unwrap();
        let list = vec![
            c("radarr", "running", "healthy", "x:1"),
            c("sonarr", "exited", "", "x:1"),
            c("prowlarr", "exited", "", "x:1"),
            c("qbittorrent", "running", "unhealthy", "x:1"),
        ];
        let l = apps_lines(&cfg, &list);
        assert_eq!(l[0].text, "2 running");
        assert_eq!(l[0].extra, " · 1 stopped (sonarr)");
        assert_eq!(l[1].text, "prowlarr exited");
        assert_eq!(l[1].hint, "dotbox start prowlarr");
        assert_eq!(l[2].text, "qbittorrent unhealthy");
        assert_eq!(l[2].level, Level::Warn);

        let core: Vec<status::Container> = status::CORE
            .iter()
            .map(|n| c(n, "running", "", "x:1"))
            .collect();
        assert!(core_lines(&core, true).is_empty());
        let l = core_lines(&core[1..], false);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text, "traefik missing");
        assert_eq!(l[1].hint, "sudo systemctl restart dotbox");
    }

    #[test]
    fn updates() {
        let cache = Cache {
            release: Some(Stamp {
                at: 0,
                value: "v0.0.9".into(),
            }),
            apt: Some(Stamp {
                at: 0,
                value: [3, 1],
            }),
            ..Cache::default()
        };
        let l = update_lines(
            &["radarr".into(), "sonarr".into()],
            &cache,
            "0.0.6",
            true,
            600,
        );
        let texts: Vec<String> = l
            .iter()
            .map(|l| format!("{}: {}", l.text, l.hint))
            .collect();
        assert_eq!(
            texts,
            [
                "2 apps behind: dotbox update --all",
                "Dot Box 0.0.9 is out, 10 min ago: sudo dotbox upgrade",
                "3 Debian updates (1 security), 10 min ago: sudo apt update && sudo apt full-upgrade",
                "reboot required: sudo reboot, when it suits you",
            ]
        );
        assert!(update_lines(&[], &Cache::default(), "0.0.6", false, 0).is_empty());
        let one = update_lines(&["radarr".into()], &Cache::default(), "0.0.9", false, 0);
        assert_eq!(one[0].hint, "dotbox update radarr");
    }

    #[test]
    fn certificates() {
        let certs = vec![
            doctor::Cert {
                names: vec!["auth.example.com".into()],
                not_after: 90 * 86_400,
                staging: false,
            },
            doctor::Cert {
                names: vec!["radarr.example.com".into()],
                not_after: 5 * 86_400 + 10,
                staging: false,
            },
        ];
        let expected = vec!["auth.example.com".into(), "radarr.example.com".into()];
        let l = cert_lines(&certs, &expected, 0);
        assert_eq!(l[0].text, "radarr.example.com: certificate ends in 5 days");
        assert!(cert_lines(&certs[..1], &expected[..1], 0).is_empty());
    }

    #[test]
    fn doctor_and_backup() {
        let d = doctor::Summary {
            date: String::new(),
            problems: 0,
            sections: 0,
        };
        let l = doctor_line(Some(&d), Some(0), 7200);
        assert_eq!(
            (l.level, l.text.as_str()),
            (Level::Ok, "nothing to report, 2 h ago")
        );
        let l = doctor_line(Some(&d), Some(0), 3 * 86_400);
        assert_eq!(l.level, Level::Warn);
        let d = doctor::Summary {
            date: String::new(),
            problems: 3,
            sections: 2,
        };
        let l = doctor_line(Some(&d), Some(0), 60);
        assert_eq!(l.text, "2 sections to look at, 1 min ago");
        assert_eq!(doctor_line(None, None, 0).text, "never run");

        let b = backup::Summary {
            started: "2026-11-22 04:00:01 CET".into(),
            ok: true,
            archives: 15,
            bytes: 120_000_000,
            error: String::new(),
            next: String::new(),
        };
        let l = backup_line(&b, Some(0), 3600);
        assert_eq!(l.text, "1 h ago · 15 archives · 120 MB");
        let bad = backup::Summary {
            ok: false,
            error: "disk full".into(),
            ..b
        };
        assert_eq!(backup_line(&bad, Some(0), 3600).level, Level::Error);
    }

    #[test]
    fn bans() {
        let json = r#"[
          {"source": {"value": "198.51.100.7"}, "decisions": [{"type": "ban"}]},
          {"source": {"value": "198.51.100.7"}, "decisions": [{"type": "ban"}]},
          {"source": {"value": "198.51.100.8"}, "decisions": []},
          {"source": {"value": "198.51.100.9"}, "decisions": [{"type": "ban"}]}
        ]"#;
        assert_eq!(parse_bans(json).unwrap(), 2);
        let l = crowdsec_line(
            &Cache {
                bans: Some(Stamp { at: 0, value: 2 }),
                ..Cache::default()
            },
            300,
        );
        assert_eq!(l.text, "2 IPs banned in 24 h, 5 min ago");
    }

    #[test]
    fn du() {
        let t = "4096\t/data/torrents\n1500000\t/data/media\n1504096\t/data\n";
        assert_eq!(
            parse_du(t, "/data"),
            Some(("/data/media".into(), 1_500_000))
        );
    }

    #[test]
    fn the_four_styles() {
        console::set_colors_enabled(false);
        let motd = m(vec![
            state("System", "Debian 13 · up 3 days · load 0.42".into()),
            Line {
                extra: " · 1 stopped (sonarr)".into(),
                ..state("Apps", "14 running".into())
            },
            alert(Level::Warn, "2 apps behind".into(), "dotbox update --all"),
        ]);
        let compact = render(&motd, "compact").join("\n");
        assert_eq!(
            compact,
            "┌  Dot Box 0.0.6 · example.com
│
◇  System    Debian 13 · up 3 days · load 0.42
◇  Apps      14 running · 1 stopped (sonarr)
▲  2 apps behind: dotbox update --all
│
└  dotbox status · dotbox --help · https://dotbox.example.com"
        );
        let ff = render(&motd, "fastfetch");
        assert_eq!(ff.len(), 7);
        assert_eq!(ff[0], "● ● ● ● ● ● ●   Dot Box 0.0.6 · example.com");
        assert_eq!(
            ff[1],
            "●           ●   System    Debian 13 · up 3 days · load 0.42"
        );
        assert_eq!(
            ff[3],
            "●   ●   ●   ●   ▲ 2 apps behind: dotbox update --all"
        );
        assert_eq!(ff[5], "●           ●");
        let banner = render(&motd, "banner");
        assert_eq!(banner[7], "0.0.6 · example.com");
        assert_eq!(banner[9], "◇  System    Debian 13 · up 3 days · load 0.42");
        let ascii = render(&motd, "ascii");
        for l in &ascii {
            assert!(l.is_ascii(), "{l}");
        }
        assert_eq!(ascii[5], "0.0.6 - example.com");
        assert_eq!(ascii[7], "+  System    Debian 13 - up 3 days - load 0.42");
        assert_eq!(ascii[9], "!  2 apps behind: dotbox update --all");
    }

    #[test]
    fn the_login_script() {
        assert!(SCRIPT_TEXT.starts_with("#!/bin/sh\n"));
        assert!(SCRIPT_TEXT.contains("timeout 5 /usr/local/bin/dotbox motd --login"));
        let u = units();
        assert!(u[1].1.contains("OnCalendar=*-*-* 05:00:00"));
        assert!(
            u[0].1
                .contains("ExecStart=/usr/local/bin/dotbox motd --refresh --quiet")
        );
    }
}
