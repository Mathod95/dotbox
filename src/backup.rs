//! `dotbox backup` (Journal, étape 16, D82, D83, D97): /opt/dotbox puis chaque app, une archive
//! par dossier, en tar, compressée en zstd (ruzstd, N14) puis chiffrée avec age pour la clé de
//! /opt/dotbox/secrets/backup.key. Rangées à plat: /backups/<app>-<AAAA-MM-JJ-HHMM>.tar.zst.age,
//! le dossier en 700 et les archives en 600, à root. Une app qui tournait est arrêtée le temps de
//! son archive, relancée et attendue Running avant la suivante. keep compte par app et par
//! destination. La planification: un timer systemd, écrit par init et par sync.

use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use crate::clack;
use crate::config::Config;
use crate::out::say;
use crate::{add, apps, catalog, github, manage, system, traefik, ui};

pub const KEY: &str = "/opt/dotbox/secrets/backup.key";
pub const RESULT: &str = "/opt/dotbox/backup.json";
pub const SUFFIX: &str = ".tar.zst.age";
pub const DEFAULT_SCHEDULE: &str = "daily 04:00";
pub const DEFAULT_KEEP: usize = 7;
pub const DEFAULT_LOCAL: &str = "/backups";
/// Toujours laissés de côté (D97), relatifs à /opt. * remplace n'importe quoi, / compris.
pub const ALWAYS_EXCLUDED: &[&str] = &["*/logs/*", "*.log"];
pub const TIMER: &str = "/etc/systemd/system/dotbox-backup.timer";
pub const SERVICE: &str = "/etc/systemd/system/dotbox-backup.service";
const OPT: &str = "/opt";
const DAYS: &[(&str, &str)] = &[
    ("monday", "Mon"),
    ("tuesday", "Tue"),
    ("wednesday", "Wed"),
    ("thursday", "Thu"),
    ("friday", "Fri"),
    ("saturday", "Sat"),
    ("sunday", "Sun"),
];

/// Une destination de backup.destinations, ou de --to.
#[derive(Debug, Clone, PartialEq)]
pub enum Dest {
    /// un dossier du serveur
    Local(String),
    /// rclone:<remote>:<chemin> ou sftp:<user>@<hôte>:<chemin>, par rclone (D83, 16.5)
    Remote(String),
}

impl Dest {
    pub fn parse(text: &str) -> Result<Dest, String> {
        if let Some(rest) = text.strip_prefix("sftp:") {
            Sftp::parse(rest)?;
            return Ok(Dest::Remote(text.to_string()));
        }
        if let Some(rest) = text.strip_prefix("rclone:") {
            let (remote, _) = rest.split_once(':').ok_or_else(|| {
                format!("{text}: rclone:<remote>:<path>, the remote of {RCLONE_CONF}")
            })?;
            if remote.is_empty()
                || !remote
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            {
                return Err(format!(
                    "{text}: rclone:<remote>:<path>, the remote of {RCLONE_CONF}"
                ));
            }
            return Ok(Dest::Remote(text.to_string()));
        }
        if !text.starts_with('/') || text.split('/').any(|p| p == "..") {
            return Err(format!(
                "destination {text:?}: a folder of the server (/backups), rclone:<remote>:<path> or sftp:<user>@<host>:<path>"
            ));
        }
        if text == "/" || text == OPT || text.starts_with("/opt/") || text.starts_with("/data") {
            return Err(format!(
                "destination {text}: outside /, /opt and /data (they are what is backed up, or the media)"
            ));
        }
        Ok(Dest::Local(text.trim_end_matches('/').to_string()))
    }
}

/// La section backup: de dotbox.yml, ou ses valeurs par défaut (D97: non écrites dans le fichier).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub schedule: String,
    pub keep: usize,
    pub exclude: Vec<String>,
    pub destinations: Vec<Dest>,
}

impl Settings {
    pub fn of(cfg: &Config) -> Result<Settings, String> {
        let mut s = Settings {
            schedule: DEFAULT_SCHEDULE.into(),
            keep: DEFAULT_KEEP,
            exclude: Vec::new(),
            destinations: vec![Dest::Local(DEFAULT_LOCAL.into())],
        };
        let Some(m) = cfg.section("backup") else {
            return Ok(s);
        };
        if let Some(v) = m.get("schedule") {
            let text = v
                .as_str()
                .ok_or("backup.schedule: a text, like daily 04:00")?;
            on_calendar(text)?;
            s.schedule = text.to_string();
        }
        if let Some(v) = m.get("keep") {
            s.keep = keep_of(v)?;
        }
        if let Some(v) = m.get("exclude") {
            let list = v.as_sequence().ok_or("backup.exclude: a list")?;
            s.exclude = list
                .iter()
                .map(|e| {
                    e.as_str()
                        .map(String::from)
                        .ok_or_else(|| "backup.exclude: a list of texts".to_string())
                })
                .collect::<Result<_, _>>()?;
        }
        if let Some(v) = m.get("destinations") {
            let list = v.as_sequence().ok_or("backup.destinations: a list")?;
            s.destinations = list.iter().map(dest_of).collect::<Result<_, _>>()?;
            if s.destinations.is_empty() {
                return Err("backup.destinations: at least one".into());
            }
        }
        Ok(s)
    }
}

fn keep_of(v: &Value) -> Result<usize, String> {
    let n = match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    };
    match n {
        Some(n) if (1..=365).contains(&n) => Ok(n as usize),
        _ => Err("backup.keep: a number from 1 to 365".into()),
    }
}

/// « - local: /backups », « - rclone: b2:dotbox », « - sftp: backup@nas:/volume1 ».
fn dest_of(v: &Value) -> Result<Dest, String> {
    let bad = || {
        "backup.destinations: - local: /backups, - rclone: <remote>:<path> or - sftp: <user>@<host>:<path>".to_string()
    };
    let m = v.as_mapping().filter(|m| m.len() == 1).ok_or_else(bad)?;
    let (k, v) = m.iter().next().ok_or_else(bad)?;
    let (k, v) = (k.as_str().ok_or_else(bad)?, v.as_str().ok_or_else(bad)?);
    match k {
        "local" => Dest::parse(v),
        "rclone" | "sftp" => Dest::parse(&format!("{k}:{v}")),
        _ => Err(bad()),
    }
}

/// backup.schedule en OnCalendar de systemd: None pour off.
pub fn on_calendar(schedule: &str) -> Result<Option<String>, String> {
    let bad = || {
        format!(
            "backup.schedule {schedule:?}: off, hourly, daily HH:MM, or a day and HH:MM (sunday 05:00)"
        )
    };
    let words: Vec<&str> = schedule.split_whitespace().collect();
    match words.as_slice() {
        ["off"] => Ok(None),
        ["hourly"] => Ok(Some("hourly".into())),
        [day, time] => {
            let (h, m) = time.split_once(':').ok_or_else(bad)?;
            let ok = |t: &str, max: u32| t.len() == 2 && t.parse::<u32>().is_ok_and(|n| n <= max);
            if !ok(h, 23) || !ok(m, 59) {
                return Err(bad());
            }
            if *day == "daily" {
                return Ok(Some(format!("*-*-* {h}:{m}:00")));
            }
            let short = DAYS
                .iter()
                .find(|(d, _)| d == day)
                .map(|(_, s)| *s)
                .ok_or_else(bad)?;
            Ok(Some(format!("{short} *-*-* {h}:{m}:00")))
        }
        _ => Err(bad()),
    }
}

