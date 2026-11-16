//! Le fichier de log de `dotbox init` (Journal, D37): tout ce que le terminal n'affiche pas.
//! Un fichier par lancement dans /opt/dotbox/logs/, lisible par root seul. Chaque ligne porte le
//! temps écoulé depuis le début. Jamais de secret: les appelants n'y écrivent ni mot de passe ni clé.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::sync::Mutex;
use std::time::Instant;

pub const DIR: &str = "/opt/dotbox/logs";

struct Log {
    file: File,
    path: String,
    start: Instant,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// Nom du fichier pour une date `AAAA-MM-JJ-HHMM`, avec un suffixe si le nom est déjà pris.
pub fn file_name(stamp: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = format!("init-{stamp}");
    let mut name = format!("{base}.log");
    let mut n = 2;
    while taken(&name) {
        name = format!("{base}-{n}.log");
        n += 1;
    }
    name
}

/// Ouvre le log de ce lancement. `header` est sa première ligne.
pub fn open(stamp: &str, header: &str) -> Result<String, String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(DIR)
        .map_err(|e| format!("cannot create {DIR}: {e}"))?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot set the permissions of {DIR}: {e}"))?;
    let name = file_name(stamp, |n| std::path::Path::new(DIR).join(n).exists());
    let path = format!("{DIR}/{name}");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("cannot create {path}: {e}"))?;
    let _ = writeln!(file, "{header}");
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(Log {
        file,
        path: path.clone(),
        start: Instant::now(),
    });
    Ok(path)
}

/// Le chemin du log ouvert, s'il y en a un.
pub fn path() -> Option<String> {
    LOG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|l| l.path.clone())
}

/// Ajoute des lignes au log (sans effet si aucun log n'est ouvert).
pub fn write(text: &str) {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(log) = guard.as_mut() {
        let t = log.start.elapsed().as_secs_f64();
        for line in text.lines() {
            let _ = writeln!(log.file, "[{t:7.1}s] {line}");
        }
    }
}

/// Les `n` dernières lignes du log, pour les montrer en cas d'erreur.
pub fn tail(n: usize) -> Vec<String> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let text = fs::read_to_string(path).unwrap_or_default();
    last_lines(&text, n)
}

pub fn last_lines(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_file_per_run() {
        assert_eq!(
            file_name("2026-11-16-1402", |_| false),
            "init-2026-11-16-1402.log"
        );
        let taken = ["init-2026-11-16-1402.log", "init-2026-11-16-1402-2.log"];
        assert_eq!(
            file_name("2026-11-16-1402", |n| taken.contains(&n)),
            "init-2026-11-16-1402-3.log"
        );
    }

    #[test]
    fn tail_skips_blank_lines() {
        let text = "a\nb\n\nc\nd\n\n";
        assert_eq!(last_lines(text, 2), ["c", "d"]);
        assert_eq!(last_lines(text, 10), ["a", "b", "c", "d"]);
        assert!(last_lines("", 3).is_empty());
    }
}
