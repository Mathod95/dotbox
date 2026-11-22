//! Traefik, installé par `dotbox init` (Journal, étape 4): un certificat Let's Encrypt par
//! sous-domaine en HTTP-01, redirection de HTTP vers HTTPS, seuls les containers marqués
//! traefik.enable=true sont routés. Traefik lit Docker à travers un socket proxy en lecture seule
//! (D20, recommandé par la doc de Traefik). Relancé, il ne refait que ce qui manque.
//!
//! Le plugin CrowdSec (10.7, D60): une copie locale, vérifiée par dotbox, que Traefik lit sur le
//! disque. Il déclare le middleware crowdsec@docker, qui soumet chaque requête à AppSec.

use crate::catalog::{Core, CoreFile};
use crate::out::say;
use crate::{crowdsec, docker, secrets, system};
use std::net::{IpAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

// Les versions de Traefik et du socket proxy sont dans le catalogue: catalog/core/traefik.yml
// (D64). Ici, la façon de les installer.

pub const COMPOSE: &str = "/opt/traefik/compose.yml";
pub const STATIC: &str = "/opt/traefik/config/traefik.yml";
const LOGS: &str = "/opt/traefik/logs";
const LOGROTATE: &str = "/etc/logrotate.d/dotbox-traefik";
/// Les plugins locaux, montés en lecture seule sur /plugins-local (le dossier de travail de
/// Traefik dans son image): <PLUGINS>/src/<module>.
const PLUGINS: &str = "/opt/traefik/plugins-local";
const PLUGIN_MODULE: &str = "github.com/maxlerebourg/crowdsec-bouncer-traefik-plugin";
/// La version installée du plugin et l'empreinte de son archive, pour ne pas le refaire.
const PLUGIN_STAMP: &str = "/opt/traefik/plugins-local/.dotbox";

const LE: &str = "https://acme-v02.api.letsencrypt.org/directory";
const LE_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// compose.yml de Traefik et de son socket proxy. Le tableau de bord de Traefik est sur
/// traefik.<domaine>, derrière Authelia (two_factor, groupe admins).
pub fn compose(domain: &str, image: &str, proxy_image: &str) -> String {
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Vos changements vont dans compose.override.yml, à côté.
name: traefik
services:
  traefik:
    image: {image}
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
      - /opt/traefik/plugins-local:/plugins-local:ro
    # La clé du plugin CrowdSec pour AppSec: lue dans ce fichier, jamais écrite dans la config.
    secrets:
      - crowdsec_appsec_key
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
      # AppSec (10.7, D60): le WAF de CrowdSec, pour les apps sans Authelia devant et le portail
      # d'Authelia. Mode appsec: les IP bannies, le pare-feu les bloque déjà. CrowdSec injoignable
      # ou réponse inattendue: la requête est bloquée (D69).
      traefik.http.middlewares.crowdsec.plugin.crowdsec.enabled: "true"
      traefik.http.middlewares.crowdsec.plugin.crowdsec.crowdsecMode: appsec
      traefik.http.middlewares.crowdsec.plugin.crowdsec.crowdsecAppsecEnabled: "true"
      traefik.http.middlewares.crowdsec.plugin.crowdsec.crowdsecAppsecHost: crowdsec:7422
      traefik.http.middlewares.crowdsec.plugin.crowdsec.crowdsecAppsecKeyFile: /run/secrets/crowdsec_appsec_key
      traefik.http.middlewares.crowdsec.plugin.crowdsec.crowdsecAppsecFailureBlock: "true"
      traefik.http.middlewares.crowdsec.plugin.crowdsec.crowdsecAppsecUnreachableBlock: "true"

  # Seul à tenir le socket Docker, en lecture: Traefik ne peut que lire les containers (Journal, D20).
  socket-proxy:
    image: {proxy_image}
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

secrets:
  crowdsec_appsec_key:
    file: {key}

networks:
  dotbox:
    external: true
  socket:
    internal: true
"#,
        key = secrets::path(crowdsec::TRAEFIK_SECRET)
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
  # La porte web de Dot Box (11.13, D106): le service tourne sur le serveur, pas dans un
  # container. dotbox écrit son routeur dans ce dossier, relu quand un fichier change.
  file:
    directory: /etc/traefik/dynamic
    watch: true

# Le plugin CrowdSec, pour AppSec (10.7): une copie locale vérifiée par dotbox, dans
# /plugins-local, jamais téléchargée par Traefik.
experimental:
  localPlugins:
    crowdsec:
      moduleName: {PLUGIN_MODULE}

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

/// La porte web (11.13): le routeur de dotbox.<domaine>, vers le service sur la passerelle du
/// réseau dotbox. Authelia d'abord (two_factor, admins), puis l'en-tête secret que le service
/// exige: une app du réseau qui l'appelle en direct ne l'a pas.
pub const DYNAMIC_DIR: &str = "/opt/traefik/config/dynamic";
pub const WEB_ROUTE: &str = "/opt/traefik/config/dynamic/dotbox.yml";

pub fn web_route(domain: &str, gateway: &str, secret: &str) -> String {
    format!(
        r#"# Écrit par dotbox (init, sync): la porte web de Dot Box (Journal, 11.13, D106).
# Ne pas modifier: il est réécrit, et il porte un secret (root seulement).
http:
  routers:
    dotbox:
      rule: "Host(`dotbox.{domain}`)"
      entryPoints:
        - websecure
      service: dotbox
      middlewares:
        - authelia@docker
        - dotbox-gate
      tls:
        certResolver: letsencrypt
  middlewares:
    dotbox-gate:
      headers:
        customRequestHeaders:
          {header}: "{secret}"
  services:
    dotbox:
      loadBalancer:
        servers:
          - url: "http://{gateway}:{port}"
"#,
        header = crate::serve::GATE_HEADER_NAME,
        port = crate::serve::WEB_PORT
    )
}

/// Le fichier de la porte web voulu: le secret (créé s'il manque) et la passerelle du réseau.
pub fn web_route_wanted(domain: &str) -> Result<String, String> {
    crate::secrets::ensure(crate::serve::GATE_SECRET)?;
    let secret = crate::secrets::read(crate::serve::GATE_SECRET)
        .ok_or("cannot read the secret of the web gate")?;
    Ok(web_route(domain, &crate::serve::gateway()?, secret.trim()))
}

/// Écrit le fichier de la porte web, en 0600: Traefik le relit seul, sans redémarrer.
pub fn write_web_route(domain: &str) -> Result<bool, String> {
    let text = web_route_wanted(domain)?;
    std::fs::create_dir_all(DYNAMIC_DIR)
        .map_err(|e| format!("cannot create {DYNAMIC_DIR}: {e}"))?;
    let changed = system::write_if_changed(WEB_ROUTE, &text, 0o600)?;
    if changed {
        say("write", &format!("{WEB_ROUTE} (dotbox.{domain})"));
    }
    Ok(changed)
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

/// Le plugin CrowdSec dans /opt/traefik/plugins-local/src/<module>: l'archive du catalogue,
/// téléchargée, vérifiée, puis son dossier mis en place. Rend true s'il a changé.
fn install_plugin(f: &CoreFile) -> Result<bool, String> {
    let dir = f
        .dir
        .as_deref()
        .ok_or("the Traefik plugin needs dir in the catalog")?;
    let target = format!("{PLUGINS}/src/{PLUGIN_MODULE}");
    let stamp = format!("{} {}\n", f.version, f.sha256);
    if std::fs::read_to_string(PLUGIN_STAMP).is_ok_and(|s| s == stamp)
        && std::path::Path::new(&format!("{target}/go.mod")).exists()
    {
        say("check", &format!("{target} ({})", f.version));
        return Ok(false);
    }
    let tmp = system::run("mktemp", &["-d"])?;
    let result = (|| {
        let tgz = format!("{tmp}/plugin.tgz");
        say(
            "download",
            &format!("crowdsec-bouncer-traefik-plugin {} from GitHub", f.version),
        );
        system::run("wget", &["-q", "-O", &tgz, &f.url])?;
        match crowdsec::sha256(&tgz) {
            Some(h) if h == f.sha256 => say("verify", "sha256 ok"),
            other => {
                return Err(format!(
                    "the Traefik plugin archive does not have the expected sha256 ({}): not installed",
                    other.unwrap_or_default()
                ));
            }
        }
        system::run("tar", &["-xzf", &tgz, "-C", &tmp])?;
        let src = format!("{tmp}/{dir}");
        if !std::path::Path::new(&format!("{src}/go.mod")).exists() {
            return Err(format!("the Traefik plugin archive has no {dir}/go.mod"));
        }
        let parent = std::path::Path::new(&target)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        system::run("mkdir", &["-p", &parent])?;
        system::run("rm", &["-rf", &target])?;
        system::run("mv", &[&src, &target])?;
        system::run("chmod", &["-R", "go-w,a+rX", PLUGINS])?;
        std::fs::write(PLUGIN_STAMP, &stamp)
            .map_err(|e| format!("cannot write {PLUGIN_STAMP}: {e}"))?;
        say("install", &format!("{target} ({})", f.version));
        Ok(true)
    })();
    let _ = system::run("rm", &["-rf", &tmp]);
    result
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

/// Le joker *.domaine pointe-t-il vers ce serveur? Rend la raison courte du refus, sinon None.
/// Seul le joker compte: Dot Box ne sert que des sous-domaines, le domaine lui-même peut pointer
/// ailleurs (un site web, par exemple). Un nom tiré au hasard teste le joker (Journal, D29).
pub fn dns_problem(domain: &str) -> Option<String> {
    let mine = match system::run("ip", &["-o", "-4", "addr", "show", "scope", "global"]) {
        Ok(text) => parse_ipv4s(&text),
        Err(e) => return Some(e),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let ips = resolve(&format!("dotbox-check-{nanos:x}.{domain}"));
    if ips.is_empty() {
        return Some(format!("*.{domain} does not resolve"));
    }
    if !ips.iter().any(|ip| mine.contains(ip)) {
        let list: Vec<String> = ips.iter().map(IpAddr::to_string).collect();
        return Some(format!(
            "*.{domain} points to {}, not here",
            list.join(", ")
        ));
    }
    None
}

/// Le domaine donné en option ou relu dans dotbox.yml: même vérification, avec quoi faire.
pub fn check_dns(domain: &str) -> Result<(), String> {
    match dns_problem(domain) {
        Some(why) => Err(format!(
            "{why}: point an A record for *.{domain} to this server, then run dotbox init again"
        )),
        None => {
            say("check", &format!("*.{domain} points to this server"));
            Ok(())
        }
    }
}

/// Les ports 80 et 443 sont libres, ou déjà tenus par Traefik.
pub fn check_ports() -> Result<(), String> {
    if container_running("traefik") {
        say("check", "ports 80 and 443 (Traefik)");
        return Ok(());
    }
    let busy: Vec<u16> = parse_listening_ports(&system::run("ss", &["-Htln"])?)
        .into_iter()
        .filter(|p| *p == 80 || *p == 443)
        .collect();
    if !busy.is_empty() {
        let who =
            system::run("ss", &["-Htlnp", "( sport = :80 or sport = :443 )"]).unwrap_or_default();
        return Err(format!(
            "ports 80 and 443 must be free for Traefik, in use:\n{who}"
        ));
    }
    say("check", "ports 80 and 443 free");
    Ok(())
}

/// État d'un container: « healthy », « running »... vide s'il n'existe pas. La santé seulement
/// s'il tourne: un container arrêté garde la dernière (« unhealthy »), il est « exited » (15.6).
pub fn container_state(name: &str) -> String {
    system::run("docker", &["inspect", "-f", STATE_FORMAT, name]).unwrap_or_default()
}

pub const STATE_FORMAT: &str = "{{if and .State.Running .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}";

pub fn setup(domain: &str, ca: &str, core: &Core, plugin: &CoreFile) -> Result<(), String> {
    let image = core.image("traefik")?;
    let proxy_image = core.image("socket-proxy")?;
    // la clé du plugin doit exister avant que Docker la monte (sinon il crée un dossier à sa place)
    if secrets::ensure(crowdsec::TRAEFIK_SECRET)? {
        say(
            "create",
            &format!("secret {}", secrets::path(crowdsec::TRAEFIK_SECRET)),
        );
    }
    let plugin_changed = install_plugin(plugin)?;
    let compose_changed =
        system::write_if_changed(COMPOSE, &compose(domain, image, proxy_image), 0o644)?;
    if compose_changed {
        say("write", COMPOSE);
    }
    // le dossier du fournisseur file existe avant que Traefik ne démarre (11.13, J280)
    std::fs::create_dir_all(DYNAMIC_DIR)
        .map_err(|e| format!("cannot create {DYNAMIC_DIR}: {e}"))?;
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
        docker::pull(proxy_image)?;
        docker::pull(image)?;
        system::run(
            "docker",
            &["compose", "-f", COMPOSE, "up", "-d", "--quiet-pull"],
        )?;
        say("start", "traefik and socket-proxy");
    } else if static_changed || plugin_changed {
        // la configuration statique et les plugins ne sont lus qu'au démarrage
        system::run("docker", &["restart", "traefik"])?;
        say("restart", "traefik (new configuration)");
    }
    wait_healthy("traefik", Duration::from_secs(90))?;
    say("check", &format!("traefik healthy ({image})"));
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

    /// La porte web (11.13): le fournisseur file, et le routeur de dotbox.<domaine>.
    #[test]
    fn web_gate_route() {
        assert!(
            static_config("letsencrypt")
                .contains("  file:\n    directory: /etc/traefik/dynamic\n    watch: true\n")
        );
        let r = web_route("example.com", "172.20.0.1", "s3cret");
        assert!(r.contains("rule: \"Host(`dotbox.example.com`)\""));
        assert!(r.contains("        - authelia@docker\n        - dotbox-gate\n"));
        assert!(r.contains("          X-Dotbox-Gate: \"s3cret\"\n"));
        assert!(r.contains("          - url: \"http://172.20.0.1:8800\"\n"));
        assert!(r.contains("certResolver: letsencrypt"));
    }

    #[test]
    fn compose_is_the_expected_one() {
        let core = crate::catalog::load_core(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog"),
            "traefik",
        )
        .unwrap();
        let (image, proxy) = (
            core.image("traefik").unwrap(),
            core.image("socket-proxy").unwrap(),
        );
        // les versions viennent du catalogue: une mise à jour par Renovate ne casse pas le test
        let expected = include_str!("../tests/expected/traefik/compose.yml")
            .replace("<traefik>", image)
            .replace("<socket-proxy>", proxy);
        assert_eq!(compose("example.com", image, proxy), expected);
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
        for text in [
            compose(
                "example.com",
                "traefik:v3.7.19",
                "lscr.io/linuxserver/socket-proxy:3.4.7-r0-ls106",
            ),
            static_config("letsencrypt"),
        ] {
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