/// * remplace n'importe quelle suite de caractères (/ compris), ? un seul caractère.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Un chemin relatif à /opt (radarr/config/logs/x.txt) est-il laissé de côté? Un motif sans *
/// (jellyfin/cache) écarte aussi tout ce qu'il contient.
pub fn excluded(patterns: &[String], rel: &str) -> bool {
    patterns
        .iter()
        .any(|p| glob(p, rel) || glob(&format!("{p}/*"), rel))
}

/// Ce qu'une archive laisse de côté: les logs, backup.exclude, et le backup_exclude du manifest.
fn patterns(settings: &Settings, name: &str) -> Vec<String> {
    let mut all: Vec<String> = ALWAYS_EXCLUDED.iter().map(|p| p.to_string()).collect();
    all.extend(settings.exclude.iter().cloned());
    if name != "dotbox" {
        let dir = add::manifest_dir(Path::new(github::DIR), name);
        match catalog::load(&dir, name) {
            Ok(m) => all.extend(m.backup_exclude.iter().map(|e| format!("{name}/{e}"))),
            Err(e) => say("check", &format!("{name}: no manifest exclusions ({e})")),
        }
    }
    all
}

// ---------- la clé

/// Crée backup.key s'il manque (age, X25519), lisible par root seulement. Rend true s'il vient
/// d'être créé. Le format est celui d'age-keygen: age -d -i backup.key l'ouvre aussi (D97).
pub fn ensure_key() -> Result<bool, String> {
    use age::secrecy::ExposeSecret;
    if Path::new(KEY).exists() {
        return Ok(false);
    }
    let dir = Path::new(KEY).parent().unwrap_or(Path::new("/"));
    std::os::unix::fs::DirBuilderExt::mode(fs::DirBuilder::new().recursive(true), 0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let id = age::x25519::Identity::generate();
    let now = system::run("date", &["+%Y-%m-%dT%H:%M:%S%:z"]).unwrap_or_default();
    let text = format!(
        "# created: {now}\n# public key: {}\n# Dot Box backup key: keep a copy off the server, the archives cannot be read without it.\n{}\n",
        id.to_public(),
        id.to_string().expose_secret()
    );
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(KEY)
        .map_err(|e| format!("cannot create {KEY}: {e}"))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("cannot write {KEY}: {e}"))?;
    Ok(true)
}

/// La clé d'un fichier au format d'age-keygen (des commentaires, puis AGE-SECRET-KEY-1...).
pub fn parse_key(text: &str) -> Result<age::x25519::Identity, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("AGE-SECRET-KEY-"))
        .ok_or("no AGE-SECRET-KEY-1... line: not an age key")?;
    line.parse::<age::x25519::Identity>()
        .map_err(|e| format!("not a valid age key: {e}"))
}

pub fn load_key(path: &str) -> Result<age::x25519::Identity, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    parse_key(&text).map_err(|e| format!("{path}: {e}"))
}

// ---------- l'archive

/// Ajoute /opt/<rel> à l'archive, et tout ce qu'il contient, sauf ce qui est laissé de côté.
/// Les liens symboliques restent des liens, les sockets et les tubes sont ignorés.
fn add_tree<W: std::io::Write>(
    b: &mut tar::Builder<W>,
    root: &Path,
    rel: &str,
    patterns: &[String],
) -> Result<u64, String> {
    if excluded(patterns, rel) {
        return Ok(0);
    }
    let full = root.join(rel);
    let meta = match fs::symlink_metadata(&full) {
        Ok(m) => m,
        // disparu pendant la sauvegarde (un fichier temporaire): rien à garder
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("cannot read {}: {e}", full.display())),
    };
    let ft = meta.file_type();
    if !(ft.is_dir() || ft.is_file() || ft.is_symlink()) {
        return Ok(0);
    }
    b.append_path_with_name(&full, rel)
        .map_err(|e| format!("cannot archive {}: {e}", full.display()))?;
    let mut count = 1;
    if ft.is_dir() {
        let mut names: Vec<String> = fs::read_dir(&full)
            .map_err(|e| format!("cannot read {}: {e}", full.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            count += add_tree(b, root, &format!("{rel}/{n}"), patterns)?;
        }
    }
    Ok(count)
}

/// Le tar de /opt/<name>, écrit dans `w`. Rend le nombre d'entrées.
pub fn write_tar<W: std::io::Write>(
    w: W,
    root: &Path,
    name: &str,
    patterns: &[String],
) -> Result<u64, String> {
    let mut b = tar::Builder::new(w);
    b.follow_symlinks(false);
    b.mode(tar::HeaderMode::Complete);
    let n = add_tree(&mut b, root, name, patterns)?;
    let mut w = b
        .into_inner()
        .map_err(|e| format!("cannot end the archive: {e}"))?;
    w.flush()
        .map_err(|e| format!("cannot end the archive: {e}"))?;
    Ok(n)
}

/// tar, puis zstd (ruzstd, niveau Fastest, N14), puis age, dans `target`. Le tar se fait dans un
/// fil à part et passe par un tuyau: aucune copie en clair sur le disque.
pub fn seal(
    root: &Path,
    name: &str,
    patterns: &[String],
    recipient: &age::x25519::Recipient,
    target: &Path,
) -> Result<u64, String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(target)
        .map_err(|e| format!("cannot create {}: {e}", target.display()))?;
    let (entries, w) = seal_to(
        root,
        name,
        patterns,
        recipient,
        BufWriter::new(file),
        &target.display().to_string(),
    )
    .map_err(|e| {
        if e.ends_with("was cut") {
            format!(
                "cannot write {}: is the disk full? (df -h {})",
                target.display(),
                target.parent().unwrap_or(Path::new("/")).display()
            )
        } else {
            e
        }
    })?;
    w.into_inner()
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?
        .sync_all()
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok(entries)
}

/// Le cœur de seal: l'archive chiffrée écrite dans `w`, un fichier ou le tuyau vers rclone
/// (--stream). ruzstd ne rend pas d'erreur: une écriture ratée (disque plein, envoi coupé) finit
/// en panique, rattrapée ici. Rend le nombre d'entrées et `w`, vidé.
pub fn seal_to<W: Write>(
    root: &Path,
    name: &str,
    patterns: &[String],
    recipient: &age::x25519::Recipient,
    w: W,
    what: &str,
) -> Result<(u64, W), String> {
    let enc = age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
        .map_err(|e| format!("age: {e}"))?;
    let mut out = enc.wrap_output(w).map_err(|e| format!("age: {e}"))?;
    let (reader, writer) = std::io::pipe().map_err(|e| format!("cannot open a pipe: {e}"))?;
    let (root, name, patterns) = (root.to_path_buf(), name.to_string(), patterns.to_vec());
    let tarring = std::thread::spawn(move || write_tar(writer, &root, &name, &patterns));
    let compressed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ruzstd::encoding::compress(
            BufReader::new(reader),
            &mut out,
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }));
    let entries = tarring
        .join()
        .map_err(|_| "the archive stopped halfway".to_string())?;
    compressed.map_err(|_| format!("{what}: the writing was cut"))?;
    let entries = entries?;
    let mut w = out
        .finish()
        .map_err(|e| format!("cannot write {what}: {e}"))?;
    w.flush().map_err(|e| format!("cannot write {what}: {e}"))?;
    Ok((entries, w))
}

