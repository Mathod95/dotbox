//! CrowdSec, installé par `dotbox init` (Journal, étape 10, D58 à D61).
//!
//! L'agent (le « Security Engine ») tourne dans un container, avec l'image officielle en variante
//! debian (journalctl, pour lire SSH dans le journal du système): il lit les connexions SSH et le
//! log d'accès de Traefik, repère les attaques et décide des bannissements. Il partage ses alertes
//! avec la communauté CrowdSec, sans console (D61). Son API locale n'écoute que sur 127.0.0.1.
//!
//! Le bouncer pare-feu applique ces décisions sur le système: le binaire officiel publié sur
//! GitHub, son empreinte vérifiée, en service systemd. Mode iptables (iptables-nft, comme Docker),
//! chaînes INPUT (SSH) et DOCKER-USER (Traefik et les ports des apps): une IP bannie n'atteint
//! plus rien. Il ne bloque rien d'autre: ce n'est pas un pare-feu du système (D59).

use std::path::Path;
use std::time::{Duration, Instant};

use crate::catalog::{Core, CoreFile};
use crate::out::say;
use crate::{docker, secrets, system, traefik};

/// Ce que l'agent surveille et sait détecter (le Hub de CrowdSec).
const COLLECTIONS: &str = "crowdsecurity/linux crowdsecurity/sshd crowdsecurity/traefik crowdsecurity/whitelist-good-actors";

const DIR: &str = "/opt/crowdsec";
const COMPOSE: &str = "/opt/crowdsec/compose.yml";
const CONFIG: &str = "/opt/crowdsec/config";
const DATA: &str = "/opt/crowdsec/data";
const ACQUIS: &str = "/opt/crowdsec/config/acquis.d/dotbox.yaml";

// L'image de l'agent et la release du bouncer (adresse, empreintes de l'archive et du binaire)
// viennent du catalogue, catalog/core/crowdsec.yml (D64): une nouvelle version ne demande pas
// de nouveau dotbox.
const BOUNCER_BIN: &str = "/usr/local/bin/crowdsec-firewall-bouncer";
const BOUNCER_CFG: &str = "/etc/crowdsec/bouncers/crowdsec-firewall-bouncer.yaml";
const BOUNCER_UNIT: &str = "/etc/systemd/system/crowdsec-firewall-bouncer.service";
const BOUNCER_SERVICE: &str = "crowdsec-firewall-bouncer";
/// La clé du bouncer: dans /opt/dotbox/secrets, passée à l'agent en secret Docker.
const BOUNCER_SECRET: &str = "crowdsec_bouncer_firewall";
/// Les ensembles d'IP bannies que le bouncer crée (sa configuration par défaut). Il leur ajoute
/// un suffixe par origine des décisions (crowdsec-blacklists-0...).
const SET_V4: &str = "crowdsec-blacklists";
/// La chaîne du bouncer: INPUT et DOCKER-USER y sautent, elle rejette les IP de ses ensembles.
const CHAIN: &str = "CROWDSEC_CHAIN";

pub fn compose(image: &str) -> String {
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Vos changements vont dans compose.override.yml, à côté.
name: crowdsec
services:
  crowdsec:
    image: {image}
    container_name: crowdsec
    restart: unless-stopped
    networks:
      - dotbox
    ports:
      # L'API locale, pour le bouncer pare-feu du système: sur 127.0.0.1, jamais sur Internet.
      - "127.0.0.1:8080:8080"
    environment:
      COLLECTIONS: "{COLLECTIONS}"
    volumes:
      - {CONFIG}:/etc/crowdsec
      - {DATA}:/var/lib/crowdsec/data
      # Les connexions SSH, dans le journal du système (Debian 13 n'écrit plus auth.log).
      - /var/log/journal:/run/log/journal:ro
      - /etc/machine-id:/etc/machine-id:ro
      # Le log d'accès de Traefik.
      - /opt/traefik/logs:/var/log/traefik:ro
    # La clé du bouncer pare-feu: l'image l'enregistre au démarrage (bouncer « firewall »).
    secrets:
      - bouncer_key_firewall
    security_opt:
      - no-new-privileges:true
    healthcheck:
      test: ["CMD", "cscli", "lapi", "status"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 60s

secrets:
  bouncer_key_firewall:
    file: {secret}

networks:
  dotbox:
    external: true
"#,
        secret = secrets::path(BOUNCER_SECRET)
    )
}

