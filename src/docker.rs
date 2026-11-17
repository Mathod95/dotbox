//! Docker, installé par `dotbox init` en suivant la doc officielle (docs.docker.com/engine/install/
//! debian, /ubuntu, /linux-postinstall), avec les bonnes pratiques du Journal (étape 3):
//! paquets en conflit retirés seulement avec accord, clé du dépôt embarquée, daemon.json écrit
//! avant l'installation. Relancé, il ne refait que ce qui manque.

use crate::out::say;
use crate::system::{self, OsRelease};
use crate::{prompt, ui};

/// Clé officielle du dépôt Docker, la même pour Debian et Ubuntu.
/// Empreinte 9DC8 5822 9FC7 DD38 854A E2D8 8D81 803C 0EBF CD88, vérifiée avec gpg (Journal 3.3).
const KEY: &str = include_str!("../assets/docker.asc");
const KEYRING: &str = "/etc/apt/keyrings/docker.asc";
const SOURCES: &str = "/etc/apt/sources.list.d/docker.sources";
const DAEMON_JSON: &str = "/etc/docker/daemon.json";
const NETWORK: &str = "dotbox";

/// Les paquets de la doc, dernière version du dépôt.
const PACKAGES: &[&str] = &[
    "docker-ce",
    "docker-ce-cli",
    "containerd.io",
    "docker-buildx-plugin",
    "docker-compose-plugin",
];

/// Paquets en conflit à retirer avant l'installation (section « Uninstall old versions »).
pub fn conflicting_packages(os_id: &str) -> Vec<&'static str> {
    let mut list = vec![
        "docker.io",
        "docker-compose",
        "docker-doc",
        "docker-buildx",
        "podman-docker",
        "containerd",
        "runc",
    ];
    if os_id == "ubuntu" {
        list.insert(2, "docker-compose-v2");
    }
    list
}

/// Le dépôt, au format deb822 de la doc.
pub fn sources(os: &OsRelease, arch: &str) -> Result<String, String> {
    if os.codename.is_empty() {
        return Err("no version codename in /etc/os-release".into());
    }
    Ok(format!(
        "Types: deb\nURIs: https://download.docker.com/linux/{}\nSuites: {}\nComponents: stable\nArchitectures: {arch}\nSigned-By: {KEYRING}\n",
        os.id, os.codename
    ))
}

/// daemon.json: les logs avec le pilote local (rotation par défaut, recommandé par la doc) et
/// live-restore. Un fichier existant garde ses autres réglages.
pub fn daemon_json(existing: Option<&str>) -> Result<String, String> {
    let mut cfg = match existing.map(str::trim) {
        None | Some("") => serde_json::Map::new(),
        Some(text) => match serde_json::from_str(text) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => return Err(format!("{DAEMON_JSON} is not a JSON object")),
            Err(e) => return Err(format!("{DAEMON_JSON} is not valid JSON: {e}")),
        },
    };
    cfg.insert("log-driver".into(), "local".into());
    cfg.insert("live-restore".into(), true.into());
    let mut text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Installe Docker et le réseau dotbox. L'utilisateur n'entre pas dans le groupe docker: il donne
/// les droits de root sans mot de passe, Docker s'utilise avec sudo (Journal, D33).
pub fn setup(os: &OsRelease, arch: &str) -> Result<String, String> {
    let installed = system::package_version("docker-ce");
    match &installed {
        Some(v) => say("check", &format!("docker-ce {v}")),
        None => install(os, arch)?,
    }

    // Docker déjà là: daemon.json complété si besoin, puis redémarrage pour le prendre en compte.
    if installed.is_some() && write_daemon_json()? {
        system::run("systemctl", &["restart", "docker"])?;
        say("restart", "docker");
    }

    // Sur Debian et Ubuntu, Docker démarre seul au boot: on s'en assure quand même.
    let enabled = system::run("systemctl", &["is-enabled", "docker"]).is_ok_and(|s| s == "enabled");
    let active = system::run("systemctl", &["is-active", "docker"]).is_ok_and(|s| s == "active");
    if !(enabled && active) {
        system::run("systemctl", &["enable", "--now", "docker", "containerd"])?;
        say("enable", "docker and containerd at boot");
    }
    let info = system::run(
        "docker",
        &[
            "info",
            "--format",
            "{{.ServerVersion}} {{.LoggingDriver}} live-restore={{.LiveRestoreEnabled}}",
        ],
    )?;
    say("check", &format!("docker {info}"));

    if system::run(
        "docker",
        &["network", "inspect", "-f", "{{.Name}}", NETWORK],
    )
    .is_ok()
    {
        say("check", &format!("network {NETWORK}"));
    } else {
        system::run("docker", &["network", "create", NETWORK])?;
        say("create", &format!("network {NETWORK}"));
    }

    Ok(version())
}