/// Le chemin inverse: age, zstd, tar. Rend un lecteur du tar.
pub fn open(
    path: &Path,
    identity: &age::x25519::Identity,
) -> Result<tar::Archive<impl std::io::Read>, String> {
    let file = fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let dec = age::Decryptor::new(BufReader::new(file))
        .map_err(|e| format!("{}: not an age archive ({e})", path.display()))?;
    let plain = dec
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| format!("{}: this key does not open it ({e})", path.display()))?;
    let unzipped = ruzstd::decoding::StreamingDecoder::new(BufReader::new(plain))
        .map_err(|e| format!("{}: not zstd inside ({e})", path.display()))?;
    Ok(tar::Archive::new(unzipped))
}

// ---------- les archives d'une destination

/// Une archive d'une destination locale: <app>-<AAAA-MM-JJ-HHMM>.tar.zst.age.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Archive {
    pub app: String,
    pub stamp: String,
    pub path: String,
    pub bytes: u64,
}

fn is_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 15
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 10 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// Le nom d'une archive: (app, date), ou None pour un autre fichier.
pub fn parse_name(file: &str) -> Option<(String, String)> {
    let base = file.strip_suffix(SUFFIX)?;
    if base.len() < 17 {
        return None;
    }
    let (app, stamp) = base.split_at(base.len() - 15);
    let app = app.strip_suffix('-')?;
    (is_stamp(stamp) && (app == "dotbox" || catalog::valid_name(app)))
        .then(|| (app.to_string(), stamp.to_string()))
}

/// Les archives d'un dossier, les plus récentes d'abord.
pub fn list(dir: &str) -> Vec<Archive> {
    let mut all: Vec<Archive> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let file = e.file_name().to_string_lossy().into_owned();
                    let (app, stamp) = parse_name(&file)?;
                    let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                    Some(Archive {
                        app,
                        stamp,
                        path: format!("{dir}/{file}"),
                        bytes,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    all.sort_by(|a, b| b.stamp.cmp(&a.stamp).then(a.app.cmp(&b.app)));
    all
}

/// keep par app et par destination (D97): les plus anciennes sont effacées. Rend leurs chemins.
pub fn prune(dir: &str, app: &str, keep: usize) -> Result<Vec<String>, String> {
    let mut gone = Vec::new();
    for a in list(dir).into_iter().filter(|a| a.app == app).skip(keep) {
        fs::remove_file(&a.path).map_err(|e| format!("cannot delete {}: {e}", a.path))?;
        gone.push(a.path);
    }
    Ok(gone)
}

// ---------- dotbox backup

/// Ce qu'a fait la dernière sauvegarde complète (/opt/dotbox/backup.json): dotbox status et doctor.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Report {
    pub started: String,
    pub ended: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub destinations: Vec<String>,
    pub keep: usize,
    pub archives: Vec<Done>,
    #[serde(default)]
    pub key_created: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Done {
    pub name: String,
    pub file: String,
    pub bytes: u64,
    /// l'état de l'app après son archive (vide pour dotbox)
    #[serde(default)]
    pub state: String,
}

/// « 2026-11-22 04:00:01 CET » devient « 2026-11-22 04:00 ».
pub fn short_date(date: &str) -> String {
    date.chars().take(16).collect()
}

/// Le résumé pour dotbox status: la dernière sauvegarde et la prochaine.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Summary {
    pub started: String,
    pub ok: bool,
    pub archives: usize,
    pub bytes: u64,
    #[serde(default)]
    pub error: String,
    /// vide: pas de timer (backup.schedule off)
    #[serde(default)]
    pub next: String,
}

pub fn summary() -> Option<Summary> {
    let r = last()?;
    Some(Summary {
        started: r.started.clone(),
        ok: r.ok,
        archives: r.archives.len(),
        bytes: r.archives.iter().map(|a| a.bytes).sum(),
        error: r.error.lines().next().unwrap_or_default().to_string(),
        next: next_run().unwrap_or_default(),
    })
}

pub fn last() -> Option<Report> {
    serde_json::from_str(&fs::read_to_string(RESULT).ok()?).ok()
}

/// « 1 archive », « 14 archives ».
pub fn count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "archive" } else { "archives" })
}

pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

fn now() -> String {
    system::run("date", &["+%Y-%m-%d %H:%M:%S %Z"]).unwrap_or_default()
}

/// Une destination locale prête: le dossier créé, en 700, à root.
fn prepare_dir(dir: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot set the permissions of {dir}: {e}"))
}

/// Une app (ou dotbox): arrêtée si elle tournait, archivée, relancée et attendue. L'archive va
/// dans la première destination, puis est copiée dans les autres. keep fait le ménage.
fn one(
    settings: &Settings,
    name: &str,
    dirs: &[String],
    stamp: &str,
    recipient: &age::x25519::Recipient,
) -> Result<Done, String> {
    let folder = format!("{OPT}/{name}");
    if !Path::new(&folder).is_dir() {
        return Err(format!("{folder} does not exist"));
    }
    let pats = patterns(settings, name);
    let file = format!("{name}-{stamp}{SUFFIX}");
    let first = PathBuf::from(&dirs[0]).join(&file);
    let partial = PathBuf::from(&dirs[0]).join(format!(".{file}.partial"));
    let was_running = name != "dotbox" && apps::running(&traefik::container_state(name));
    if was_running {
        say("stop", name);
        apps::compose(name, &["stop"])?;
    }
    let sealed = seal(Path::new(OPT), name, &pats, recipient, &partial);
    // relancée même si l'archive a échoué: l'app ne doit pas rester arrêtée
    let state = if was_running {
        say("start", name);
        apps::compose(name, &["start"])?;
        apps::wait_ready(name, manage::has_health(name), Duration::from_secs(180))?
    } else if name == "dotbox" {
        String::new()
    } else {
        "stopped".to_string()
    };
    let entries = match sealed {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&partial);
            return Err(e);
        }
    };
    fs::rename(&partial, &first)
        .map_err(|e| format!("cannot rename {}: {e}", partial.display()))?;
    let bytes = fs::metadata(&first).map(|m| m.len()).unwrap_or(0);
    say(
        "write",
        &format!("{} ({entries} entries, {})", first.display(), human(bytes)),
    );
    for d in &dirs[1..] {
        let copy = PathBuf::from(d).join(&file);
        fs::copy(&first, &copy).map_err(|e| format!("cannot copy to {}: {e}", copy.display()))?;
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot set the permissions of {}: {e}", copy.display()))?;
        say("copy", &copy.display().to_string());
    }
    for d in dirs {
        for gone in prune(d, name, settings.keep)? {
            say("delete", &format!("{gone} (keep {})", settings.keep));
        }
    }
    let shown = if state.is_empty() {
        human(bytes)
    } else {
        format!("{}, {state}", human(bytes))
    };
    ui::done(&shown);
    Ok(Done {
        name: name.to_string(),
        file: first.display().to_string(),
        bytes,
        state,
    })
}

