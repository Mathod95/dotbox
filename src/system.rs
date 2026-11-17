//! Lecture du système et commandes externes (apt-get, useradd...). Les fonctions de lecture
//! pures sont séparées de celles qui touchent le système, pour être testées sur le PC.

use std::collections::HashMap;
use std::fs;
use std::process::Command;

use crate::log;

/// Systèmes pris en charge (page Prerequisites): versions LTS seulement.
const SUPPORTED: &[(&str, &[&str])] = &[("debian", &["12", "13"]), ("ubuntu", &["24.04", "26.04"])];

pub struct OsRelease {
    pub id: String,
    pub version_id: String,
    pub pretty: String,
    /// Nom de version (trixie, noble...): UBUNTU_CODENAME sur Ubuntu, sinon VERSION_CODENAME,
    /// comme dans la doc d'installation de Docker.
    pub codename: String,
}

/// Lit /etc/os-release (lignes CLE=valeur, valeur parfois entre guillemets).
pub fn parse_os_release(text: &str) -> OsRelease {
    let map: HashMap<&str, String> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim().trim_matches('"').to_string()))
        .collect();
    let get = |k: &str| map.get(k).cloned().unwrap_or_default();
    let pretty = match get("PRETTY_NAME") {
        p if p.is_empty() => format!("{} {}", get("ID"), get("VERSION_ID")),
        p => p,
    };
    let codename = match get("UBUNTU_CODENAME") {
        c if c.is_empty() => get("VERSION_CODENAME"),
        c => c,
    };
    OsRelease {
        id: get("ID"),
        version_id: get("VERSION_ID"),
        pretty,
        codename,
    }
}

pub fn supported(os: &OsRelease) -> bool {
    SUPPORTED
        .iter()
        .any(|(id, versions)| *id == os.id && versions.contains(&os.version_id.as_str()))
}

pub fn os_release() -> Result<OsRelease, String> {
    fs::read_to_string("/etc/os-release")
        .map(|t| parse_os_release(&t))
        .map_err(|e| format!("cannot read /etc/os-release: {e}"))
}

/// UID effectif, deuxième valeur de la ligne « Uid: » de /proc/self/status.
pub fn parse_status_euid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

pub fn effective_uid() -> Result<u32, String> {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| parse_status_euid(&t))
        .ok_or_else(|| "cannot read the current user from /proc/self/status".to_string())
}

/// Nom d'un compte à partir de son UID, dans le contenu de /etc/passwd.
pub fn parse_passwd_name(text: &str, uid: u32) -> Option<String> {
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2].parse() == Ok(uid)).then(|| f[0].to_string())
    })
}

/// Le compte avec lequel on s'est connecté: SUDO_USER avec sudo, sinon le loginuid de la
/// session, qui ne change pas avec su. None si c'est root ou si la session n'en a pas.
pub fn login_user() -> Option<String> {
    if let Ok(u) = std::env::var("SUDO_USER") {
        if !u.is_empty() && u != "root" {
            return Some(u);
        }
    }
    let uid: u32 = fs::read_to_string("/proc/self/loginuid")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if uid == 0 || uid == u32::MAX {
        return None;
    }
    parse_passwd_name(&fs::read_to_string("/etc/passwd").ok()?, uid)
}

/// Nom d'utilisateur Linux sûr: minuscules, chiffres, « _ » et « - », 32 caractères au plus.
pub fn valid_username(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && name.len() <= 32
}

/// PATH complet de root: avec `su -c` (au lieu de `su -`), le PATH reste celui de l'utilisateur,
/// sans /usr/sbin, et useradd ou usermod seraient introuvables (Journal, J20).
const SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Une commande système, cherchée dans SYSTEM_PATH quel que soit le PATH reçu.
pub fn command(cmd: &str) -> Command {
    let mut c = Command::new(cmd);
    c.env("PATH", SYSTEM_PATH);
    c
}

/// La dernière ligne non vide d'une sortie d'erreur, pour un message court.
fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// Lance une commande et rend sa sortie, ou une erreur avec ce qu'elle a écrit.
/// La commande, sa sortie et son code de retour vont dans le log (D37).
pub fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    log::write(&format!("$ {cmd} {}", args.join(" ")));
    let o = command(cmd)
        .args(args)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .output()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    log::write(&out);
    log::write(&err);
    log::write(&format!("  exit {}", o.status.code().unwrap_or(-1)));
    if o.status.success() {
        Ok(out.trim().to_string())
    } else {
        Err(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            last_line(&err)
        ))
    }
}