/// Le sous-réseau IPv4 du réseau dotbox (« 172.18.0.0/16 »): là où sont Traefik et les apps.
pub fn network_subnet() -> Result<String, String> {
    let out = system::run(
        "docker",
        &[
            "network",
            "inspect",
            "-f",
            "{{range .IPAM.Config}}{{.Subnet}} {{end}}",
            NETWORK,
        ],
    )?;
    first_ipv4_subnet(&out).ok_or(format!("no IPv4 subnet for the network {NETWORK}"))
}

pub fn first_ipv4_subnet(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|s| s.contains('.') && s.contains('/'))
        .map(String::from)
}

/// Version du serveur Docker (vide si Docker ne répond pas).
pub fn version() -> String {
    system::run("docker", &["version", "--format", "{{.Server.Version}}"]).unwrap_or_default()
}

/// Télécharge une image si elle manque, avec la progression des couches sous l'étape en cours.
pub fn pull(image: &str) -> Result<(), String> {
    if system::run(
        "docker",
        &["image", "inspect", "--format", "{{.Id}}", image],
    )
    .is_ok()
    {
        say("check", &format!("image {image}"));
        return Ok(());
    }
    say("pull", &format!("{image} from {}", registry(image)));
    let mut layers = Layers::default();
    system::run_streaming("docker", &["pull", image], |line| {
        if let Some(p) = layers.read(line) {
            ui::progress(p);
        }
    })?;
    say("pulled", image);
    Ok(())
}

/// D'où vient une image: Docker Hub, sauf si son nom commence par un registre (lscr.io...).
pub fn registry(image: &str) -> &str {
    match image.split_once('/') {
        Some((host, _)) if host.contains('.') => host,
        _ => "Docker Hub",
    }
}

/// Suit les couches d'un docker pull: « <id>: Pulling fs layer », « <id>: Pull complete »...
#[derive(Default)]
pub struct Layers {
    seen: Vec<String>,
    done: Vec<String>,
}

impl Layers {
    /// Lit une ligne, rend le pourcentage de couches prêtes s'il a pu changer.
    pub fn read(&mut self, line: &str) -> Option<f64> {
        let (id, what) = line.split_once(": ")?;
        if id.len() < 6 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        if !self.seen.iter().any(|s| s == id) {
            self.seen.push(id.to_string());
        }
        if (what.starts_with("Pull complete") || what.starts_with("Already exists"))
            && !self.done.iter().any(|s| s == id)
        {
            self.done.push(id.to_string());
        }
        Some(100.0 * self.done.len() as f64 / self.seen.len() as f64)
    }
}

fn install(os: &OsRelease, arch: &str) -> Result<(), String> {
    // 1. paquets en conflit: jamais retirés sans accord
    let found: Vec<&str> = conflicting_packages(&os.id)
        .into_iter()
        .filter(|p| system::package_installed(p))
        .collect();
    if !found.is_empty() {
        let list = found.join(" ");
        say(
            "conflict",
            &format!("{list} (conflict with Docker's packages)"),
        );
        let question = format!("{} conflict with Docker\nRemove them?", found.join(", "));
        if !ui::ask(|| prompt::confirm(&question, "←/→ to choose, Enter to confirm"))? {
            return Err(format!(
                "nothing changed: remove them with apt-get remove {list}, then run dotbox init again"
            ));
        }
        let mut args = vec!["remove", "-y", "-q"];
        args.extend(&found);
        system::run("apt-get", &args)?;
        say("remove", &list);
    }

    // 2. clé et dépôt
    if !system::package_installed("ca-certificates") {
        system::apt_install("ca-certificates")?;
        say("install", "ca-certificates");
    }
    if system::write_if_changed(KEYRING, KEY, 0o644)? {
        say("write", KEYRING);
    }
    if system::write_if_changed(SOURCES, &sources(os, arch)?, 0o644)? {
        say("write", SOURCES);
    }

    // 3. daemon.json avant l'installation: Docker démarre directement avec
    write_daemon_json()?;

    // 4. les paquets, avec leurs recommandations comme dans la doc (dont iptables)
    system::run("apt-get", &["update", "-q"])?;
    say(
        "install",
        "docker-ce and its packages (official repository)",
    );
    system::apt_install_progress(PACKAGES, true, ui::progress)?;
    let v = system::package_version("docker-ce").unwrap_or_default();
    say("install", &format!("docker-ce {v}"));
    Ok(())
}