/// Ce qui part vers rclone (--stream): chaque octet écrit dans chaque envoi, et leur compte.
struct Tee {
    outs: Vec<std::process::ChildStdin>,
    bytes: u64,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for o in &mut self.outs {
            o.write_all(buf)?;
        }
        self.bytes += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        for o in &mut self.outs {
            o.flush()?;
        }
        Ok(())
    }
}

/// « :sftp,...:/backups » et « .radarr-....partial » donnent « :sftp,...:/backups/.radarr-... ».
fn remote_file(target: &str, file: &str) -> String {
    if target.ends_with('/') || target.ends_with(':') {
        format!("{target}{file}")
    } else {
        format!("{target}/{file}")
    }
}

/// --stream (D157): l'app arrêtée, son archive envoyée tout droit à chaque destination distante
/// (rclone rcat, sous un nom provisoire, renommée une fois arrivée), puis relancée. Rien n'est
/// écrit sur le disque du serveur. L'app reste arrêtée le temps de l'envoi.
fn one_stream(
    settings: &Settings,
    name: &str,
    remotes: &[String],
    stamp: &str,
    recipient: &age::x25519::Recipient,
) -> Result<Done, String> {
    use std::process::Stdio;
    let folder = format!("{OPT}/{name}");
    if !Path::new(&folder).is_dir() {
        return Err(format!("{folder} does not exist"));
    }
    let pats = patterns(settings, name);
    let file = format!("{name}-{stamp}{SUFFIX}");
    let partial = format!(".{file}.partial");
    let mut sends = Vec::new();
    let mut outs = Vec::new();
    for (i, r) in remotes.iter().enumerate() {
        prepare_sftp(r)?;
        let (target, files) = rclone_target(r)?;
        let args = rclone_args(
            &format!("dotbox-rclone-{i}"),
            None,
            &files,
            &["rcat".into(), remote_file(&target, &partial)],
            true,
        )?;
        crate::log::write(&format!("$ docker {}", args.join(" ")));
        let mut child = system::command("docker")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run docker: {e}"))?;
        outs.extend(child.stdin.take());
        sends.push((r, target, files, child));
    }
    let was_running = name != "dotbox" && apps::running(&traefik::container_state(name));
    if was_running {
        say("stop", name);
        apps::compose(name, &["stop"])?;
    }
    // l'entrée de rclone se ferme avec le Tee, à la fin de seal_to: rclone finit son envoi
    let sealed = seal_to(
        Path::new(OPT),
        name,
        &pats,
        recipient,
        Tee { outs, bytes: 0 },
        &partial,
    )
    .map(|(n, tee)| (n, tee.bytes));
    let mut failed: Vec<String> = Vec::new();
    for (r, _, _, child) in sends.iter_mut() {
        let out = child.wait().map_err(|e| format!("rclone for {r}: {e}"));
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        crate::log::write(&err);
        match out {
            Ok(st) if st.success() => {}
            Ok(st) => failed.push(explain(
                r,
                format!(
                    "rclone rcat to {r} failed (exit {}): {}",
                    st.code().unwrap_or(-1),
                    err.lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                ),
            )),
            Err(e) => failed.push(e),
        }
    }
    // relancée même si l'envoi a échoué: l'app ne doit pas rester arrêtée
    let state = if was_running {
        say("start", name);
        apps::compose(name, &["start"])?;
        apps::wait_ready(name, manage::has_health(name), Duration::from_secs(180))?
    } else if name == "dotbox" {
        String::new()
    } else {
        "stopped".to_string()
    };
    // une archive coupée parce que rclone s'est arrêté: l'erreur de rclone suffit
    if let (Err(e), true) = (&sealed, failed.is_empty()) {
        failed.push(e.clone());
    }
    if !failed.is_empty() {
        for (_, target, files, _) in &sends {
            let _ = rclone_in(
                None,
                files,
                &["deletefile".into(), remote_file(target, &partial)],
            );
        }
        return Err(failed.join("\n"));
    }
    let (entries, bytes) = sealed?;
    for (r, target, files, _) in &sends {
        rclone_in(
            None,
            files,
            &[
                "moveto".into(),
                remote_file(target, &partial),
                remote_file(target, &file),
            ],
        )
        .map_err(|e| explain(r, e))?;
        say(
            "send",
            &format!("{r}/{file} ({entries} entries, {})", human(bytes)),
        );
        prune_remote(r, None, &[name.to_string()], settings.keep)?;
    }
    let shown = if state.is_empty() {
        human(bytes)
    } else {
        format!("{}, {state}", human(bytes))
    };
    ui::done(&shown);
    Ok(Done {
        name: name.to_string(),
        file: format!("{}/{file}", remotes[0]),
        bytes,
        state,
    })
}

/// dotbox backup --local, ou la tâche du service (D97: une tâche à la fois).
pub fn run(app: Option<&str>, to: Option<&str>, stream: bool, verbose: bool) -> Result<(), String> {
    let cfg = Config::load()?;
    let settings = Settings::of(&cfg)?;
    let installed = cfg.app_names();
    let names: Vec<String> = match app {
        Some(a) if a == "dotbox" || installed.iter().any(|i| i == a) => vec![a.to_string()],
        Some(a) => return Err(format!("{a} is not installed: dotbox list shows what is")),
        None => std::iter::once("dotbox".to_string())
            .chain(installed.iter().cloned())
            .collect(),
    };
    let wanted = match to {
        Some(t) => vec![Dest::parse(t)?],
        None => settings.destinations.clone(),
    };
    // les archives se font toujours d'abord ici (D99, 6): les dossiers voulus, ou ceux de
    // backup.destinations quand --to vise une destination distante, sinon /backups
    let local = |list: &[Dest]| -> Vec<String> {
        list.iter()
            .filter_map(|d| match d {
                Dest::Local(p) => Some(p.clone()),
                Dest::Remote(_) => None,
            })
            .collect()
    };
    let remotes: Vec<String> = wanted
        .iter()
        .filter_map(|d| match d {
            Dest::Remote(r) => Some(r.clone()),
            Dest::Local(_) => None,
        })
        .collect();
    // --stream (D157): rien sur le disque du serveur, chaque archive part tout droit vers les
    // destinations distantes
    let mut dirs = if stream { Vec::new() } else { local(&wanted) };
    if stream && remotes.is_empty() {
        return Err(
            "--stream sends to a remote destination: --to sftp:<user>@<host>:<path> or rclone:<remote>:<path>, or one in backup.destinations"
                .into(),
        );
    }
    if !stream && dirs.is_empty() {
        dirs = local(&settings.destinations);
    }
    if !stream && dirs.is_empty() {
        dirs.push(DEFAULT_LOCAL.to_string());
    }
    let what = [
        app.map(|a| format!("--app {a}")),
        to.map(|t| format!("--to {t}")),
    ]
    .into_iter()
    .flatten()
    .chain(stream.then(|| "--stream".to_string()))
    .collect::<Vec<_>>()
    .join(" ");
    let sends = if stream { 0 } else { remotes.len() };
    apps::begin("backup", &what, 1 + names.len() + sends, verbose)?;
    let mut report = Report {
        started: now(),
        destinations: dirs.iter().chain(remotes.iter()).cloned().collect(),
        keep: settings.keep,
        ..Report::default()
    };
    let result = (|| {
        ui::step("Key", "the lock of the archives (age)");
        let created = ensure_key()?;
        say(if created { "create" } else { "check" }, KEY);
        let recipient = load_key(KEY)?.to_public();
        for d in &dirs {
            prepare_dir(d)?;
            say("check", &format!("{d} (700, root)"));
        }
        // --stream: chaque destination doit accepter Dot Box avant qu'une app soit arrêtée
        if stream {
            for r in &remotes {
                remote_list(r, None)?;
                say("check", r);
            }
        }
        report.key_created = created;
        ui::done(if created {
            "backup.key created: keep a copy off the server"
        } else {
            "backup.key"
        });
        let stamp = system::run("date", &["+%Y-%m-%d-%H%M"])?;
        for name in &names {
            ui::step(
                &apps::title(name),
                if name == "dotbox" {
                    "config, manifests, secrets"
                } else {
                    "stop, archive, start"
                },
            );
            let done = if stream {
                one_stream(&settings, name, &remotes, &stamp, &recipient)?
            } else {
                one(&settings, name, &dirs, &stamp, &recipient)?
            };
            report.archives.push(done);
        }
        if stream {
            return Ok(());
        }
        // puis en une fois vers chaque destination distante, keep y fait aussi le ménage
        for r in &remotes {
            ui::step(&remote_title(r), "send the archives (rclone)");
            let sent = send(r, &dirs[0], &stamp, &names, settings.keep)?;
            ui::done(&sent);
        }
        Ok(())
    })();
    report.ended = now();
    report.ok = result.is_ok();
    report.error = result.as_ref().err().cloned().unwrap_or_default();
    // seule une sauvegarde complète compte pour status et doctor: une app seule (--app) ne
    // remplace pas son résultat
    if app.is_none() {
        if let Ok(text) = serde_json::to_string_pretty(&report) {
            let _ = system::write_if_changed(RESULT, &format!("{text}\n"), 0o600);
        }
    }
    apps::finish(result)?;
    if ui::events() {
        ui::result(serde_json::to_value(&report).unwrap_or_default());
    } else {
        print_report(&report);
    }
    Ok(())
}

