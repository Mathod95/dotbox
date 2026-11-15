//! Docker, installé par `dotbox init` en suivant la doc officielle (docs.docker.com/engine/install/
//! debian, /ubuntu, /linux-postinstall), avec les bonnes pratiques du Journal (phase C):
//! paquets en conflit retirés seulement avec accord, clé du dépôt embarquée, daemon.json écrit
//! avant l'installation. Relancé, il ne refait que ce qui manque.

use crate::out::say;
use crate::prompt;
use crate::system::{self, OsRelease};

/// Clé officielle du dépôt Docker, la même pour Debian et Ubuntu.
/// Empreinte 9DC8 5822 9FC7 DD38 854A E2D8 8D81 803C 0EBF CD88, vérifiée avec gpg (Journal C.3).
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

pub fn setup(os: &OsRelease, arch: &str, user: &str) -> Result<(), String> {
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

    if system::run("docker", &["network", "inspect", NETWORK]).is_ok() {
        say("check", &format!("network {NETWORK}"));
    } else {
        system::run("docker", &["network", "create", NETWORK])?;
        say("create", &format!("network {NETWORK}"));
    }

    if system::user_groups(user)?.iter().any(|g| g == "docker") {
        say("check", &format!("{user} in group docker"));
    } else {
        system::run("usermod", &["-aG", "docker", user])?;
        say("group", &format!("{user} added to docker"));
        say(
            "note",
            "the docker group gives root-level privileges (docs.docker.com), log in again to use it",
        );
    }
    Ok(())
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
        if !prompt::confirm("remove them? [y/N]")? {
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
    let mut args = vec!["install", "-y", "-q"];
    args.extend(PACKAGES);
    system::run("apt-get", &args)?;
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
