//! Le moteur (Journal, 7.2): un manifest du catalogue devient /opt/<app>/compose.yml.
//! Réseau dotbox, aucun port publié, routage par Traefik sur <sous-domaine>.<domaine>, et le
//! middleware d'Authelia pour les niveaux two_factor et one_factor (Accessing Apps).

use crate::catalog::{Access, Manifest};

pub const HEADER: &str =
    "# Généré par dotbox install: ne pas modifier, il est réécrit à chaque install.";

/// Ce que le manifest ne sait pas: le serveur, l'utilisateur, les réglages de dotbox.yml.
pub struct Ctx {
    pub domain: String,
    pub subdomain: String,
    pub access: Access,
    pub uid: u32,
    pub gid: u32,
    pub tz: String,
    /// le réseau dotbox (« 172.18.0.0/16 »), là où est Traefik
    pub subnet: String,
}

pub fn dir(app: &str) -> String {
    format!("/opt/{app}")
}

pub fn compose_path(app: &str) -> String {
    format!("/opt/{app}/compose.yml")
}

/// Une valeur YAML entre apostrophes.
fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// Remplit les {{...}} d'une valeur du manifest.
pub fn fill(value: &str, app: &str, c: &Ctx) -> String {
    value
        .replace("{{uid}}", &c.uid.to_string())
        .replace("{{gid}}", &c.gid.to_string())
        .replace("{{tz}}", &c.tz)
        .replace("{{host}}", &format!("{}.{}", c.subdomain, c.domain))
        .replace("{{app}}", app)
        .replace("{{subnet}}", &c.subnet)
}