/// Comme run, avec un texte envoyé sur l'entrée de la commande. Ce texte n'est jamais écrit dans
/// le log: il sert aux secrets (chpasswd).
pub fn run_with_input(cmd: &str, args: &[&str], input: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    log::write(&format!("$ {cmd} {}   (input not logged)", args.join(" ")));
    let mut child = command(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| format!("cannot write to {cmd}: {e}"))?;
    }
    let o = child
        .wait_with_output()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    let err = String::from_utf8_lossy(&o.stderr);
    log::write(&String::from_utf8_lossy(&o.stdout));
    log::write(&err);
    log::write(&format!("  exit {}", o.status.code().unwrap_or(-1)));
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            last_line(&err)
        ))
    }
}

/// Lance une commande longue et passe chaque ligne de sa sortie à `on_line` au fur et à mesure
/// (progression d'apt ou de docker pull). Tout va dans le log.
pub fn run_streaming(
    cmd: &str,
    args: &[&str],
    mut on_line: impl FnMut(&str),
) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    log::write(&format!("$ {cmd} {}", args.join(" ")));
    let mut child = command(cmd)
        .args(args)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_string(&mut text);
        }
        text
    });
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            log::write(&line);
            on_line(&line);
        }
    }
    let status = child.wait().map_err(|e| format!("cannot run {cmd}: {e}"))?;
    let err = errors.join().unwrap_or_default();
    log::write(&err);
    log::write(&format!("  exit {}", status.code().unwrap_or(-1)));
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            last_line(&err)
        ))
    }
}

/// Progression d'une ligne « pmstatus:paquet:pourcentage:texte » d'apt (APT::Status-Fd).
pub fn apt_progress(line: &str) -> Option<f64> {
    let rest = line
        .strip_prefix("pmstatus:")
        .or_else(|| line.strip_prefix("dlstatus:"))?;
    rest.split(':').nth(1)?.parse().ok()
}

/// apt-get install, avec la progression d'apt passée à `on_pct` (0 à 100).
pub fn apt_install_progress(
    packages: &[&str],
    recommends: bool,
    on_pct: impl Fn(f64),
) -> Result<(), String> {
    let mut args = vec!["-o", "APT::Status-Fd=1", "install", "-y", "-q"];
    if !recommends {
        args.push("--no-install-recommends");
    }
    args.extend(packages);
    let downloads = std::cell::Cell::new(true);
    run_streaming("apt-get", &args, |line| {
        if let Some(p) = apt_progress(line) {
            // le téléchargement compte pour le premier tiers, l'installation pour le reste
            if line.starts_with("pmstatus:") {
                downloads.set(false);
            }
            let pct = if downloads.get() {
                p / 3.0
            } else {
                33.0 + p * 2.0 / 3.0
            };
            on_pct(pct.min(100.0));
        }
    })
}

pub fn architecture() -> Result<String, String> {
    run("dpkg", &["--print-architecture"])
}

pub fn package_installed(name: &str) -> bool {
    run("dpkg-query", &["-W", "-f=${Status}", name]).is_ok_and(|s| s == "install ok installed")
}

/// Installe un paquet, sans mise à jour du système (apt-get update relit seulement les listes).
pub fn apt_install(name: &str) -> Result<(), String> {
    run("apt-get", &["update", "-q"])?;
    run(
        "apt-get",
        &["install", "-y", "-q", "--no-install-recommends", name],
    )
    .map(|_| ())
}

/// Version installée d'un paquet, None s'il n'est pas installé.
pub fn package_version(name: &str) -> Option<String> {
    run("dpkg-query", &["-W", "-f=${Status}|${Version}", name])
        .ok()
        .and_then(|s| s.strip_prefix("install ok installed|").map(String::from))
}

/// Écrit un fichier seulement si son contenu change, crée son dossier (0755) s'il manque.
/// Rend true si le fichier a été écrit.
pub fn write_if_changed(path: &str, content: &str, mode: u32) -> Result<bool, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if let Some(dir) = std::path::Path::new(path).parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("cannot write {path}: {e}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("cannot set the permissions of {path}: {e}"))?;
    Ok(true)
}