/// Le résumé, après les étapes.
pub fn print_report(r: &Report) {
    let total: u64 = r.archives.iter().map(|a| a.bytes).sum();
    clack::put("");
    clack::put(&format!(
        "  {}, {}, in {}, keep {}",
        count(r.archives.len()),
        human(total),
        r.destinations.join(" and "),
        r.keep
    ));
    if r.key_created {
        clack::put(&format!(
            "  {} {}",
            console::style("→").cyan(),
            "keep a copy of /opt/dotbox/secrets/backup.key off the server (a password manager): without it, the archives cannot be read"
        ));
        clack::put(&format!(
            "    {}",
            console::style("sudo cat /opt/dotbox/secrets/backup.key").dim()
        ));
    }
}

// ---------- les destinations distantes (16.5, D83, D99)

pub const SSH_KEY: &str = "/opt/dotbox/secrets/backup_ssh";
pub const KNOWN_HOSTS: &str = "/opt/dotbox/secrets/backup_known_hosts";
pub const RCLONE_CONF: &str = "/opt/dotbox/secrets/rclone.conf";

/// sftp:<user>@<hôte>:<chemin>, port 22.
#[derive(Debug, Clone, PartialEq)]
pub struct Sftp {
    pub user: String,
    pub host: String,
    pub path: String,
}

impl Sftp {
    pub fn parse(text: &str) -> Result<Sftp, String> {
        let bad = || {
            format!(
                "sftp:{text}: sftp:<user>@<host>:<path>, like sftp:dotbox-backup@nas.example.com:/backups"
            )
        };
        let (user, rest) = text.split_once('@').ok_or_else(bad)?;
        let (host, path) = rest.split_once(':').ok_or_else(bad)?;
        let ok = |t: &str, extra: &str| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
        };
        if !ok(user, "-_.")
            || !ok(host, "-.")
            || !path.starts_with('/')
            || path.contains("..")
            || !ok(path, "-_./")
        {
            return Err(bad());
        }
        Ok(Sftp {
            user: user.into(),
            host: host.into(),
            path: path.trim_end_matches('/').to_string(),
        })
    }
}

/// « sftp:dotbox-backup@nas:/backups » devient « Sftp » sur la ligne de l'étape.
fn remote_title(r: &str) -> String {
    apps::title(r.split(':').next().unwrap_or("remote"))
}

/// La cible pour rclone (vue depuis son container), et les fichiers qu'il doit voir.
/// Un fichier du serveur, et où rclone le voit dans son container.
type Mount = (&'static str, &'static str);

fn rclone_target(r: &str) -> Result<(String, Vec<Mount>), String> {
    if let Some(rest) = r.strip_prefix("sftp:") {
        let s = Sftp::parse(rest)?;
        // shell_type none: pas de commande à lancer en face (un compte enfermé n'a que sftp).
        // host_key_algorithms: la clé ed25519, celle que known_hosts garde (sinon rclone peut
        // négocier une clé RSA ou ECDSA et refuser: « knownhosts: key mismatch », J259)
        let target = format!(
            ":sftp,host={},user={},key_file=/secrets/backup_ssh,known_hosts_file=/secrets/backup_known_hosts,host_key_algorithms=ssh-ed25519,shell_type=none:{}",
            s.host, s.user, s.path
        );
        return Ok((
            target,
            vec![
                (SSH_KEY, "/secrets/backup_ssh"),
                (KNOWN_HOSTS, "/secrets/backup_known_hosts"),
            ],
        ));
    }
    if let Some(rest) = r.strip_prefix("rclone:") {
        if !Path::new(RCLONE_CONF).exists() {
            return Err(format!(
                "{r}: no {RCLONE_CONF}, where the remotes of rclone are (rclone config, then copy its file there, root only)"
            ));
        }
        return Ok((
            rest.to_string(),
            vec![(RCLONE_CONF, "/config/rclone/rclone.conf")],
        ));
    }
    Err(format!("{r}: not a remote destination"))
}

/// rclone, par son image du catalogue (D99, 5), le temps d'une commande. /backups (ou le dossier
/// local) monté en lecture seule, sauf pour un téléchargement.
fn rclone(
    local: &str,
    writable: bool,
    files: &[(&str, &str)],
    args: &[String],
) -> Result<String, String> {
    let all = rclone_args("dotbox-rclone", Some((local, writable)), files, args, false)?;
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    system::run("docker", &refs)
}

/// rclone, avec ou sans dossier local (--stream n'en a pas), en lecture seule.
fn rclone_in(
    local: Option<&str>,
    files: &[(&str, &str)],
    args: &[String],
) -> Result<String, String> {
    let all = rclone_args(
        "dotbox-rclone",
        local.map(|l| (l, false)),
        files,
        args,
        false,
    )?;
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    system::run("docker", &refs)
}

/// La ligne de docker run pour rclone. `input`: l'entrée de la commande reste ouverte (-i), pour
/// rclone rcat, qui envoie ce qu'il lit.
fn rclone_args(
    name: &str,
    local: Option<(&str, bool)>,
    files: &[(&str, &str)],
    args: &[String],
    input: bool,
) -> Result<Vec<String>, String> {
    let image = catalog::load_core(Path::new(github::DIR), "rclone")?
        .image("rclone")?
        .to_string();
    let mut all: Vec<String> = vec!["run".into(), "--rm".into()];
    if input {
        all.push("-i".into());
    }
    all.push("--name".into());
    all.push(name.into());
    if let Some((local, writable)) = local {
        all.push("-v".into());
        all.push(format!(
            "{local}:{local}{}",
            if writable { "" } else { ":ro" }
        ));
    }
    for (host, inside) in files {
        all.push("-v".into());
        all.push(format!("{host}:{inside}:ro"));
    }
    all.push(image);
    all.extend(args.iter().cloned());
    Ok(all)
}

