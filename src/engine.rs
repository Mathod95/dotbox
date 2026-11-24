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

/// Le nom du fichier d'un secret de l'app dans /opt/dotbox/secrets (D151).
pub fn secret_name(app: &str, name: &str) -> String {
    format!("{app}_{name}")
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

/// Pose des réglages dans un fichier JSON (settings.json de Seerr), en gardant le reste et
/// l'ordre des clés. « a/b/c »: la clé c de l'objet b de l'objet a, créés s'ils manquent.
/// « true » et « false » deviennent des booléens. Rien ne change si les valeurs y sont déjà:
/// le texte de l'app est rendu tel quel.
pub fn merge_json(existing: Option<&str>, values: &[(String, String)]) -> Result<String, String> {
    use serde_json::Value;
    let text = existing.unwrap_or("{}");
    let mut doc: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let before = doc.clone();
    for (path, value) in values {
        let v = match value.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            s => Value::String(s.to_string()),
        };
        let (parents, last) = match path.rsplit_once('/') {
            Some((p, l)) => (p.split('/').collect::<Vec<_>>(), l),
            None => (Vec::new(), path.as_str()),
        };
        let mut node = &mut doc;
        for part in parents {
            node = node
                .as_object_mut()
                .ok_or(format!("{path}: {part} is not inside an object"))?
                .entry(part.to_string())
                .or_insert_with(|| Value::Object(Default::default()));
        }
        node.as_object_mut()
            .ok_or(format!("{path}: {last} is not inside an object"))?
            .insert(last.to_string(), v);
    }
    if doc == before && existing.is_some() {
        return Ok(text.to_string());
    }
    serde_json::to_string_pretty(&doc)
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}