/// Ce que l'agent lit, en plus de sa configuration par défaut.
pub const ACQUIS_CONF: &str = r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Les connexions SSH, dans le journal du système.
source: journalctl
journalctl_filter:
  - "_SYSTEMD_UNIT=ssh.service"
labels:
  type: syslog
---
# Le log d'accès de Traefik (JSON).
filenames:
  - /var/log/traefik/access.log
labels:
  type: traefik
"#;

/// La configuration du bouncer: celle de sa release, en mode iptables, avec DOCKER-USER.
pub fn bouncer_config(api_key: &str, version: &str) -> String {
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# La configuration de la release {version}, en mode iptables (iptables-nft, comme Docker),
# sur les chaînes INPUT (SSH) et DOCKER-USER (Traefik et les ports des apps).
mode: iptables
update_frequency: 10s
log_mode: file
log_dir: /var/log/
log_level: info
log_compression: true
log_max_size: 100
log_max_backups: 3
log_max_age: 30
api_url: http://127.0.0.1:8080/
api_key: {api_key}
insecure_skip_verify: false
disable_ipv6: false
deny_action: DROP
deny_log: false
supported_decisions_types:
  - ban
blacklists_ipv4: {SET_V4}
blacklists_ipv6: crowdsec6-blacklists
ipset_type: nethash
iptables_chains:
  - INPUT
  - DOCKER-USER
iptables_add_rule_comments: true
"#
    )
}

/// Le service systemd: celui de la release, après Docker (l'agent est dans un container).
pub fn bouncer_unit() -> String {
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
[Unit]
Description=The firewall bouncer for CrowdSec
After=network.target docker.service
Wants=docker.service

[Service]
Type=notify
ExecStart={BOUNCER_BIN} -c {BOUNCER_CFG}
ExecStartPre={BOUNCER_BIN} -c {BOUNCER_CFG} -t
ExecStartPost=/bin/sleep 0.1
Restart=always
RestartSec=10
LimitNOFILE=65536
# Ne pas tuer les processus enfants: le mode iptables lance ipset plusieurs fois pour s'arrêter.
KillMode=mixed

[Install]
WantedBy=multi-user.target
"#
    )
}

fn sha256(path: &str) -> Option<String> {
    system::run("sha256sum", &[path])
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
}

/// Le binaire du bouncer: téléchargé seulement s'il manque ou n'est pas le bon. L'empreinte du
/// binaire dit, sans le lancer, que le bon est déjà installé.
fn install_bouncer_binary(f: &CoreFile) -> Result<bool, String> {
    let version = &f.version;
    if sha256(BOUNCER_BIN).as_deref() == Some(f.binary_sha256.as_str()) {
        say("check", &format!("{BOUNCER_BIN} ({version})"));
        return Ok(false);
    }
    let tmp = system::run("mktemp", &["-d"])?;
    let result = (|| {
        let tgz = format!("{tmp}/bouncer.tgz");
        say(
            "download",
            &format!("crowdsec-firewall-bouncer {version} from GitHub"),
        );
        system::run("wget", &["-q", "-O", &tgz, &f.url])?;
        match sha256(&tgz) {
            Some(h) if h == f.sha256 => say("verify", "sha256 ok"),
            other => {
                return Err(format!(
                    "the bouncer archive does not have the expected sha256 ({}): not installed",
                    other.unwrap_or_default()
                ));
            }
        }
        system::run("tar", &["-xzf", &tgz, "-C", &tmp])?;
        let bin = format!("{tmp}/{}", f.binary);
        if sha256(&bin).as_deref() != Some(f.binary_sha256.as_str()) {
            return Err(
                "the bouncer binary does not have the expected sha256: not installed".into(),
            );
        }
        system::run("install", &["-m", "0755", &bin, BOUNCER_BIN])?;
        say("install", &format!("{BOUNCER_BIN} ({version})"));
        Ok(true)
    })();
    let _ = system::run("rm", &["-rf", &tmp]);
    result
}