/// La clé SSH de Dot Box pour sftp (D99, 3): créée la première fois, root seulement.
pub fn ensure_ssh_key() -> Result<bool, String> {
    if Path::new(SSH_KEY).exists() {
        return Ok(false);
    }
    system::run(
        "ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "dotbox backup",
            "-f",
            SSH_KEY,
        ],
    )?;
    fs::set_permissions(SSH_KEY, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot set the permissions of {SSH_KEY}: {e}"))?;
    Ok(true)
}

/// L'empreinte de l'autre machine, relevée à la première sauvegarde puis gardée (D99, 4).
fn ensure_known_host(host: &str) -> Result<Option<String>, String> {
    let known = fs::read_to_string(KNOWN_HOSTS).unwrap_or_default();
    if known
        .lines()
        .any(|l| l.split_whitespace().next() == Some(host))
    {
        return Ok(None);
    }
    let found = system::run("ssh-keyscan", &["-T", "10", "-t", "ed25519", host])?;
    let line = found
        .lines()
        .find(|l| !l.starts_with('#') && l.split_whitespace().nth(1) == Some("ssh-ed25519"))
        .ok_or(format!(
            "ssh-keyscan {host}: no ed25519 key, is SSH on port 22?"
        ))?
        .to_string();
    let mut f = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(KNOWN_HOSTS)
        .map_err(|e| format!("cannot write {KNOWN_HOSTS}: {e}"))?;
    f.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("cannot write {KNOWN_HOSTS}: {e}"))?;
    let tmp = std::env::temp_dir().join(format!("dotbox-hostkey-{}", std::process::id()));
    fs::write(&tmp, format!("{line}\n")).map_err(|e| e.to_string())?;
    let print = system::run("ssh-keygen", &["-l", "-f", &tmp.display().to_string()]);
    let _ = fs::remove_file(&tmp);
    Ok(Some(print.unwrap_or(line)))
}

/// Prépare une destination sftp: la clé de Dot Box, l'empreinte de l'autre machine.
fn prepare_sftp(r: &str) -> Result<(), String> {
    let Some(rest) = r.strip_prefix("sftp:") else {
        return Ok(());
    };
    let s = Sftp::parse(rest)?;
    if ensure_ssh_key()? {
        say("create", &format!("{SSH_KEY} (ed25519)"));
    }
    if let Some(print) = ensure_known_host(&s.host)? {
        say(
            "trust",
            &format!(
                "{} as {print}: compare it with ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub there",
                s.host
            ),
        );
    }
    Ok(())
}

/// Un message clair quand l'autre machine refuse la clé de Dot Box.
fn explain(r: &str, e: String) -> String {
    let low = e.to_lowercase();
    if r.starts_with("sftp:") && low.contains("knownhosts") {
        return format!(
            "{r}: the other machine does not show the key kept in {KNOWN_HOSTS}. If it was reinstalled, check its fingerprint there (ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub), then delete its line in {KNOWN_HOSTS}. Otherwise, someone may be in between: nothing was sent.\n{e}"
        );
    }
    if r.starts_with("sftp:")
        && (low.contains("unable to authenticate")
            || low.contains("permission denied")
            || low.contains("handshake failed"))
    {
        let public = fs::read_to_string(format!("{SSH_KEY}.pub")).unwrap_or_default();
        return format!(
            "{r} refuses the key of Dot Box: add this line to the account's authorized_keys there\n{}",
            public.trim()
        );
    }
    e
}

/// Les archives d'une destination distante: (nom du fichier, app, date).
pub fn remote_list(r: &str, local: Option<&str>) -> Result<Vec<(String, String, String)>, String> {
    prepare_sftp(r)?;
    let (target, files) = rclone_target(r)?;
    let out = rclone_in(
        local,
        &files,
        &["lsf".into(), "--files-only".into(), target],
    )
    .map_err(|e| explain(r, e))?;
    Ok(out
        .lines()
        .filter_map(|f| {
            let (app, stamp) = parse_name(f.trim())?;
            Some((f.trim().to_string(), app, stamp))
        })
        .collect())
}

/// Envoie les archives de cette sauvegarde, puis keep de l'autre côté. Rend le résumé.
fn send(
    r: &str,
    local: &str,
    stamp: &str,
    names: &[String],
    keep: usize,
) -> Result<String, String> {
    prepare_sftp(r)?;
    let (target, files) = rclone_target(r)?;
    let mut args: Vec<String> = vec!["copy".into(), local.into(), target.clone()];
    for n in names {
        args.push("--include".into());
        args.push(format!("{n}-{stamp}{SUFFIX}"));
    }
    rclone(local, false, &files, &args).map_err(|e| explain(r, e))?;
    say("send", &format!("{} to {r}", count(names.len())));
    prune_remote(r, Some(local), names, keep)?;
    Ok(format!("{} sent, keep {keep}", count(names.len())))
}

/// keep, par app, de l'autre côté aussi. `local`: le dossier que rclone voit (aucun pour --stream).
fn prune_remote(r: &str, local: Option<&str>, names: &[String], keep: usize) -> Result<(), String> {
    let (target, files) = rclone_target(r)?;
    let mut old: Vec<String> = Vec::new();
    let mut all = remote_list(r, local)?;
    all.sort_by(|a, b| b.2.cmp(&a.2));
    for n in names {
        old.extend(
            all.iter()
                .filter(|(_, app, _)| app == n)
                .skip(keep)
                .map(|(f, _, _)| f.clone()),
        );
    }
    if !old.is_empty() {
        let mut args: Vec<String> = vec!["delete".into(), target];
        for f in &old {
            args.push("--include".into());
            args.push(f.clone());
            say("delete", &format!("{r}/{f} (keep {keep})"));
        }
        rclone_in(local, &files, &args).map_err(|e| explain(r, e))?;
    }
    Ok(())
}

/// Rapporte des archives d'une destination distante dans `local`, pour un restore.
pub fn fetch(r: &str, local: &str, wanted: &[String]) -> Result<(), String> {
    prepare_sftp(r)?;
    prepare_dir(local)?;
    let (target, files) = rclone_target(r)?;
    let mut args: Vec<String> = vec!["copy".into(), target, local.into()];
    for f in wanted {
        args.push("--include".into());
        args.push(f.clone());
    }
    rclone(local, true, &files, &args).map_err(|e| explain(r, e))?;
    for f in wanted {
        let p = format!("{local}/{f}");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot set the permissions of {p}: {e}"))?;
    }
    Ok(())
}

// ---------- la planification (16.6)

