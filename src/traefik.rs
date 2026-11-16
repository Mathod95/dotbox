//! Traefik, installé par `dotbox init` (Journal, phase T): un certificat Let's Encrypt par
//! sous-domaine en HTTP-01, redirection de HTTP vers HTTPS, seuls les containers marqués
//! traefik.enable=true sont routés. Traefik lit Docker à travers un socket proxy en lecture seule
//! (D20, recommandé par la doc de Traefik). Relancé, il ne refait que ce qui manque.

use crate::out::say;
use crate::system;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Dernières versions stables, fixées (D15).
pub const IMAGE: &str = "traefik:v3.7.19";
pub const PROXY_IMAGE: &str = "lscr.io/linuxserver/socket-proxy:3.4.7-r0-ls106";

const COMPOSE: &str = "/opt/traefik/compose.yml";
const STATIC: &str = "/opt/traefik/config/traefik.yml";
const LOGS: &str = "/opt/traefik/logs";
const LOGROTATE: &str = "/etc/logrotate.d/dotbox-traefik";

const LE: &str = "https://acme-v02.api.letsencrypt.org/directory";
const LE_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// compose.yml de Traefik et de son socket proxy. Le tableau de bord de Traefik est sur
/// traefik.<domaine>, derrière Authelia (two_factor, groupe admins).
pub fn compose(domain: &str) -> String {
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Vos changements vont dans compose.override.yml, à côté.
name: traefik
services:
  traefik:
    image: {IMAGE}
    container_name: traefik
    restart: unless-stopped
    depends_on:
      - socket-proxy
    ports:
      - "80:80"
      - "443:443"
    networks:
      - dotbox
      - socket
    volumes:
      - /opt/traefik/config:/etc/traefik
      - /opt/traefik/logs:/logs
    security_opt:
      - no-new-privileges:true
    healthcheck:
      test: ["CMD", "traefik", "healthcheck", "--ping"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 10s
    labels:
      traefik.enable: "true"
      traefik.http.routers.dashboard.rule: Host(`traefik.{domain}`)
      traefik.http.routers.dashboard.entrypoints: websecure
      traefik.http.routers.dashboard.service: api@internal
      traefik.http.routers.dashboard.middlewares: authelia@docker

  # Seul à tenir le socket Docker, en lecture: Traefik ne peut que lire les containers (Journal, D20).
  socket-proxy:
    image: {PROXY_IMAGE}
    container_name: socket-proxy
    restart: unless-stopped
    environment:
      CONTAINERS: "1"
      POST: "0"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    read_only: true
    tmpfs:
      - /run
    networks:
      - socket
    security_opt:
      - no-new-privileges:true

networks:
  dotbox:
    external: true
  socket:
    internal: true
"#
    )
}

/// Configuration statique de Traefik (/opt/traefik/config/traefik.yml).
pub fn static_config(ca: &str) -> String {
    let server = if ca == "letsencrypt-staging" {
        LE_STAGING
    } else {
        LE
    };
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
global:
  checkNewVersion: false
  sendAnonymousUsage: false
  # En-têtes au nom « alias » (X_Auth_User pour X-Auth-User) supprimés: ils pourraient
  # usurper ceux qu'Authelia transmet aux apps (doc de Traefik, aliasHeadersStrategy).
  aliasHeadersStrategy: delete

log:
  level: INFO

# Logs d'accès en JSON, pour CrowdSec. Tournés chaque jour par logrotate.
accessLog:
  filePath: /logs/access.log
  format: json

# Pour le healthcheck, sur un port interne jamais publié.
ping: {{}}

# Le tableau de bord, seulement par son routeur protégé par Authelia (jamais api.insecure)
api:
  dashboard: true

entryPoints:
  web:
    address: ":80"
    http:
      redirections:
        entryPoint:
          to: websecure
          scheme: https
  websecure:
    address: ":443"
    http:
      tls:
        certResolver: letsencrypt

providers:
  docker:
    endpoint: "tcp://socket-proxy:2375"
    exposedByDefault: false
    network: dotbox

certificatesResolvers:
  letsencrypt:
    acme:
      caServer: {server}
      storage: /etc/traefik/acme.json
      httpChallenge:
        entryPoint: web
"#
    )
}