/// Attend que l'agent réponde (son healthcheck: cscli lapi status).
fn wait_healthy(timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let state = traefik::container_state("crowdsec");
        if state == "healthy" {
            return Ok(());
        }
        if state == "unhealthy" || state == "exited" || start.elapsed() > timeout {
            let logs =
                system::run("docker", &["logs", "--tail", "20", "crowdsec"]).unwrap_or_default();
            return Err(format!(
                "crowdsec is not healthy after {}s ({state}), last logs:\n{logs}",
                start.elapsed().as_secs()
            ));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Les règles du bouncer sont-elles en place? INPUT et DOCKER-USER sautent vers sa chaîne, qui
/// rejette les IP de ses ensembles (vu sur le serveur, J127).
pub fn rules_ok(input: &str, docker_user: &str, chain: &str) -> bool {
    let jump = format!("-j {CHAIN}");
    input.contains(&jump)
        && docker_user.contains(&jump)
        && chain.contains(&format!("--match-set {SET_V4}"))
}

fn rules_in_place() -> bool {
    let rules = |c: &str| system::run("iptables", &["-S", c]).unwrap_or_default();
    rules_ok(&rules("INPUT"), &rules("DOCKER-USER"), &rules(CHAIN))
}

/// La version de CrowdSec, lue dans le tag de l'image (v1.8.2-debian donne v1.8.2).
pub fn version(image: &str) -> String {
    image
        .rsplit_once(':')
        .map(|(_, t)| t)
        .unwrap_or(image)
        .trim_end_matches("-debian")
        .to_string()
}

/// Installe ou remet d'aplomb l'agent et le bouncer, aux versions du catalogue.
pub fn setup(core: &Core) -> Result<(), String> {
    let image = core.image("crowdsec")?;
    let bouncer = core.file("firewall-bouncer")?;
    // l'agent
    if secrets::ensure(BOUNCER_SECRET)? {
        say(
            "create",
            &format!("secret {}", secrets::path(BOUNCER_SECRET)),
        );
    }
    for d in [DIR, CONFIG, DATA, &format!("{CONFIG}/acquis.d")] {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {d}: {e}"))?;
    }
    let compose_changed = system::write_if_changed(COMPOSE, &compose(image), 0o644)?;
    if compose_changed {
        say("write", COMPOSE);
    }
    let acquis_changed = system::write_if_changed(ACQUIS, ACQUIS_CONF, 0o644)?;
    if acquis_changed {
        say(
            "write",
            &format!("{ACQUIS} (SSH in journald, Traefik access log)"),
        );
    }
    let state = traefik::container_state("crowdsec");
    let running = state == "running" || state == "healthy" || state == "starting";
    if compose_changed || !running {
        docker::pull(image)?;
        system::run(
            "docker",
            &["compose", "-f", COMPOSE, "up", "-d", "--quiet-pull"],
        )?;
        say("start", "crowdsec");
    } else if acquis_changed {
        system::run("docker", &["restart", "crowdsec"])?;
        say("restart", "crowdsec (new acquisition)");
    }
    wait_healthy(Duration::from_secs(240))?;
    say("check", &format!("crowdsec healthy ({image})"));

    // le bouncer pare-feu
    if !system::package_installed("ipset") {
        system::apt_install("ipset")?;
        say("install", "ipset");
    } else {
        say("check", "ipset");
    }
    let bin_changed = install_bouncer_binary(bouncer)?;
    let key = secrets::read(BOUNCER_SECRET).ok_or("the bouncer key is missing")?;
    if let Some(dir) = Path::new(BOUNCER_CFG).parent() {
        system::run("install", &["-d", "-m", "0700", &dir.to_string_lossy()])?;
    }
    let cfg_changed =
        system::write_if_changed(BOUNCER_CFG, &bouncer_config(&key, &bouncer.version), 0o600)?;
    if cfg_changed {
        say(
            "write",
            &format!("{BOUNCER_CFG} (iptables, INPUT and DOCKER-USER)"),
        );
    }
    let unit_changed = system::write_if_changed(BOUNCER_UNIT, &bouncer_unit(), 0o644)?;
    if unit_changed {
        say("write", BOUNCER_UNIT);
        system::run("systemctl", &["daemon-reload"])?;
    }
    let active =
        system::run("systemctl", &["is-active", BOUNCER_SERVICE]).is_ok_and(|s| s == "active");
    if !active {
        system::run("systemctl", &["enable", "--now", BOUNCER_SERVICE])?;
        say("start", BOUNCER_SERVICE);
    } else if bin_changed || cfg_changed || unit_changed {
        system::run("systemctl", &["restart", BOUNCER_SERVICE])?;
        say("restart", &format!("{BOUNCER_SERVICE} (new configuration)"));
    }
    // ses règles arrivent quand il a joint l'API de l'agent
    let start = Instant::now();
    while !rules_in_place() {
        if start.elapsed() > Duration::from_secs(60) {
            let logs = system::run(
                "journalctl",
                &["-u", BOUNCER_SERVICE, "-n", "15", "--no-pager"],
            )
            .unwrap_or_default();
            return Err(format!(
                "the firewall bouncer did not add its rules to INPUT and DOCKER-USER, last logs:\n{logs}"
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    say(
        "check",
        "bouncer rules: INPUT and DOCKER-USER jump to CROWDSEC_CHAIN",
    );
    Ok(())
}

/// État pour la carte de fin: celui du container, et du service du bouncer.
pub fn state() -> String {
    let agent = traefik::container_state("crowdsec");
    let bouncer = system::run("systemctl", &["is-active", BOUNCER_SERVICE]).unwrap_or_default();
    if agent == "healthy" && bouncer == "active" {
        "healthy".into()
    } else if agent.is_empty() {
        String::new()
    } else {
        format!("{agent}, bouncer {bouncer}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: &str = "crowdsecurity/crowdsec:v1.8.2-debian";

    #[test]
    fn files_are_the_expected_ones() {
        let core = crate::catalog::load_core(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog"),
            "crowdsec",
        )
        .unwrap();
        let (image, bouncer) = (
            core.image("crowdsec").unwrap(),
            core.file("firewall-bouncer").unwrap(),
        );
        // les versions viennent du catalogue: une mise à jour par Renovate ne casse pas le test
        assert_eq!(
            compose(image),
            include_str!("../tests/expected/crowdsec/compose.yml").replace("<crowdsec>", image)
        );
        assert_eq!(
            ACQUIS_CONF,
            include_str!("../tests/expected/crowdsec/acquis.yaml")
        );
        assert_eq!(
            bouncer_config("KEY", &bouncer.version),
            include_str!("../tests/expected/crowdsec/crowdsec-firewall-bouncer.yaml")
                .replace("<version>", &bouncer.version)
        );
        assert_eq!(
            bouncer_unit(),
            include_str!("../tests/expected/crowdsec/crowdsec-firewall-bouncer.service")
        );
    }

    #[test]
    fn the_api_never_listens_on_the_internet() {
        let c = compose(IMAGE);
        assert!(c.contains("\"127.0.0.1:8080:8080\""));
        assert!(!c.contains("- \"8080:8080\""));
        assert!(!c.contains("0.0.0.0"));
    }

    #[test]
    fn the_bouncer_blocks_input_and_docker_user() {
        let c = bouncer_config("KEY", "v0.0.38");
        assert!(c.contains("mode: iptables"));
        assert!(c.contains("  - INPUT\n  - DOCKER-USER\n"));
        assert!(c.contains("api_url: http://127.0.0.1:8080/"));
    }

    #[test]
    fn rules_as_the_bouncer_writes_them() {
        // la sortie de iptables -S sur le serveur de test (J127)
        let input = "-P INPUT ACCEPT\n-A INPUT -j CROWDSEC_CHAIN";
        let du = "-N DOCKER-USER\n-A DOCKER-USER -j CROWDSEC_CHAIN";
        let chain = "-N CROWDSEC_CHAIN\n-A CROWDSEC_CHAIN -m set --match-set crowdsec-blacklists-0 src -m comment --comment \"CrowdSec: crowdsec\" -j DROP";
        assert!(rules_ok(input, du, chain));
        assert!(!rules_ok(input, "-N DOCKER-USER", chain));
        assert!(!rules_ok(input, du, "-N CROWDSEC_CHAIN"));
    }

    #[test]
    fn the_version_comes_from_the_image_tag() {
        assert_eq!(version(IMAGE), "v1.8.2");
        assert_eq!(version("crowdsecurity/crowdsec:v1.9.0"), "v1.9.0");
    }
}