/// UID et GID d'un utilisateur (PUID et PGID des apps).
pub fn user_ids(name: &str) -> Result<(u32, u32), String> {
    let id = |flag: &str| -> Result<u32, String> {
        run("id", &[flag, name])?
            .parse()
            .map_err(|_| format!("cannot read the ids of {name}"))
    };
    Ok((id("-u")?, id("-g")?))
}

/// Le fuseau horaire du serveur (« Europe/Paris »), Etc/UTC s'il est introuvable.
pub fn timezone() -> String {
    if let Ok(t) = fs::read_to_string("/etc/timezone") {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| zone_from_link(&p.to_string_lossy()))
        .unwrap_or_else(|| "Etc/UTC".to_string())
}

pub fn zone_from_link(link: &str) -> Option<String> {
    link.split_once("zoneinfo/").map(|(_, z)| z.to_string())
}

/// Crée un dossier (0755) et le donne à un utilisateur.
pub fn owned_dir(path: &str, uid: u32, gid: u32) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(path)
        .map_err(|e| format!("cannot create {path}: {e}"))?;
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
        .map_err(|e| format!("cannot give {path} to {uid}:{gid}: {e}"))
}

pub fn user_exists(name: &str) -> bool {
    run("getent", &["passwd", name]).is_ok()
}

pub fn user_groups(name: &str) -> Result<Vec<String>, String> {
    run("id", &["-nG", name]).map(|s| s.split_whitespace().map(String::from).collect())
}

#[cfg(test)]
mod tests {

    #[test]
    fn timezone_from_the_localtime_link() {
        assert_eq!(
            zone_from_link("/usr/share/zoneinfo/Europe/Paris").as_deref(),
            Some("Europe/Paris")
        );
        assert_eq!(
            zone_from_link("../usr/share/zoneinfo/Etc/UTC").as_deref(),
            Some("Etc/UTC")
        );
        assert_eq!(zone_from_link("/etc/foo"), None);
    }

    use super::*;

    #[test]
    fn os_release_debian_13() {
        let os = parse_os_release(
            "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\nVERSION_ID=\"13\"\nVERSION_CODENAME=trixie\nID=debian\n",
        );
        assert_eq!(os.id, "debian");
        assert_eq!(os.version_id, "13");
        assert_eq!(os.pretty, "Debian GNU/Linux 13 (trixie)");
        assert_eq!(os.codename, "trixie");
        assert!(supported(&os));
    }

    #[test]
    fn os_release_supported_or_not() {
        let os = |id: &str, v: &str| parse_os_release(&format!("ID={id}\nVERSION_ID=\"{v}\"\n"));
        assert!(supported(&os("debian", "12")));
        assert!(supported(&os("ubuntu", "24.04")));
        assert!(supported(&os("ubuntu", "26.04")));
        assert!(!supported(&os("debian", "11")));
        assert!(!supported(&os("ubuntu", "25.10")));
        assert!(!supported(&os("fedora", "43")));
        assert_eq!(os("ubuntu", "26.04").pretty, "ubuntu 26.04");
    }

    #[test]
    fn status_euid() {
        let status = "Name:\tdotbox\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(parse_status_euid(status), Some(0));
        assert_eq!(parse_status_euid("Name:\tx\n"), None);
    }

    #[test]
    fn passwd_name() {
        let passwd =
            "root:x:0:0:root:/root:/bin/bash\nadmin:x:1000:1000:,,,:/home/admin:/bin/bash\n";
        assert_eq!(parse_passwd_name(passwd, 1000).as_deref(), Some("admin"));
        assert_eq!(parse_passwd_name(passwd, 0).as_deref(), Some("root"));
        assert_eq!(parse_passwd_name(passwd, 1001), None);
    }

    #[test]
    fn apt_status_lines() {
        assert_eq!(
            apt_progress("pmstatus:docker-ce:57.1429:Installing docker-ce (amd64)"),
            Some(57.1429)
        );
        assert_eq!(
            apt_progress("dlstatus:1:9.5:Retrieving file 1 of 10"),
            Some(9.5)
        );
        assert_eq!(
            apt_progress("Get:1 http://deb.debian.org trixie InRelease"),
            None
        );
    }

    #[test]
    fn usernames() {
        for ok in ["admin", "_svc", "media-1", "a_b"] {
            assert!(valid_username(ok), "{ok}");
        }
        for bad in [
            "",
            "Admin",
            "1admin",
            "ad min",
            "ad;min",
            "-admin",
            &"a".repeat(33),
        ] {
            assert!(!valid_username(bad), "{bad}");
        }
    }
}