/// Les deux unités systemd voulues: (chemin, contenu), None pour une unité à retirer.
pub fn units(settings: &Settings) -> Result<Vec<(&'static str, Option<String>)>, String> {
    let service = "# Écrit par dotbox (init, sync): la sauvegarde planifiée, backup.schedule de dotbox.yml (D97).
[Unit]
Description=Dot Box backup (backup.schedule in /opt/dotbox/dotbox.yml)
After=dotbox.service docker.service
Wants=dotbox.service

[Service]
Type=oneshot
ExecStart=/usr/local/bin/dotbox backup
"
    .to_string();
    let timer = on_calendar(&settings.schedule)?.map(|cal| {
        format!(
            "# Écrit par dotbox (init, sync): backup.schedule de dotbox.yml, « {} » (D97).
[Unit]
Description=Dot Box backup, {}

[Timer]
OnCalendar={cal}
# une sauvegarde manquée (serveur éteint) est faite au démarrage
Persistent=true

[Install]
WantedBy=timers.target
",
            settings.schedule, settings.schedule
        )
    });
    Ok(vec![(SERVICE, Some(service)), (TIMER, timer)])
}

/// Écrit les unités, puis active le timer (ou l'arrête pour off). Rend le résumé.
pub fn setup_timer(settings: &Settings) -> Result<String, String> {
    let wanted = units(settings)?;
    let off = wanted.iter().any(|(p, c)| *p == TIMER && c.is_none());
    if off && Path::new(TIMER).exists() {
        let _ = system::run("systemctl", &["disable", "--now", "dotbox-backup.timer"]);
    }
    let mut changed = false;
    for (path, content) in &wanted {
        match content {
            Some(text) => {
                if system::write_if_changed(path, text, 0o644)? {
                    say("write", path);
                    changed = true;
                }
            }
            None => {
                if Path::new(path).exists() {
                    fs::remove_file(path).map_err(|e| format!("cannot delete {path}: {e}"))?;
                    say("delete", path);
                    changed = true;
                }
            }
        }
    }
    if changed {
        system::run("systemctl", &["daemon-reload"])?;
    }
    if off {
        return Ok("backup off (backup.schedule)".into());
    }
    system::run("systemctl", &["enable", "dotbox-backup.timer"])?;
    if changed {
        system::run("systemctl", &["restart", "dotbox-backup.timer"])?;
        say("start", "dotbox-backup.timer");
    } else {
        system::run("systemctl", &["start", "dotbox-backup.timer"])?;
        say("check", "dotbox-backup.timer");
    }
    Ok(format!("backup {}", settings.schedule))
}