/// Pose des réglages dans un fichier YAML (config.yaml de code-server), des clés du premier
/// niveau, en gardant les autres. « true » et « false » deviennent des booléens. Rien ne change
/// si les valeurs y sont déjà: le texte de l'app est rendu tel quel.
pub fn merge_yaml(existing: Option<&str>, values: &[(String, String)]) -> Result<String, String> {
    use serde_yaml_ng::{Mapping, Value};
    let text = existing.unwrap_or("");
    let mut doc: Mapping = match serde_yaml_ng::from_str::<Value>(text) {
        Ok(Value::Mapping(m)) => m,
        Ok(Value::Null) => Mapping::new(),
        Ok(_) => return Err("not a YAML mapping".into()),
        Err(e) => return Err(format!("not valid YAML: {e}")),
    };
    let before = doc.clone();
    for (key, value) in values {
        let v = match value.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            s => Value::String(s.to_string()),
        };
        doc.insert(Value::String(key.clone()), v);
    }
    if doc == before && existing.is_some() {
        return Ok(text.to_string());
    }
    serde_yaml_ng::to_string(&doc).map_err(|e| e.to_string())
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
    if !m.command.is_empty() {
        let args: Vec<String> = m.command.iter().map(|a| quote(&fill(a, app, c))).collect();
        out.push_str(&format!("    command: [{}]\n", args.join(", ")));
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
    if !m.cap_add.is_empty() {
        out.push_str("    # des droits réseau du noyau, pour un VPN (D75)\n    cap_add:\n");
        for c in &m.cap_add {
            out.push_str(&format!("      - {c}\n"));
        }
    }
    if !m.sysctls.is_empty() {
        out.push_str("    sysctls:\n");
        for (k, v) in &m.sysctls {
            out.push_str(&format!("      {k}: \"{v}\"\n"));
        }
    }
    if !m.env.is_empty() {
        out.push_str("    environment:\n");
        for (k, v) in &m.env {
            out.push_str(&format!("      {k}: {}\n", quote(&fill(v, app, c))));
        }
    }
    if !m.volumes.is_empty() || !m.secrets.is_empty() {
        out.push_str("    volumes:\n");
        for v in &m.volumes {
            let ro = if v.read_only { ":ro" } else { "" };
            out.push_str(&format!("      - {}:{}{ro}\n", v.host, v.container));
        }
        // les secrets: tirés par dotbox, lus par l'app dans /run/secrets (D151)
        for s in &m.secrets {
            out.push_str(&format!(
                "      - {}:/run/secrets/{s}:ro\n",
                crate::secrets::path(&secret_name(app, s))
            ));
        }
    }
    out.push_str("    security_opt:\n      - no-new-privileges:true\n");
    // health: image, l'image a déjà son healthcheck: on n'en ajoute pas
    if let Some(path) = m.health.as_ref().filter(|h| h.as_str() != "image") {
        // wget pour les images sans curl (celui de busybox, sur Alpine)
        let tool = if m.health_wget {
            "\"wget\", \"-q\", \"--spider\""
        } else {
            "\"curl\", \"-fsS\""
        };
        out.push_str(&format!(
            "    healthcheck:
      test: [\"CMD\", {tool}, \"http://localhost:{port}{path}\"]
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
        // des chemins derrière Authelia quand même (/admin): un routeur à eux, plus précis,
        // donc prioritaire pour Traefik
        for p in &m.authelia_paths {
            let r = format!("{app}{}", p.replace('/', "-"));
            out.push_str(&format!(
                "      # {p}: derrière Authelia, en plus
      traefik.http.routers.{r}.rule: Host(`{sub}.{domain}`) && PathPrefix(`{p}`)
      traefik.http.routers.{r}.entrypoints: websecure
      traefik.http.routers.{r}.service: {app}
      traefik.http.routers.{r}.middlewares: authelia@docker
",
                sub = c.subdomain,
                domain = c.domain
            ));
        }
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

    /// Homarr: sa clé de chiffrement montée depuis /opt/dotbox/secrets, jamais dans le compose (D151).
    #[test]
    fn homarr_reads_its_key_from_a_file() {
        let m = load("homarr");
        let mut c = ctx(Access::TwoFactor);
        c.subdomain = "homarr".into();
        let out = compose(&m, &c);
        let expected =
            include_str!("../tests/expected/homarr/compose.yml").replace("<version>", m.version());
        assert_eq!(out, expected);
        assert!(
            out.contains(
                "/opt/dotbox/secrets/homarr_encryption_key:/run/secrets/encryption_key:ro"
            )
        );
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
    fn seerr_compose_is_the_expected_one() {
        let m = load("seerr");
        let mut c = ctx(Access::App);
        c.subdomain = "seerr".into();
        let expected =
            include_str!("../tests/expected/seerr/compose.yml").replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn wizarr_compose_is_the_expected_one() {
        let m = load("wizarr");
        let mut c = ctx(Access::Public);
        c.subdomain = "wizarr".into();
        let expected =
            include_str!("../tests/expected/wizarr/compose.yml").replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn vaultwarden_compose_is_the_expected_one() {
        let m = load("vaultwarden");
        let mut c = ctx(Access::App);
        c.subdomain = "vaultwarden".into();
        let expected = include_str!("../tests/expected/vaultwarden/compose.yml")
            .replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
        // derrière Authelia tout entier: pas de routeur de plus
        c.access = Access::TwoFactor;
        assert!(!compose(&m, &c).contains("vaultwarden-admin"));
    }

    #[test]
    fn plex_compose_is_the_expected_one() {
        let m = load("plex");
        let mut c = ctx(Access::App);
        c.subdomain = "plex".into();
        let expected =
            include_str!("../tests/expected/plex/compose.yml").replace("<version>", m.version());
        assert_eq!(compose(&m, &c), expected);
    }

    #[test]
    fn tautulli_autobrr_maintainerr_composes() {
        for app in ["tautulli", "autobrr", "maintainerr", "wg-easy"] {
            let m = load(app);
            let mut c = ctx(Access::TwoFactor);
            c.subdomain = app.into();
            let expected = std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/expected/{app}/compose.yml")),
            )
            .unwrap()
            .replace("<version>", m.version());
            assert_eq!(compose(&m, &c), expected, "{app}");
        }
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
    fn json_settings_keep_the_rest() {
        let v = |k: &str, x: &str| (k.to_string(), x.to_string());
        // un settings.json de Seerr (une espace d'indentation): l'ordre et le reste gardés
        let seerr = "{\n \"clientId\": \"abc\",\n \"network\": {\n  \"csrfProtection\": false,\n  \"trustProxy\": false\n },\n \"public\": {\n  \"initialized\": false\n }\n}";
        let out = merge_json(Some(seerr), &[v("network/trustProxy", "true")]).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["network"]["trustProxy"], true);
        assert_eq!(doc["network"]["csrfProtection"], false);
        assert_eq!(doc["clientId"], "abc");
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["clientId", "network", "public"]);
        // déjà réglé: le texte de l'app, tel quel
        let set = seerr.replace("\"trustProxy\": false", "\"trustProxy\": true");
        assert_eq!(
            merge_json(Some(&set), &[v("network/trustProxy", "true")]).unwrap(),
            set
        );
        // un objet qui manque est créé, du texte reste du texte
        let out = merge_json(Some("{}"), &[v("a/b", "x")]).unwrap();
        assert_eq!(out, "{\n  \"a\": {\n    \"b\": \"x\"\n  }\n}\n");
        assert!(merge_json(Some("{\"a\": 1}"), &[v("a/b", "x")]).is_err());
        assert!(merge_json(Some("pas du json"), &[v("a", "x")]).is_err());
    }

    /// Le config.yaml de code-server écrit avant le premier démarrage (D131).
    #[test]
    fn code_server_settings_are_the_expected_ones() {
        let m = load("code-server");
        let c = ctx(Access::TwoFactor);
        let st = m.settings.unwrap();
        assert_eq!(st.format, "yaml");
        let values: Vec<(String, String)> = st
            .values
            .iter()
            .map(|(k, v)| (k.clone(), fill(v, &m.name, &c)))
            .collect();
        let out = merge_yaml(None, &values).unwrap();
        assert_eq!(
            out,
            include_str!("../tests/expected/code-server/config.yaml")
        );
        // relancé: rien ne change
        assert_eq!(merge_yaml(Some(&out), &values).unwrap(), out);
    }

    #[test]
    fn yaml_settings_keep_the_rest() {
        let v = |k: &str, x: &str| (k.to_string(), x.to_string());
        // le config.yaml que code-server écrit à son premier démarrage: le mot de passe gardé,
        // auth changé, cert reste un booléen
        let cs = "bind-addr: 127.0.0.1:8080\nauth: password\npassword: abc123\ncert: false\n";
        let out = merge_yaml(
            Some(cs),
            &[v("bind-addr", "0.0.0.0:8080"), v("auth", "none")],
        )
        .unwrap();
        assert_eq!(
            out,
            "bind-addr: 0.0.0.0:8080\nauth: none\npassword: abc123\ncert: false\n"
        );
        // déjà réglé: le texte de l'app, tel quel
        let set = "# écrit par l'app\nauth: none\n";
        assert_eq!(merge_yaml(Some(set), &[v("auth", "none")]).unwrap(), set);
        // un fichier vide, ou une liste au lieu de clés
        assert_eq!(
            merge_yaml(Some(""), &[v("auth", "none")]).unwrap(),
            "auth: none\n"
        );
        assert!(merge_yaml(Some("- a\n"), &[v("auth", "none")]).is_err());
        assert!(merge_yaml(Some("a: [\n"), &[v("auth", "none")]).is_err());
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
