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
/// réglages: l'app complète le reste au démarrage.
pub fn merge_xml(existing: Option<&str>, values: &[(String, String)]) -> Result<String, String> {
    let mut text = match existing {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => "<Config>\n</Config>\n".to_string(),
    };
    if !text.contains("</Config>") {
        return Err("no </Config> in the file: not the expected format".into());
    }
    for (key, value) in values {
        let v = xml_escape(value);
        let open = format!("<{key}>");
        let close = format!("</{key}>");
        if let Some(i) = text.find(&open) {
            let start = i + open.len();
            let end = text[start..]
                .find(&close)
                .ok_or(format!("<{key}> without </{key}>"))?
                + start;
            text.replace_range(start..end, &v);
        } else if let Some((i, len)) = [format!("<{key} />"), format!("<{key}/>")]
            .iter()
            .find_map(|e| text.find(e.as_str()).map(|i| (i, e.len())))
        {
            text.replace_range(i..i + len, &format!("{open}{v}{close}"));
        } else {
            let at = text.rfind("</Config>").unwrap_or(text.len());
            let line = if text[..at].ends_with('\n') { "" } else { "\n" };
            text.insert_str(at, &format!("{line}  {open}{v}{close}\n"));
        }
    }
    Ok(text)
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
            merge_xml(None, &[v("AuthenticationMethod", "External")]).unwrap(),
            "<Config>\n  <AuthenticationMethod>External</AuthenticationMethod>\n</Config>\n"
        );
        // un fichier de Radarr: la clé d'API reste, une valeur change, une vide se remplit, une manque
        let radarr = "<Config>\n  <ApiKey>abc123</ApiKey>\n  <AuthenticationMethod>Forms</AuthenticationMethod>\n  <AllowedHosts></AllowedHosts>\n  <Port>7878</Port>\n</Config>";
        let out = merge_xml(
            Some(radarr),
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
            merge_xml(Some(&out), &[v("AuthenticationMethod", "External")]).unwrap(),
            out
        );
        assert_eq!(
            merge_xml(Some("<Config>\n  <A />\n</Config>"), &[v("A", "x & y")]).unwrap(),
            "<Config>\n  <A>x &amp; y</A>\n</Config>"
        );
        assert!(merge_xml(Some("<Other/>"), &[v("A", "x")]).is_err());
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
        }
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