/// La prochaine sauvegarde planifiée, telle que systemd la donne.
pub fn next_run() -> Option<String> {
    let v = system::run(
        "systemctl",
        &[
            "show",
            "dotbox-backup.timer",
            "-p",
            "NextElapseUSecRealtime",
            "--value",
        ],
    )
    .ok()?;
    let v = v.trim();
    if v.is_empty() || v == "n/a" {
        return None;
    }
    let words: Vec<&str> = v.split_whitespace().collect();
    Some(match words.as_slice() {
        [day, date, time, ..] => format!("{day} {date} {}", time.get(..5).unwrap_or(time)),
        _ => v.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn schedules() {
        assert_eq!(on_calendar("off").unwrap(), None);
        assert_eq!(on_calendar("hourly").unwrap().unwrap(), "hourly");
        assert_eq!(
            on_calendar("daily 04:00").unwrap().unwrap(),
            "*-*-* 04:00:00"
        );
        assert_eq!(
            on_calendar("sunday 05:30").unwrap().unwrap(),
            "Sun *-*-* 05:30:00"
        );
        for bad in [
            "daily 4:00",
            "daily 24:00",
            "daily 04:60",
            "never",
            "daily",
            "funday 04:00",
            "",
        ] {
            assert!(on_calendar(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_and_defaults() {
        let s = Settings::of(&cfg("domain: example.com\n")).unwrap();
        assert_eq!(s.schedule, "daily 04:00");
        assert_eq!(s.keep, 7);
        assert_eq!(s.destinations, [Dest::Local("/backups".into())]);
        let s = Settings::of(&cfg(
            "backup:\n  schedule: sunday 05:00\n  keep: 3\n  exclude: [\"*/cache/*\"]\n  destinations:\n    - local: /srv/backups/\n    - rclone: b2:dotbox\n",
        ))
        .unwrap();
        assert_eq!(s.keep, 3);
        assert_eq!(s.exclude, ["*/cache/*"]);
        assert_eq!(
            s.destinations,
            [
                Dest::Local("/srv/backups".into()),
                Dest::Remote("rclone:b2:dotbox".into())
            ]
        );
        assert!(Settings::of(&cfg("backup:\n  keep: 0\n")).is_err());
        assert!(Settings::of(&cfg("backup:\n  schedule: daily 4h\n")).is_err());
        assert!(Settings::of(&cfg("backup:\n  destinations:\n    - local: /opt/x\n")).is_err());
        assert!(Settings::of(&cfg("backup:\n  destinations:\n    - ftp: x\n")).is_err());
    }

    #[test]
    fn destinations() {
        assert!(Dest::parse("/backups").is_ok());
        assert!(Dest::parse("backups").is_err());
        assert!(Dest::parse("/").is_err());
        assert!(Dest::parse("/opt").is_err());
        assert!(Dest::parse("/data/backups").is_err());
        assert!(Dest::parse("/srv/../opt").is_err());
        assert_eq!(
            Dest::parse("sftp:backup@nas:/x").unwrap(),
            Dest::Remote("sftp:backup@nas:/x".into())
        );
    }

    /// sftp et rclone (16.5, D99, 7)
    #[test]
    fn remote_destinations() {
        assert_eq!(
            Sftp::parse("dotbox-backup@nas.example.com:/backups/").unwrap(),
            Sftp {
                user: "dotbox-backup".into(),
                host: "nas.example.com".into(),
                path: "/backups".into()
            }
        );
        for bad in [
            "nas:/x",
            "u@nas",
            "u@nas:x",
            "u@nas:/../etc",
            "u;x@nas:/x",
            "u@na s:/x",
            "u@nas:/a b",
        ] {
            assert!(Sftp::parse(bad).is_err(), "{bad}");
        }
        assert!(Dest::parse("sftp:u@nas:/x").is_ok());
        assert!(Dest::parse("sftp:nas").is_err());
        assert!(Dest::parse("rclone:b2:dotbox").is_ok());
        assert!(Dest::parse("rclone:b2").is_err());
        assert!(Dest::parse("rclone::x").is_err());
        let (t, files) = rclone_target("sftp:u@nas.example.com:/backups").unwrap();
        assert_eq!(
            t,
            ":sftp,host=nas.example.com,user=u,key_file=/secrets/backup_ssh,known_hosts_file=/secrets/backup_known_hosts,host_key_algorithms=ssh-ed25519,shell_type=none:/backups"
        );
        assert_eq!(files.len(), 2);
        assert_eq!(remote_title("sftp:u@nas:/x"), "Sftp");
    }

    #[test]
    fn exclusions() {
        let p: Vec<String> = ["*/logs/*", "*.log", "jellyfin/cache"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(excluded(&p, "radarr/config/logs/radarr.txt"));
        assert!(!excluded(&p, "qbittorrent/config/qBittorrent/logs")); // le dossier reste, vide
        assert!(excluded(
            &p,
            "qbittorrent/config/qBittorrent/logs/qbittorrent.txt"
        ));
        assert!(excluded(&p, "radarr/config/radarr.log"));
        assert!(excluded(&p, "jellyfin/cache"));
        assert!(excluded(&p, "jellyfin/cache/images/a.jpg"));
        assert!(!excluded(&p, "jellyfin/cachefile"));
        assert!(!excluded(&p, "radarr/config/radarr.db"));
        assert!(!excluded(&p, "radarr/config/logs.db"));
        assert!(glob("a?c", "abc") && !glob("a?c", "ac"));
    }

    #[test]
    fn names() {
        assert_eq!(
            parse_name("radarr-2026-11-22-0400.tar.zst.age"),
            Some(("radarr".into(), "2026-11-22-0400".into()))
        );
        assert_eq!(
            parse_name("uptime-kuma-2026-11-22-0400.tar.zst.age"),
            Some(("uptime-kuma".into(), "2026-11-22-0400".into()))
        );
        assert_eq!(
            parse_name("dotbox-2026-11-22-0400.tar.zst.age"),
            Some(("dotbox".into(), "2026-11-22-0400".into()))
        );
        assert_eq!(parse_name("radarr-2026-11-22.tar.zst.age"), None);
        assert_eq!(
            parse_name(".radarr-2026-11-22-0400.tar.zst.age.partial"),
            None
        );
        assert_eq!(parse_name("notes.txt"), None);
    }

    #[test]
    fn keys() {
        let id = age::x25519::Identity::generate();
        use age::secrecy::ExposeSecret;
        let text = format!(
            "# created: x\n# public key: {}\n{}\n",
            id.to_public(),
            id.to_string().expose_secret()
        );
        assert_eq!(
            parse_key(&text).unwrap().to_public().to_string(),
            id.to_public().to_string()
        );
        assert!(parse_key("# nothing\n").is_err());
    }

    /// Une archive faite puis relue, et refusée avec une autre clé (16.1).
    #[test]
    fn sealed_and_opened() {
        let tmp = std::env::temp_dir().join(format!("dotbox-backup-test-{}", std::process::id()));
        let app = tmp.join("demo");
        fs::create_dir_all(app.join("config/logs")).unwrap();
        fs::create_dir_all(app.join("cache")).unwrap();
        fs::write(app.join("config/app.db"), "x".repeat(200_000)).unwrap();
        fs::write(app.join("config/logs/today.txt"), "log").unwrap();
        fs::write(app.join("config/old.log"), "log").unwrap();
        fs::write(app.join("cache/thumb"), "cache").unwrap();
        std::os::unix::fs::symlink("config/app.db", app.join("link")).unwrap();
        let id = age::x25519::Identity::generate();
        let target = tmp.join("demo.tar.zst.age");
        let pats: Vec<String> = ["*/logs/*", "*.log", "demo/cache"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let n = seal(&tmp, "demo", &pats, &id.to_public(), &target).unwrap();
        assert_eq!(n, 5); // demo, config, config/app.db, config/logs, link
        let size = fs::metadata(&target).unwrap().len();
        assert!(size < 20_000, "{size}"); // 200 kB de « x », compressés
        let mut ar = open(&target, &id).unwrap();
        let mut names: Vec<String> = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "demo",
                "demo/config",
                "demo/config/app.db",
                "demo/config/logs",
                "demo/link"
            ]
        );
        let other = age::x25519::Identity::generate();
        assert!(open(&target, &other).is_err());
        fs::remove_dir_all(&tmp).unwrap();
    }

    /// --stream: l'archive écrite dans l'entrée de deux commandes à la fois, comme vers deux
    /// rclone rcat, arrive entière des deux côtés. Une commande qui ferme son entrée coupe tout.
    #[test]
    fn streamed_to_two_commands() {
        use std::process::{Command, Stdio};
        let tmp = std::env::temp_dir().join(format!("dotbox-stream-test-{}", std::process::id()));
        let app = tmp.join("demo");
        fs::create_dir_all(app.join("config")).unwrap();
        // 2 Mo qui ne se compressent pas: plus que ce que garde un tuyau
        let mut x: u32 = 1;
        let noise: Vec<u8> = (0..2_000_000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (x >> 16) as u8
            })
            .collect();
        fs::write(app.join("config/app.db"), &noise).unwrap();
        let id = age::x25519::Identity::generate();
        let spawn = |cmd: String| {
            Command::new("sh")
                .args(["-c", &cmd])
                .stdin(Stdio::piped())
                .spawn()
                .unwrap()
        };
        let (a, b) = (tmp.join("a.age"), tmp.join("b.age"));
        let mut kids = [
            spawn(format!("cat > {}", a.display())),
            spawn(format!("cat > {}", b.display())),
        ];
        let outs = kids.iter_mut().filter_map(|k| k.stdin.take()).collect();
        let (n, tee) = seal_to(
            &tmp,
            "demo",
            &[],
            &id.to_public(),
            Tee { outs, bytes: 0 },
            "test",
        )
        .unwrap();
        let bytes = tee.bytes;
        drop(tee);
        for k in kids.iter_mut() {
            assert!(k.wait().unwrap().success());
        }
        assert_eq!(n, 3); // demo, config, config/app.db
        assert_eq!(fs::metadata(&a).unwrap().len(), bytes);
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        let mut ar = open(&a, &id).unwrap();
        let mut db = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap())
            .find(|e| e.path().unwrap().ends_with("app.db"))
            .unwrap();
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut db, &mut back).unwrap();
        assert_eq!(back, noise);
        // l'autre côté qui s'arrête (rclone refusé): l'écriture est coupée, une erreur, pas un blocage
        let mut gone = spawn("exit 0".into());
        let outs = gone.stdin.take().into_iter().collect();
        gone.wait().unwrap();
        let cut = seal_to(
            &tmp,
            "demo",
            &[],
            &id.to_public(),
            Tee { outs, bytes: 0 },
            "x",
        );
        assert!(cut.is_err());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn remote_paths() {
        assert_eq!(remote_file("b2:dotbox", "f"), "b2:dotbox/f");
        assert_eq!(remote_file("b2:", "f"), "b2:f");
        assert_eq!(
            remote_file(":sftp,host=h:/backups/", "f"),
            ":sftp,host=h:/backups/f"
        );
    }

    #[test]
    fn pruned_per_app() {
        let tmp = std::env::temp_dir().join(format!("dotbox-prune-test-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        for d in ["2026-11-20-0400", "2026-11-21-0400", "2026-11-22-0400"] {
            for a in ["radarr", "sonarr"] {
                fs::write(tmp.join(format!("{a}-{d}{SUFFIX}")), "x").unwrap();
            }
        }
        let dir = tmp.display().to_string();
        let gone = prune(&dir, "radarr", 2).unwrap();
        assert_eq!(gone.len(), 1);
        assert!(gone[0].ends_with("radarr-2026-11-20-0400.tar.zst.age"));
        assert_eq!(list(&dir).len(), 5);
        assert_eq!(list(&dir)[0].stamp, "2026-11-22-0400");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn timer_units() {
        let s = Settings::of(&cfg("domain: x.y\n")).unwrap();
        let u = units(&s).unwrap();
        assert!(
            u[1].1
                .as_deref()
                .unwrap()
                .contains("OnCalendar=*-*-* 04:00:00\n")
        );
        assert!(u[1].1.as_deref().unwrap().contains("Persistent=true"));
        let s = Settings::of(&cfg("backup:\n  schedule: \"off\"\n")).unwrap();
        assert_eq!(units(&s).unwrap()[1], (TIMER, None));
    }
}