fn write_daemon_json() -> Result<bool, String> {
    let existing = std::fs::read_to_string(DAEMON_JSON).ok();
    let changed = system::write_if_changed(DAEMON_JSON, &daemon_json(existing.as_deref())?, 0o644)?;
    if changed {
        say(
            "write",
            &format!("{DAEMON_JSON} (local logs, live-restore)"),
        );
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {

    #[test]
    fn subnet_of_the_network() {
        assert_eq!(
            first_ipv4_subnet("172.18.0.0/16 fd00::/64 ").as_deref(),
            Some("172.18.0.0/16")
        );
        assert_eq!(first_ipv4_subnet("fd00::/64"), None);
    }

    use super::*;
    use crate::system::parse_os_release;
    use sha2::{Digest, Sha256};

    fn os(text: &str) -> OsRelease {
        parse_os_release(text)
    }

    #[test]
    fn sources_debian_and_ubuntu() {
        let cases = [
            (
                "ID=debian\nVERSION_ID=\"13\"\nVERSION_CODENAME=trixie\n",
                "debian",
                "trixie",
            ),
            (
                "ID=debian\nVERSION_ID=\"12\"\nVERSION_CODENAME=bookworm\n",
                "debian",
                "bookworm",
            ),
            (
                "ID=ubuntu\nVERSION_ID=\"24.04\"\nVERSION_CODENAME=noble\nUBUNTU_CODENAME=noble\n",
                "ubuntu",
                "noble",
            ),
            (
                "ID=ubuntu\nVERSION_ID=\"26.04\"\nVERSION_CODENAME=resolute\nUBUNTU_CODENAME=resolute\n",
                "ubuntu",
                "resolute",
            ),
        ];
        for (text, dir, suite) in cases {
            let s = sources(&os(text), "amd64").unwrap();
            assert_eq!(
                s,
                format!(
                    "Types: deb\nURIs: https://download.docker.com/linux/{dir}\nSuites: {suite}\nComponents: stable\nArchitectures: amd64\nSigned-By: /etc/apt/keyrings/docker.asc\n"
                )
            );
        }
        assert!(sources(&os("ID=debian\nVERSION_ID=\"13\"\n"), "amd64").is_err());
    }

    #[test]
    fn ubuntu_codename_wins() {
        // la doc Ubuntu: ${UBUNTU_CODENAME:-$VERSION_CODENAME}
        let o = os("ID=ubuntu\nVERSION_CODENAME=other\nUBUNTU_CODENAME=noble\n");
        assert_eq!(o.codename, "noble");
    }

    #[test]
    fn conflicts() {
        let debian = conflicting_packages("debian");
        let ubuntu = conflicting_packages("ubuntu");
        assert!(!debian.contains(&"docker-compose-v2"));
        assert!(ubuntu.contains(&"docker-compose-v2"));
        for p in ["docker.io", "podman-docker", "containerd", "runc"] {
            assert!(debian.contains(&p) && ubuntu.contains(&p), "{p}");
        }
        assert_eq!(ubuntu.len(), debian.len() + 1);
    }

    #[test]
    fn embedded_key_is_the_verified_one() {
        // SHA-256 du fichier dont gpg a affiché l'empreinte 9DC8 5822 9FC7 DD38 854A E2D8 8D81 803C 0EBF CD88
        let digest = Sha256::digest(KEY.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "1500c1f56fa9e26b9b8f42452a553675796ade0807cdce11975eb98170b3a570"
        );
        assert!(KEY.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"));
    }

    #[test]
    fn pull_progress_by_layers() {
        let mut l = Layers::default();
        assert_eq!(l.read("4.39.28: Pulling from authelia/authelia"), None);
        assert_eq!(l.read("a1b2c3d4e5f6: Pulling fs layer"), Some(0.0));
        assert_eq!(l.read("b2c3d4e5f6a1: Already exists"), Some(50.0));
        assert_eq!(l.read("a1b2c3d4e5f6: Downloading"), Some(50.0));
        assert_eq!(l.read("a1b2c3d4e5f6: Pull complete"), Some(100.0));
        assert_eq!(l.read("Digest: sha256:abc"), None);
        assert_eq!(l.read("Status: Downloaded newer image"), None);
    }

    #[test]
    fn registries() {
        assert_eq!(registry("traefik:v3.7.19"), "Docker Hub");
        assert_eq!(registry("authelia/authelia:4.39.28"), "Docker Hub");
        assert_eq!(
            registry("lscr.io/linuxserver/socket-proxy:3.4.7"),
            "lscr.io"
        );
    }

    #[test]
    fn daemon_json_new_and_merged() {
        let fresh: serde_json::Value = serde_json::from_str(&daemon_json(None).unwrap()).unwrap();
        assert_eq!(
            fresh,
            serde_json::json!({"log-driver": "local", "live-restore": true})
        );

        let merged = daemon_json(Some(
            r#"{"log-driver": "json-file", "log-opts": {"max-size": "10m"}, "debug": true}"#,
        ))
        .unwrap();
        let merged: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(merged["log-driver"], "local");
        assert_eq!(merged["live-restore"], true);
        assert_eq!(merged["debug"], true);
        assert_eq!(merged["log-opts"]["max-size"], "10m");

        // déjà à jour: même texte, donc rien à réécrire
        let once = daemon_json(None).unwrap();
        assert_eq!(daemon_json(Some(&once)).unwrap(), once);

        assert!(daemon_json(Some("[1, 2]")).is_err());
        assert!(daemon_json(Some("{not json")).is_err());
    }
}