pub const LOGROTATE_CONF: &str = r#"# Généré par dotbox init: logs d'accès de Traefik.
/opt/traefik/logs/*.log {
    daily
    rotate 14
    compress
    delaycompress
    missingok
    notifempty
    postrotate
        docker kill --signal=USR1 traefik >/dev/null 2>&1 || true
    endscript
}
"#;

/// Les IPv4 globales du serveur, depuis `ip -o -4 addr show scope global`.
pub fn parse_ipv4s(text: &str) -> Vec<IpAddr> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            f.position(|w| w == "inet")?;
            f.next()?.split('/').next()?.parse().ok()
        })
        .collect()
}

/// Ports en écoute, depuis `ss -Htln` (4e colonne: adresse:port).
pub fn parse_listening_ports(text: &str) -> Vec<u16> {
    text.lines()
        .filter_map(|l| {
            l.split_whitespace()
                .nth(3)?
                .rsplit(':')
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

fn resolve(name: &str) -> Vec<IpAddr> {
    (name, 0)
        .to_socket_addrs()
        .map(|it| it.map(|a| a.ip()).filter(IpAddr::is_ipv4).collect())
        .unwrap_or_default()
}

fn container_running(name: &str) -> bool {
    system::run("docker", &["inspect", "-f", "{{.State.Running}}", name]).is_ok_and(|s| s == "true")
}

/// Avant de toucher à quoi que ce soit: un sous-domaine tiré au hasard (le joker) pointe vers le
/// serveur, et les ports 80 et 443 sont libres (ou déjà tenus par Traefik).
pub fn check(domain: &str) -> Result<(), String> {
    let mine = parse_ipv4s(&system::run(
        "ip",
        &["-o", "-4", "addr", "show", "scope", "global"],
    )?);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // Seul le joker compte: Dot Box ne sert que des sous-domaines, le domaine lui-même peut
    // pointer ailleurs (un site web, par exemple). Un nom tiré au hasard teste le joker (Journal, D29).
    let random = format!("dotbox-check-{nanos:x}.{domain}");
    let ips = resolve(&random);
    if ips.is_empty() {
        return Err(format!(
            "*.{domain} does not resolve: create an A record to this server, then run dotbox init again"
        ));
    }
    if !ips.iter().any(|ip| mine.contains(ip)) {
        let list: Vec<String> = ips.iter().map(IpAddr::to_string).collect();
        return Err(format!(
            "*.{domain} points to {}, not to this server: fix the A record, then run dotbox init again",
            list.join(", ")
        ));
    }
    say("check", &format!("*.{domain} points to this server"));

    if container_running("traefik") {
        say("check", "ports 80 and 443 (Traefik)");
    } else {
        let busy: Vec<u16> = parse_listening_ports(&system::run("ss", &["-Htln"])?)
            .into_iter()
            .filter(|p| *p == 80 || *p == 443)
            .collect();
        if !busy.is_empty() {
            let who = system::run("ss", &["-Htlnp", "( sport = :80 or sport = :443 )"])
                .unwrap_or_default();
            return Err(format!(
                "ports 80 and 443 must be free for Traefik, in use:\n{who}"
            ));
        }
        say("check", "ports 80 and 443 free");
    }
    Ok(())
}

pub fn setup(domain: &str, ca: &str) -> Result<(), String> {
    let compose_changed = system::write_if_changed(COMPOSE, &compose(domain), 0o644)?;
    if compose_changed {
        say("write", COMPOSE);
    }
    let static_changed = system::write_if_changed(STATIC, &static_config(ca), 0o644)?;
    if static_changed {
        say("write", &format!("{STATIC} ({ca})"));
    }
    std::fs::create_dir_all(LOGS).map_err(|e| format!("cannot create {LOGS}: {e}"))?;
    if system::write_if_changed(LOGROTATE, LOGROTATE_CONF, 0o644)? {
        say("write", LOGROTATE);
    }

    let running = container_running("traefik") && container_running("socket-proxy");
    if compose_changed || !running {
        system::run(
            "docker",
            &["compose", "-f", COMPOSE, "up", "-d", "--quiet-pull"],
        )?;
        say("start", "traefik and socket-proxy");
    } else if static_changed {
        // la configuration statique n'est relue qu'au démarrage
        system::run("docker", &["restart", "traefik"])?;
        say("restart", "traefik (new configuration)");
    }
    wait_healthy("traefik", Duration::from_secs(90))?;
    say("check", &format!("traefik healthy ({IMAGE})"));
    Ok(())
}

fn wait_healthy(name: &str, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let status = system::run(
            "docker",
            &["inspect", "-f", "{{.State.Health.Status}}", name],
        )
        .unwrap_or_default();
        if status == "healthy" {
            return Ok(());
        }
        if start.elapsed() > timeout {
            let logs = system::run("docker", &["logs", "--tail", "20", name]).unwrap_or_default();
            return Err(format!(
                "{name} is not healthy after {}s ({status}), last logs:\n{logs}",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_is_the_expected_one() {
        assert_eq!(
            compose("example.com"),
            include_str!("../tests/expected/traefik/compose.yml")
        );
    }

    #[test]
    fn static_config_staging_and_production() {
        assert_eq!(
            static_config("letsencrypt-staging"),
            include_str!("../tests/expected/traefik/traefik.staging.yml")
        );
        let prod = static_config("letsencrypt");
        assert!(prod.contains("caServer: https://acme-v02.api.letsencrypt.org/directory"));
        assert!(!prod.contains("staging"));
    }

    #[test]
    fn generated_yaml_is_valid() {
        for text in [compose("example.com"), static_config("letsencrypt")] {
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text).unwrap();
        }
    }

    #[test]
    fn ipv4s_from_ip_addr() {
        let text = "2: enp1s0    inet 203.0.113.10/24 brd 203.0.113.255 scope global enp1s0\\       valid_lft forever\n\
                    3: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\\       valid_lft forever\n";
        let ips: Vec<String> = parse_ipv4s(text).iter().map(|i| i.to_string()).collect();
        assert_eq!(ips, ["203.0.113.10", "172.17.0.1"]);
    }

    #[test]
    fn listening_ports_from_ss() {
        let text = "LISTEN 0      4096         0.0.0.0:22         0.0.0.0:*\n\
                    LISTEN 0      4096       127.0.0.1:953        0.0.0.0:*\n\
                    LISTEN 0      4096            [::]:443           [::]:*\n";
        assert_eq!(parse_listening_ports(text), [22, 953, 443]);
    }
}