/// Ce que XML ne laisse pas écrire tel quel dans un texte (comme Radarr l'écrit lui-même).
fn xml_escape(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Pose des réglages dans un fichier XML à plat (<Config><Clé>valeur</Clé>...</Config>), en
/// gardant tout le reste (la clé d'API, par exemple). Sans fichier, il est créé avec ces seuls
/// réglages: l'app complète le reste au démarrage. `root`: l'élément racine (Config pour les
/// *arr, NetworkConfiguration pour Jellyfin). Une clé « Clé/Enfant » met un seul élément dans
/// Clé, à la place de son contenu: une liste d'une valeur (<KnownProxies><string>…</string>).
pub fn merge_xml(
    existing: Option<&str>,
    root: &str,
    values: &[(String, String)],
) -> Result<String, String> {
    let end_root = format!("</{root}>");
    let mut text = match existing {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => format!("<{root}>\n{end_root}\n"),
    };
    if !text.contains(&end_root) {
        return Err(format!(
            "no {end_root} in the file: not the expected format"
        ));
    }
    for (path, value) in values {
        let v = xml_escape(value);
        let (key, child) = match path.split_once('/') {
            Some((k, c)) => (k, Some(c)),
            None => (path.as_str(), None),
        };
        let open = format!("<{key}>");
        let close = format!("</{key}>");
        if let Some(i) = text.find(&open) {
            let start = i + open.len();
            let end = text[start..]
                .find(&close)
                .ok_or(format!("<{key}> without </{key}>"))?
                + start;
            let inner = match child {
                None => v,
                Some(c) => {
                    let want = format!("<{c}>{v}</{c}>");
                    // déjà là, même avec d'autres retours à la ligne: on ne touche à rien
                    let current: String = text[start..end]
                        .lines()
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .concat();
                    if current == want {
                        continue;
                    }
                    let line_start = text[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
                    let indent: String = text[line_start..i]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect();
                    format!("\n{indent}  {want}\n{indent}")
                }
            };
            text.replace_range(start..end, &inner);
        } else if let Some((i, len)) = [format!("<{key} />"), format!("<{key}/>")]
            .iter()
            .find_map(|e| text.find(e.as_str()).map(|i| (i, e.len())))
        {
            let line_start = text[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
            let indent: String = text[line_start..i]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let inner = match child {
                None => v,
                Some(c) => format!("\n{indent}  <{c}>{v}</{c}>\n{indent}"),
            };
            text.replace_range(i..i + len, &format!("{open}{inner}{close}"));
        } else {
            let at = text.rfind(&end_root).unwrap_or(text.len());
            let line = if text[..at].ends_with('\n') { "" } else { "\n" };
            let inner = match child {
                None => v,
                Some(c) => format!("\n    <{c}>{v}</{c}>\n  "),
            };
            text.insert_str(at, &format!("{line}  {open}{inner}{close}\n"));
        }
    }
    Ok(text)
}

/// Une valeur comme Qt l'écrit dans un fichier INI: \\ et \" échappés, et entre guillemets si
/// elle contient ; , = ou des espaces au bord, que Qt lirait autrement (; ouvre un commentaire,
/// , sépare une liste).
fn ini_value(v: &str) -> String {
    let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
    if v.contains([';', ',', '=', '"']) || v.starts_with(' ') || v.ends_with(' ') {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// Pose des réglages dans un fichier INI de Qt (qBittorrent.conf), en gardant tout le reste. La
/// clé « WebUI/Port » va dans [WebUI] en « Port », « BitTorrent/Session/Port » dans [BitTorrent]
/// en « Session\Port ». Une section qui manque est ajoutée à la fin, une clé qui manque à la fin
/// de sa section. Sans fichier, il est créé avec ces seuls réglages.
pub fn merge_ini(existing: Option<&str>, values: &[(String, String)]) -> Result<String, String> {
    let mut lines: Vec<String> = existing.unwrap_or("").lines().map(str::to_string).collect();
    for (path, value) in values {
        let (section, rest) = path
            .split_once('/')
            .ok_or(format!("{path}: expected Section/Key"))?;
        let key = rest.replace('/', "\\");
        let line = format!("{key}={}", ini_value(value));
        let header = format!("[{section}]");
        match lines.iter().position(|l| l.trim() == header) {
            Some(h) => {
                let end = lines[h + 1..]
                    .iter()
                    .position(|l| l.trim_start().starts_with('['))
                    .map_or(lines.len(), |i| h + 1 + i);
                let found = (h + 1..end).find(|&i| {
                    lines[i]
                        .split_once('=')
                        .is_some_and(|(k, _)| k.trim() == key)
                });
                match found {
                    Some(i) => lines[i] = line,
                    None => {
                        // après la dernière ligne non vide de la section
                        let at = (h + 1..end)
                            .rev()
                            .find(|&i| !lines[i].trim().is_empty())
                            .map_or(h + 1, |i| i + 1);
                        lines.insert(at, line);
                    }
                }
            }
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(header);
                lines.push(line);
            }
        }
    }
    Ok(lines.join("\n") + "\n")
}

pub fn compose(m: &Manifest, c: &Ctx) -> String {
    let app = &m.name;
    let mut out = format!(
        "{HEADER}
# Vos changements vont dans compose.override.yml, à côté.
name: {app}
services:
  {app}:
    image: {image}
    container_name: {app}
    restart: unless-stopped
    networks:
      - dotbox
",
        image = m.image
    );
    if m.run_as_user {
        // l'image ne connaît pas PUID et PGID: le container tourne avec l'utilisateur (7.11)
        out.push_str(&format!("    user: \"{}:{}\"\n", c.uid, c.gid));
    }
    if !m.ports.is_empty() {
        out.push_str(
            "    # pas du web (D44): publiés sur le serveur, au même numéro\n    ports:\n",
        );
        for p in &m.ports {
            out.push_str(&format!(
                "      - \"{n}:{n}/{proto}\"\n",
                n = p.number,
                proto = p.proto
            ));
        }
    }
    if !m.env.is_empty() {
        out.push_str("    environment:\n");
        for (k, v) in &m.env {
            out.push_str(&format!("      {k}: {}\n", quote(&fill(v, app, c))));
        }
    }
    if !m.volumes.is_empty() {
        out.push_str("    volumes:\n");
        for v in &m.volumes {
            let ro = if v.read_only { ":ro" } else { "" };
            out.push_str(&format!("      - {}:{}{ro}\n", v.host, v.container));
        }
    }
    out.push_str("    security_opt:\n      - no-new-privileges:true\n");
    // health: image, l'image a déjà son healthcheck: on n'en ajoute pas
    if let Some(path) = m.health.as_ref().filter(|h| h.as_str() != "image") {
        out.push_str(&format!(
            "    healthcheck:
      test: [\"CMD\", \"curl\", \"-fsS\", \"http://localhost:{port}{path}\"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 30s
",
            port = m.port
        ));
    }
    out.push_str(&format!(
        "    labels:
      traefik.enable: \"true\"
      traefik.http.routers.{app}.rule: Host(`{sub}.{domain}`)
      traefik.http.routers.{app}.entrypoints: websecure
      traefik.http.services.{app}.loadbalancer.server.port: \"{port}\"
",
        sub = c.subdomain,
        domain = c.domain,
        port = m.port
    ));
    if c.access.behind_authelia() {
        out.push_str(&format!(
            "      # {}: Authelia demande qui se connecte avant de laisser passer
      traefik.http.routers.{app}.middlewares: authelia@docker
",
            c.access.as_str()
        ));
    } else {
        // sans Authelia devant, ouverte à Internet: AppSec, le WAF de CrowdSec (10.7, D60)
        out.push_str(&format!(
            "      # {}: AppSec, le WAF de CrowdSec, bloque les attaques connues
      traefik.http.routers.{app}.middlewares: crowdsec@docker
",
            c.access.as_str()
        ));
    }
    out.push_str(
        "
networks:
  dotbox:
    external: true
",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;
    use std::path::Path;

    fn ctx(access: Access) -> Ctx {
        Ctx {
            domain: "example.com".into(),
            subdomain: "prowlarr".into(),
            access,
            uid: 1000,
            gid: 1000,
            tz: "Europe/Paris".into(),
            subnet: "172.18.0.0/16".into(),
        }
    }

    fn load(app: &str) -> Manifest {
        catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog"), app).unwrap()
    }

    #[test]
    fn radarr_compose_is_the_expected_one() {
        let m = load("radarr");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "radarr".into();
        let expected =
            include_str!("../tests/expected/radarr/compose.yml").replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn qui_compose_keeps_the_image_healthcheck() {
        let m = load("qui");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "qui".into();
        let expected =
            include_str!("../tests/expected/qui/compose.yml").replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn sonarr_compose_is_the_expected_one() {
        let m = load("sonarr");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "sonarr".into();
        let expected =
            include_str!("../tests/expected/sonarr/compose.yml").replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn radarr_settings_are_filled() {
        let m = load("radarr");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "radarr".into();
        let values: Vec<(String, String)> = m
            .settings
            .unwrap()
            .values
            .iter()
            .map(|(k, v)| (k.clone(), fill(v, &m.name, &c)))
            .collect();
        assert_eq!(
            values,
            [
                ("AuthenticationMethod".to_string(), "External".to_string()),
                (
                    "AllowedHosts".to_string(),
                    "radarr.example.com,radarr".to_string()
                ),
                ("TrustedNetworks".to_string(), "172.18.0.0/16".to_string()),
            ]
        );
    }

    #[test]
    fn xml_settings_keep_the_rest() {
        let v = |k: &str, x: &str| (k.to_string(), x.to_string());
        // pas de fichier: seulement nos réglages
        assert_eq!(
            merge_xml(None, "Config", &[v("AuthenticationMethod", "External")]).unwrap(),
            "<Config>\n  <AuthenticationMethod>External</AuthenticationMethod>\n</Config>\n"
        );
        // un fichier de Radarr: la clé d'API reste, une valeur change, une vide se remplit, une manque
        let radarr = "<Config>\n  <ApiKey>abc123</ApiKey>\n  <AuthenticationMethod>Forms</AuthenticationMethod>\n  <AllowedHosts></AllowedHosts>\n  <Port>7878</Port>\n</Config>";
        let out = merge_xml(
            Some(radarr),
            "Config",
            &[
                v("AuthenticationMethod", "External"),
                v("AllowedHosts", "radarr.example.com,radarr"),
                v("TrustedNetworks", "172.18.0.0/16"),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            "<Config>\n  <ApiKey>abc123</ApiKey>\n  <AuthenticationMethod>External</AuthenticationMethod>\n  <AllowedHosts>radarr.example.com,radarr</AllowedHosts>\n  <Port>7878</Port>\n  <TrustedNetworks>172.18.0.0/16</TrustedNetworks>\n</Config>"
        );
        // relancé: rien ne change
        assert_eq!(
            merge_xml(
                Some(&out),
                "Config",
                &[v("AuthenticationMethod", "External")]
            )
            .unwrap(),
            out
        );
        assert_eq!(
            merge_xml(
                Some("<Config>\n  <A />\n</Config>"),
                "Config",
                &[v("A", "x & y")]
            )
            .unwrap(),
            "<Config>\n  <A>x &amp; y</A>\n</Config>"
        );
        assert!(merge_xml(Some("<Other/>"), "Config", &[v("A", "x")]).is_err());
    }

    #[test]
    fn qbittorrent_compose_publishes_its_peer_port() {
        let m = load("qbittorrent");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "qbittorrent".into();
        let expected = include_str!("../tests/expected/qbittorrent/compose.yml")
            .replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn uptime_kuma_runs_as_the_user() {
        let m = load("uptime-kuma");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "uptime-kuma".into();
        let expected = include_str!("../tests/expected/uptime-kuma/compose.yml")
            .replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn code_server_sees_opt() {
        let m = load("code-server");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "code-server".into();
        let expected = include_str!("../tests/expected/code-server/compose.yml")
            .replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    /// Le qBittorrent.conf écrit avant le premier démarrage.
    #[test]
    fn qbittorrent_settings_are_the_expected_ones() {
        let m = load("qbittorrent");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "qbittorrent".into();
        let values: Vec<(String, String)> = m
            .settings
            .unwrap()
            .values
            .iter()
            .map(|(k, v)| (k.clone(), fill(v, &m.name, &c)))
            .collect();
        let out = merge_ini(None, &values).unwrap();
        assert_eq!(
            out,
            include_str!("../tests/expected/qbittorrent/qBittorrent.conf")
        );
        // relancé: rien ne change
        assert_eq!(merge_ini(Some(&out), &values).unwrap(), out);
    }

    #[test]
    fn jellyfin_compose_is_the_expected_one() {
        let m = load("jellyfin");
        let mut c = ctx(Access::App);
        c.subdomain = "jellyfin".into();
        let expected = include_str!("../tests/expected/jellyfin/compose.yml")
            .replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
        // l'assistant pas fini: le même compose, derrière Authelia (D71)
        c.access = Access::TwoFactor;
        let pending = compose(&m, &c);
        assert!(pending.contains("traefik.http.routers.jellyfin.middlewares: authelia@docker"));
        assert!(!pending.contains("crowdsec@docker"));
    }

    #[test]
    fn jellyfin_network_xml() {
        let m = load("jellyfin");
        let mut c = ctx(Access::App);
        c.subdomain = "jellyfin".into();
        let st = m.settings.unwrap();
        let values: Vec<(String, String)> = st
            .values
            .iter()
            .map(|(k, v)| (k.clone(), fill(v, &m.name, &c)))
            .collect();
        // avant le premier démarrage: seulement les proxys de confiance
        let out = merge_xml(None, &st.root, &values).unwrap();
        assert_eq!(out, include_str!("../tests/expected/jellyfin/network.xml"));
        assert_eq!(merge_xml(Some(&out), &st.root, &values).unwrap(), out);
        // le fichier réécrit par Jellyfin: rien ne change, même mis en forme autrement
        let jf = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<NetworkConfiguration xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n  <EnableRemoteAccess>true</EnableRemoteAccess>\n  <KnownProxies>\n    <string>172.18.0.0/16</string>\n  </KnownProxies>\n  <IgnoreVirtualInterfaces>true</IgnoreVirtualInterfaces>\n</NetworkConfiguration>";
        assert_eq!(merge_xml(Some(jf), &st.root, &values).unwrap(), jf);
        // une autre liste: remplacée, le reste gardé
        let other = jf.replace("172.18.0.0/16", "10.0.0.1</string>\n    <string>10.0.0.2");
        assert_eq!(merge_xml(Some(&other), &st.root, &values).unwrap(), jf);
        // une liste vide (<KnownProxies />), comme Jellyfin l'écrit
        let empty = jf.replace(
            "<KnownProxies>\n    <string>172.18.0.0/16</string>\n  </KnownProxies>",
            "<KnownProxies />",
        );
        assert_eq!(merge_xml(Some(&empty), &st.root, &values).unwrap(), jf);
        assert!(merge_xml(Some("<Config>\n</Config>"), &st.root, &values).is_err());
    }

    #[test]
    fn ini_settings_keep_the_rest() {
        let v = |k: &str, x: &str| (k.to_string(), x.to_string());
        // un fichier de qBittorrent: les autres clés et sections restent, une valeur change,
        // une clé manque dans sa section, une section manque
        let conf = "[BitTorrent]\nSession\\Port=6881\nSession\\QueueingSystemEnabled=true\n\n[Meta]\nMigrationVersion=12\n";
        let out = merge_ini(
            Some(conf),
            &[
                v("BitTorrent/Session/Port", "56881"),
                v("BitTorrent/Session/DefaultSavePath", "/data/torrents"),
                v("WebUI/Port", "8080"),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            "[BitTorrent]\nSession\\Port=56881\nSession\\QueueingSystemEnabled=true\nSession\\DefaultSavePath=/data/torrents\n\n[Meta]\nMigrationVersion=12\n\n[WebUI]\nPort=8080\n"
        );
        // comme Qt: entre guillemets avec ; , ou =, \\ et \" échappés
        assert_eq!(ini_value("a;b"), "\"a;b\"");
        assert_eq!(ini_value("a,b"), "\"a,b\"");
        assert_eq!(ini_value("a\\b"), "a\\\\b");
        assert_eq!(ini_value("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(ini_value("172.18.0.0/16"), "172.18.0.0/16");
        assert!(merge_ini(None, &[v("Port", "1")]).is_err());
    }

    fn prowlarr() -> Manifest {
        catalog::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog"),
            "prowlarr",
        )
        .unwrap()
    }

    #[test]
    fn prowlarr_compose_is_the_expected_one() {
        let m = prowlarr();
        let expected = include_str!("../tests/expected/prowlarr/compose.yml")
            .replace("<version>", m.version());
        assert_eq!(compose(&m, &ctx(Access::TwoFactor)), expected);
    }

    #[test]
    fn no_authelia_for_app_and_public() {
        let m = prowlarr();
        for a in [Access::App, Access::Public] {
            assert!(!compose(&m, &ctx(a)).contains("authelia@docker"));
            // ouverte à Internet: AppSec devant (10.7)
            assert!(compose(&m, &ctx(a)).contains("middlewares: crowdsec@docker"));
        }
        assert!(!compose(&m, &ctx(Access::TwoFactor)).contains("crowdsec@docker"));
        assert!(compose(&m, &ctx(Access::OneFactor)).contains("authelia@docker"));
    }

    #[test]
    fn never_a_published_port() {
        assert!(!compose(&prowlarr(), &ctx(Access::Public)).contains("ports:"));
    }

    #[test]
    fn values_are_quoted() {
        let mut c = ctx(Access::App);
        c.tz = "it's".into();
        assert!(compose(&prowlarr(), &c).contains("TZ: 'it''s'"));
    }
}
