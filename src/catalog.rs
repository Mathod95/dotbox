//! Le catalogue (Journal, D41): un manifest YAML par app, dans catalog/<app>.yml à la racine du
//! repo, lu sur GitHub (7.6, github.rs) ou dans un dossier local (--catalog).
//!
//! Un manifest décrit l'app: son image à une version fixe, son port interne, son niveau d'accès,
//! ses variables et ses volumes. dotbox en fait /opt/<app>/compose.yml (engine.rs).

use serde::Deserialize;
use std::path::Path;

/// Noms pris par le core: ni une app ni un sous-domaine ne peut les utiliser.
pub const RESERVED: &[&str] = &[
    "auth",
    "authelia",
    "catalog",
    "core",
    "crowdsec",
    "dotbox",
    "socket-proxy",
    "traefik",
];

/// Les valeurs que dotbox remplit à l'installation: l'utilisateur (uid, gid), le fuseau horaire,
/// l'adresse de l'app (host: <sous-domaine>.<domaine>), son nom (app, celui du container) et le
/// réseau dotbox (subnet, là où est Traefik).
pub const PLACEHOLDERS: &[&str] = &[
    "{{uid}}",
    "{{gid}}",
    "{{tz}}",
    "{{host}}",
    "{{app}}",
    "{{subnet}}",
];

/// Les quatre niveaux d'accès (Accessing Apps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    TwoFactor,
    OneFactor,
    App,
    Public,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::TwoFactor => "two_factor",
            Access::OneFactor => "one_factor",
            Access::App => "app",
            Access::Public => "public",
        }
    }

    pub fn parse(s: &str) -> Option<Access> {
        [
            Access::TwoFactor,
            Access::OneFactor,
            Access::App,
            Access::Public,
        ]
        .into_iter()
        .find(|a| a.as_str() == s)
    }

    /// L'app passe par Authelia (middleware authelia@docker).
    pub fn behind_authelia(self) -> bool {
        matches!(self, Access::TwoFactor | Access::OneFactor)
    }
}

/// Le fichier tel qu'il est écrit.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: String,
    description: String,
    min_dotbox: String,
    image: String,
    port: u16,
    access: Access,
    #[serde(default)]
    health: Option<String>,
    #[serde(default)]
    env: serde_yaml_ng::Mapping,
    #[serde(default)]
    volumes: Vec<String>,
    /// dossiers à créer sous /data (« media/movies »)
    #[serde(default)]
    data: Vec<String>,
    #[serde(default)]
    settings: Option<RawSettings>,
    /// ports publiés, seulement ce qui n'est pas du web (D44): « 56881/tcp »
    #[serde(default)]
    ports: Vec<String>,
    /// l'utilisateur du container, pour les images sans PUID ni PGID (7.11): « {{uid}}:{{gid}} »
    #[serde(default)]
    user: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettings {
    file: String,
    format: String,
    values: serde_yaml_ng::Mapping,
}

/// Des réglages que dotbox écrit dans le fichier de configuration de l'app, avant son premier
/// démarrage et à chaque install, sans toucher aux autres (config.xml de Radarr, par exemple).
#[derive(Debug)]
pub struct Settings {
    /// chemin sous /opt/<app> (« config/config.xml »)
    pub file: String,
    /// « xml »: des éléments directement sous la racine (<Config><Clé>valeur</Clé></Config>).
    /// « ini »: un fichier de Qt (qBittorrent.conf), la clé « Section/Clé/Sous-clé » va dans
    /// [Section] en « Clé\Sous-clé ».
    pub format: String,
    /// dans l'ordre du fichier
    pub values: Vec<(String, String)>,
}

/// Un volume: dossier du serveur, chemin dans le container, lecture seule ou non.
#[derive(Debug, PartialEq, Eq)]
pub struct Volume {
    pub host: String,
    pub container: String,
    pub read_only: bool,
}

/// Un port publié sur le serveur, le même numéro dans le container (D44).
#[derive(Debug, PartialEq, Eq)]
pub struct Port {
    pub number: u16,
    /// « tcp » ou « udp »
    pub proto: String,
}

/// Un manifest vérifié.
#[derive(Debug)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub min_dotbox: String,
    pub image: String,
    pub port: u16,
    pub access: Access,
    pub health: Option<String>,
    /// dans l'ordre du fichier
    pub env: Vec<(String, String)>,
    pub volumes: Vec<Volume>,
    pub data: Vec<String>,
    pub settings: Option<Settings>,
    pub ports: Vec<Port>,
    /// le container tourne avec l'utilisateur de dotbox.yml (user: dans le compose)
    pub run_as_user: bool,
}

impl Manifest {
    /// L'app monte /data (médias et téléchargements).
    pub fn uses_data(&self) -> bool {
        self.volumes.iter().any(|v| v.host == "/data")
    }

    /// La version de l'image (« 3.0.2.5780-ls169 »).
    pub fn version(&self) -> &str {
        self.image.rsplit_once(':').map(|(_, t)| t).unwrap_or("")
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 30
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-')
}

/// « 0.0.3 » en (0, 0, 3).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let out = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(out)
}

fn volume(app: &str, spec: &str) -> Result<Volume, String> {
    let parts: Vec<&str> = spec.split(':').collect();
    let (src, dst, read_only) = match parts.as_slice() {
        [s, d] => (*s, *d, false),
        [s, d, "ro"] => (*s, *d, true),
        _ => {
            return Err(format!(
                "volume {spec:?}: expected name:/path or name:/path:ro"
            ));
        }
    };
    if !dst.starts_with('/') || dst.contains("..") {
        return Err(format!(
            "volume {spec:?}: the container path must be absolute"
        ));
    }
    let host = if src.starts_with('/') {
        // /data: les médias et les téléchargements (Paths). /opt: les dossiers des apps, pour
        // code-server (D67), /opt/dotbox reste à root seul.
        if src.contains("..") || !(src == "/data" || src.starts_with("/data/") || src == "/opt") {
            return Err(format!(
                "volume {spec:?}: a server path must be /data, under /data, or /opt"
            ));
        }
        // les mêmes chemins dans le container et sur le serveur: pas de hardlink sinon (Paths),
        // et un chemin lu dans l'app est le même sur le serveur
        if dst != src {
            return Err(format!(
                "volume {spec:?}: /data keeps the same path in the container ({src}:{src})"
            ));
        }
        src.to_string()
    } else {
        if src.is_empty()
            || !src
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(format!(
                "volume {spec:?}: {src:?} must be a folder name, like config"
            ));
        }
        format!("/opt/{app}/{src}")
    };
    Ok(Volume {
        host,
        container: dst.to_string(),
        read_only,
    })
}

/// Lit et vérifie un manifest.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let raw: Raw = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
    let name = raw.name;
    if !valid_name(&name) {
        return Err(format!(
            "name {name:?}: lowercase letters, digits and -, 30 characters at most"
        ));
    }
    if RESERVED.contains(&name.as_str()) {
        return Err(format!("name {name:?} is taken by the core"));
    }
    if parse_version(&raw.min_dotbox).is_none() {
        return Err(format!(
            "min_dotbox {:?}: expected a version like 0.0.3",
            raw.min_dotbox
        ));
    }
    check_image(&raw.image)?;
    if raw.port == 0 {
        return Err("port: the port the app listens on in its container".into());
    }
    // un chemin (/ping): dotbox ajoute un healthcheck qui l'interroge avec curl. « image »:
    // l'image a déjà le sien (HEALTHCHECK de son Dockerfile), dotbox l'attend sans en ajouter.
    if let Some(h) = raw.health.as_ref().filter(|h| h.as_str() != "image") {
        if !h.starts_with('/')
            || !h
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_.-".contains(c))
        {
            return Err(format!("health {h:?}: a path like /ping, or image"));
        }
    }
    let mut env = Vec::new();
    for (k, v) in &raw.env {
        let key = k.as_str().ok_or("env: every name must be text")?;
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!("env {key:?}: uppercase letters, digits and _"));
        }
        let value = match v {
            serde_yaml_ng::Value::String(s) => s.clone(),
            serde_yaml_ng::Value::Number(n) => n.to_string(),
            serde_yaml_ng::Value::Bool(b) => b.to_string(),
            _ => return Err(format!("env {key}: the value must be text")),
        };
        check_placeholders(&format!("env {key}"), &value)?;
        env.push((key.to_string(), value));
    }
    let volumes = raw
        .volumes
        .iter()
        .map(|v| volume(&name, v))
        .collect::<Result<Vec<_>, _>>()?;
    if !raw.data.is_empty() && !volumes.iter().any(|v| v.host == "/data") {
        return Err("data: needs the volume /data:/data".into());
    }
    for d in &raw.data {
        if !relative_path(d, false) {
            return Err(format!(
                "data {d:?}: a folder under /data, like media/movies"
            ));
        }
    }
    // seulement l'utilisateur de dotbox.yml: jamais root, jamais un autre compte
    let run_as_user = match raw.user.as_deref() {
        None => false,
        Some("{{uid}}:{{gid}}") => true,
        Some(u) => return Err(format!("user {u:?}: only \"{{{{uid}}}}:{{{{gid}}}}\"")),
    };
    let mut ports: Vec<Port> = Vec::new();
    for spec in &raw.ports {
        let p = port(spec)?;
        if p.number == raw.port || p.number == 80 || p.number == 443 {
            return Err(format!(
                "ports {spec:?}: only ports that are not web, the web goes through Traefik (D44)"
            ));
        }
        if ports.contains(&p) {
            return Err(format!("ports {spec:?}: listed twice"));
        }
        ports.push(p);
    }
    let settings = match raw.settings {
        None => None,
        Some(st) => {
            if !settings_file(&st.file) {
                return Err(format!(
                    "settings: file {:?} must be under /opt/{name}, like config/config.xml",
                    st.file
                ));
            }
            let ini = match st.format.as_str() {
                "xml" => false,
                "ini" => true,
                other => return Err(format!("settings: format {other:?}, xml or ini")),
            };
            let mut values = Vec::new();
            for (k, v) in &st.values {
                let key = k.as_str().ok_or("settings: every name must be text")?;
                let word = |w: &str| {
                    w.starts_with(|c: char| c.is_ascii_alphabetic())
                        && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                };
                if ini && !(key.contains('/') && key.split('/').all(word)) {
                    return Err(format!(
                        "settings {key:?}: Section/Key, letters, digits and _ (format ini)"
                    ));
                }
                if !ini && !word(key) {
                    return Err(format!("settings {key:?}: letters, digits and _"));
                }
                let value = v
                    .as_str()
                    .ok_or(format!("settings {key}: the value must be text"))?
                    .to_string();
                check_placeholders(&format!("settings {key}"), &value)?;
                values.push((key.to_string(), value));
            }
            Some(Settings {
                file: st.file,
                format: st.format,
                values,
            })
        }
    };
    Ok(Manifest {
        name,
        description: raw.description,
        min_dotbox: raw.min_dotbox,
        image: raw.image,
        port: raw.port,
        access: raw.access,
        health: raw.health,
        env,
        volumes,
        data: raw.data,
        settings,
        ports,
        run_as_user,
    })
}

/// « 56881/tcp » ou « 56881/udp ».
fn port(spec: &str) -> Result<Port, String> {
    let bad = || format!("ports {spec:?}: expected a port and its protocol, like 56881/tcp");
    let (n, proto) = spec.split_once('/').ok_or_else(bad)?;
    let number: u16 = n.parse().map_err(|_| bad())?;
    if number == 0 || !(proto == "tcp" || proto == "udp") {
        return Err(bad());
    }
    Ok(Port {
        number,
        proto: proto.to_string(),
    })
}

/// Seuls les {{...}} connus de dotbox.
fn check_placeholders(what: &str, value: &str) -> Result<(), String> {
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        let end = rest[start..]
            .find("}}")
            .ok_or(format!("{what}: {{{{ without }}}}"))?;
        let p = &rest[start..start + end + 2];
        if !PLACEHOLDERS.contains(&p) {
            return Err(format!(
                "{what}: unknown {p}, expected one of {}",
                PLACEHOLDERS.join(" ")
            ));
        }
        rest = &rest[start + end + 2..];
    }
    Ok(())
}

/// Un chemin relatif sans « .. »: des segments en minuscules, chiffres, - et _ (et . pour un
/// fichier).
fn relative_path(p: &str, file: bool) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && p.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg.chars().all(|c| {
                    c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || c == '-'
                        || c == '_'
                        || (file && c == '.')
                })
        })
}

/// Le fichier de réglages sous /opt/<app>: comme relative_path, avec les majuscules, puisque
/// c'est l'app qui le nomme (config/qBittorrent/qBittorrent.conf).
fn settings_file(p: &str) -> bool {
    relative_path(&p.to_ascii_lowercase(), true)
}

/// Ce binaire peut-il installer ce manifest?
pub fn supported(min: &str, current: &str) -> bool {
    match (parse_version(min), parse_version(current)) {
        (Some(m), Some(c)) => c >= m,
        _ => false,
    }
}

/// Une image à une version fixe (image:version), jamais latest (D15).
fn check_image(image: &str) -> Result<(), String> {
    match image.rsplit_once(':') {
        Some((repo, tag)) if !repo.is_empty() && !tag.is_empty() && !tag.contains('/') => {
            if tag == "latest" {
                Err(format!("image {image}: a fixed version, never latest"))
            } else {
                Ok(())
            }
        }
        _ => Err(format!(
            "image {image}: needs a fixed version (image:version)"
        )),
    }
}

/// Le manifest d'une app, lu dans un dossier du catalogue.
pub fn load(dir: &Path, app: &str) -> Result<Manifest, String> {
    if !valid_name(app) {
        return Err(format!("{app:?} is not an app name"));
    }
    let path = dir.join(format!("{app}.yml"));
    let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            format!("no app {app} in the catalog ({})", dir.display())
        }
        _ => format!("cannot read {}: {e}", path.display()),
    })?;
    // min_dotbox d'abord: un manifest plus récent peut avoir des champs que ce binaire ne
    // connaît pas, le message doit dire qu'il faut un dotbox plus récent
    let current = env!("CARGO_PKG_VERSION");
    if let Some(min) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        .ok()
        .and_then(|v| v.get("min_dotbox")?.as_str().map(str::to_string))
    {
        if parse_version(&min).is_some() && !supported(&min, current) {
            return Err(format!(
                "{app} needs Dot Box {min} or newer, this is {current}: install a newer Dot Box first"
            ));
        }
    }
    let m = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if m.name != app {
        return Err(format!(
            "{}: its name is {}, not {app}",
            path.display(),
            m.name
        ));
    }
    if !supported(&m.min_dotbox, current) {
        return Err(format!(
            "{app} needs Dot Box {} or newer, this is {current}: install a newer Dot Box first",
            m.min_dotbox
        ));
    }
    Ok(m)
}

/// Un fichier que le core télécharge (le bouncer de CrowdSec): une archive publiée sur GitHub, le
/// binaire qu'elle contient, et leurs empreintes, vérifiées à l'installation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreFile {
    pub version: String,
    pub url: String,
    pub sha256: String,
    /// chemin du binaire dans l'archive
    pub binary: String,
    pub binary_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCore {
    name: String,
    min_dotbox: String,
    images: serde_yaml_ng::Mapping,
    #[serde(default)]
    files: std::collections::BTreeMap<String, CoreFile>,
}

/// Un manifest du core (D64): catalog/core/<nom>.yml. Les versions du core, pas la façon de
/// l'installer, qui reste dans dotbox.
#[derive(Debug)]
pub struct Core {
    pub name: String,
    pub min_dotbox: String,
    /// dans l'ordre du fichier
    pub images: Vec<(String, String)>,
    pub files: std::collections::BTreeMap<String, CoreFile>,
}

impl Core {
    /// Une image du manifest, par son nom (« traefik », « socket-proxy »...).
    pub fn image(&self, key: &str) -> Result<&str, String> {
        self.images
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .ok_or(format!("catalog/core/{}.yml has no image {key}", self.name))
    }

    pub fn file(&self, key: &str) -> Result<&CoreFile, String> {
        self.files
            .get(key)
            .ok_or(format!("catalog/core/{}.yml has no file {key}", self.name))
    }
}

fn is_sha256(h: &str) -> bool {
    h.len() == 64
        && h.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

pub fn parse_core(text: &str) -> Result<Core, String> {
    let raw: RawCore = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
    if !valid_name(&raw.name) {
        return Err(format!(
            "name {:?}: lowercase letters, digits and -",
            raw.name
        ));
    }
    if parse_version(&raw.min_dotbox).is_none() {
        return Err(format!(
            "min_dotbox {:?}: expected a version like 0.0.3",
            raw.min_dotbox
        ));
    }
    let mut images = Vec::new();
    for (k, v) in &raw.images {
        let key = k.as_str().ok_or("images: every name must be text")?;
        let image = v
            .as_str()
            .ok_or(format!("images {key}: the image must be text"))?;
        check_image(image)?;
        images.push((key.to_string(), image.to_string()));
    }
    if images.is_empty() {
        return Err("images: at least one".into());
    }
    for (key, f) in &raw.files {
        if !f.url.starts_with("https://") {
            return Err(format!("files {key}: the url must be https"));
        }
        if !is_sha256(&f.sha256) || !is_sha256(&f.binary_sha256) {
            return Err(format!(
                "files {key}: sha256 values are 64 lowercase hex digits"
            ));
        }
        if f.version.trim().is_empty() || !f.url.contains(&f.version) {
            return Err(format!(
                "files {key}: the url must contain the version {}",
                f.version
            ));
        }
        if f.binary.is_empty()
            || f.binary.starts_with('/')
            || f.binary.split('/').any(|p| p.is_empty() || p == "..")
        {
            return Err(format!("files {key}: binary is a path inside the archive"));
        }
    }
    Ok(Core {
        name: raw.name,
        min_dotbox: raw.min_dotbox,
        images,
        files: raw.files,
    })
}

/// Un manifest du core, lu dans un dossier du catalogue (<dossier>/core/<nom>.yml).
pub fn load_core(dir: &Path, name: &str) -> Result<Core, String> {
    let path = dir.join("core").join(format!("{name}.yml"));
    let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "no {name} in the core of the catalog ({}): a catalog for a newer Dot Box?",
            dir.display()
        ),
        _ => format!("cannot read {}: {e}", path.display()),
    })?;
    let c = parse_core(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if c.name != name {
        return Err(format!(
            "{}: its name is {}, not {name}",
            path.display(),
            c.name
        ));
    }
    let current = env!("CARGO_PKG_VERSION");
    if !supported(&c.min_dotbox, current) {
        return Err(format!(
            "{} needs Dot Box {} or newer, this is {current}: install a newer Dot Box first",
            path.display(),
            c.min_dotbox
        ));
    }
    Ok(c)
}

/// Les apps d'un dossier du catalogue (les manifests à sa racine).
pub fn app_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "yml"))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .filter(|n| n != "catalog")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog")
    }

    /// Chaque manifest du catalogue se lit (la CI le lance à chaque pull request de Renovate).
    #[test]
    fn every_manifest_of_the_catalog_is_valid() {
        let mut n = 0;
        for entry in std::fs::read_dir(catalog_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "yml") {
                let app = path.file_stem().unwrap().to_str().unwrap().to_string();
                load(&catalog_dir(), &app).unwrap_or_else(|e| panic!("{e}"));
                n += 1;
            }
        }
        assert!(n >= 1);
    }

    #[test]
    fn prowlarr() {
        let m = load(&catalog_dir(), "prowlarr").unwrap();
        assert_eq!(m.port, 9696);
        assert_eq!(m.access, Access::TwoFactor);
        assert!(m.version().starts_with("3."));
        assert_eq!(
            m.volumes,
            [Volume {
                host: "/opt/prowlarr/config".into(),
                container: "/config".into(),
                read_only: false
            }]
        );
        assert_eq!(m.env[0], ("PUID".to_string(), "{{uid}}".to_string()));
        assert!(!m.uses_data());
    }

    #[test]
    fn every_core_manifest_is_valid() {
        for name in ["traefik", "authelia", "crowdsec"] {
            load_core(&catalog_dir(), name).unwrap_or_else(|e| panic!("{e}"));
        }
        let t = load_core(&catalog_dir(), "traefik").unwrap();
        assert!(t.image("traefik").unwrap().starts_with("traefik:v"));
        assert!(t.image("socket-proxy").is_ok());
        assert!(t.image("nope").is_err());
        let c = load_core(&catalog_dir(), "crowdsec").unwrap();
        let b = c.file("firewall-bouncer").unwrap();
        assert!(b.url.contains(&format!("/download/{}/", b.version)));
        assert!(app_names(&catalog_dir()).contains(&"radarr".to_string()));
        assert!(!app_names(&catalog_dir()).contains(&"core".to_string()));
    }

    #[test]
    fn core_rules() {
        let ok = "name: x\nmin_dotbox: 0.0.3\nimages:\n  x: a/b:1.0\n";
        assert!(parse_core(ok).is_ok());
        assert!(parse_core(&ok.replace("a/b:1.0", "a/b:latest")).is_err());
        assert!(parse_core(&ok.replace("a/b:1.0", "a/b")).is_err());
        assert!(parse_core("name: x\nmin_dotbox: 0.0.3\nimages: {}\n").is_err());
        let f = |url: &str, sha: &str| {
            format!(
                "{ok}files:\n  b:\n    version: v1\n    url: {url}\n    sha256: {sha}\n    binary: d/b\n    binary_sha256: {}\n",
                "a".repeat(64)
            )
        };
        assert!(parse_core(&f("https://x/v1/b.tgz", &"a".repeat(64))).is_ok());
        assert!(parse_core(&f("http://x/v1/b.tgz", &"a".repeat(64))).is_err());
        assert!(parse_core(&f("https://x/v2/b.tgz", &"a".repeat(64))).is_err());
        assert!(parse_core(&f("https://x/v1/b.tgz", "abc")).is_err());
        assert!(parse_core(&format!("{ok}extra: 1\n")).is_err());
    }

    #[test]
    fn qui() {
        let m = load(&catalog_dir(), "qui").unwrap();
        assert_eq!(m.port, 7476);
        assert_eq!(m.health.as_deref(), Some("image"));
        assert!(!m.uses_data());
        assert_eq!(m.version(), "v1.35.0");
    }

    #[test]
    fn sonarr() {
        let m = load(&catalog_dir(), "sonarr").unwrap();
        assert_eq!(m.port, 8989);
        assert!(m.uses_data());
        assert_eq!(m.data, ["media/tv"]);
        assert_eq!(m.settings.unwrap().values.len(), 3);
    }

    #[test]
    fn radarr() {
        let m = load(&catalog_dir(), "radarr").unwrap();
        assert_eq!(m.port, 7878);
        assert!(m.uses_data());
        assert_eq!(m.data, ["media/movies"]);
        let st = m.settings.unwrap();
        assert_eq!(st.file, "config/config.xml");
        assert_eq!(
            st.values[0],
            ("AuthenticationMethod".to_string(), "External".to_string())
        );
    }

    const OK: &str = "name: demo\ndescription: d\nmin_dotbox: 0.0.3\nimage: demo/demo:1.2\nport: 80\naccess: public\n";

    #[test]
    fn rules() {
        assert!(parse(OK).is_ok());
        let bad = |from: &str, to: &str| parse(&OK.replace(from, to)).unwrap_err();
        assert!(bad("demo/demo:1.2", "demo/demo:latest").contains("never latest"));
        assert!(bad("demo/demo:1.2", "demo/demo").contains("fixed version"));
        assert!(bad("demo/demo:1.2", "registry:5000/demo").contains("fixed version"));
        assert!(bad("name: demo", "name: traefik").contains("core"));
        assert!(bad("name: demo", "name: Demo").contains("lowercase"));
        assert!(bad("access: public", "access: open").contains("unknown variant"));
        assert!(bad("port: 80", "port: 80\nimage2: x").contains("unknown field"));
        let with = |extra: &str| parse(&format!("{OK}{extra}"));
        assert!(with("volumes:\n  - /etc:/etc\n").is_err());
        assert!(with("volumes:\n  - /data/../etc:/x\n").is_err());
        assert!(with("volumes:\n  - ../x:/x\n").is_err());
        assert!(with("volumes:\n  - /data:/data\n  - config:/config:ro\n").is_ok());
        assert!(
            with("env:\n  A: \"{{home}}\"\n")
                .unwrap_err()
                .contains("unknown {{home}}")
        );
        assert!(with("env:\n  a: x\n").is_err());
        assert!(with("health: ping\n").is_err());
        assert!(with("health: image\n").is_ok());
        // /data garde le même chemin dans le container
        assert!(
            with("volumes:\n  - /data:/media\n")
                .unwrap_err()
                .contains("same path")
        );
        assert!(
            with("data:\n  - media/movies\n")
                .unwrap_err()
                .contains("/data:/data")
        );
        let data = "volumes:\n  - /data:/data\ndata:\n";
        assert!(with(&format!("{data}  - media/movies\n")).is_ok());
        assert!(with(&format!("{data}  - ../etc\n")).is_err());
        assert!(with(&format!("{data}  - /media\n")).is_err());
        let st = "settings:\n  file: config/config.xml\n  format: xml\n  values:\n";
        assert!(
            with(&format!(
                "{st}    AllowedHosts: \"{{{{host}}}},{{{{app}}}}\"\n"
            ))
            .is_ok()
        );
        assert!(with(&format!("{st}    A: \"{{{{home}}}}\"\n")).is_err());
        assert!(with(&format!("{st}    <A>: x\n")).is_err());
        assert!(with("settings:\n  file: ../x.xml\n  format: xml\n  values: {}\n").is_err());
        assert!(with("settings:\n  file: config/x.ini\n  format: toml\n  values: {}\n").is_err());
        // ini: Section/Clé, le fichier peut avoir des majuscules (c'est l'app qui le nomme)
        let ini =
            "settings:\n  file: config/qBittorrent/qBittorrent.conf\n  format: ini\n  values:\n";
        assert!(with(&format!("{ini}    WebUI/Port: \"8080\"\n")).is_ok());
        assert!(with(&format!("{ini}    Port: \"8080\"\n")).is_err());
        assert!(with(&format!("{ini}    WebUI//Port: \"8080\"\n")).is_err());
        assert!(with(&format!("{st}    WebUI/Port: \"8080\"\n")).is_err());
        assert!(with("settings:\n  file: Config/../x\n  format: ini\n  values: {}\n").is_err());
        // ports: pas du web (D44)
        assert_eq!(
            with("ports:\n  - 56881/tcp\n  - 56881/udp\n")
                .unwrap()
                .ports,
            [
                Port {
                    number: 56881,
                    proto: "tcp".into()
                },
                Port {
                    number: 56881,
                    proto: "udp".into()
                }
            ]
        );
        assert!(
            with("ports:\n  - 80/tcp\n")
                .unwrap_err()
                .contains("not web")
        );
        assert!(with("ports:\n  - 443/tcp\n").is_err());
        assert!(
            with("ports:\n  - 56881\n")
                .unwrap_err()
                .contains("56881/tcp")
        );
        assert!(with("ports:\n  - 56881/sctp\n").is_err());
        assert!(with("ports:\n  - 0/tcp\n").is_err());
        assert!(
            with("ports:\n  - 56881/tcp\n  - 56881/tcp\n")
                .unwrap_err()
                .contains("twice")
        );
        // user: seulement celui de dotbox.yml (7.11)
        assert!(with("user: \"{{uid}}:{{gid}}\"\n").unwrap().run_as_user);
        assert!(!parse(OK).unwrap().run_as_user);
        assert!(with("user: root\n").is_err());
        assert!(with("user: \"0:0\"\n").is_err());
        // /opt au même chemin (D67), rien d'autre
        assert!(with("volumes:\n  - /opt:/opt\n").is_ok());
        assert!(with("volumes:\n  - /opt:/srv\n").is_err());
        assert!(with("volumes:\n  - /opt/dotbox:/opt/dotbox\n").is_err());
    }

    #[test]
    fn qbittorrent() {
        let m = load(&catalog_dir(), "qbittorrent").unwrap();
        assert_eq!(m.port, 8080);
        assert!(m.uses_data());
        assert_eq!(m.data, ["torrents"]);
        assert_eq!(m.ports.len(), 2);
        let st = m.settings.unwrap();
        assert_eq!(st.format, "ini");
        assert!(
            st.values
                .iter()
                .any(|(k, v)| k == "WebUI/AuthSubnetWhitelist" && v == "{{subnet}}")
        );
    }

    /// Un manifest pour un dotbox plus récent: le message le dit, même s'il a des champs que ce
    /// binaire ne connaît pas.
    #[test]
    fn a_newer_manifest_says_so() {
        let dir = std::env::temp_dir().join(format!("dotbox-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("demo.yml"),
            OK.replace("min_dotbox: 0.0.3", "min_dotbox: 99.0.0") + "future: 1\n",
        )
        .unwrap();
        let e = load(&dir, "demo").unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(e.contains("needs Dot Box 99.0.0"), "{e}");
    }

    #[test]
    fn versions() {
        assert!(supported("0.0.3", "0.0.3"));
        assert!(supported("0.0.3", "0.1.0"));
        assert!(!supported("0.0.3", "0.0.2"));
        assert!(!supported("x", "0.0.3"));
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
    }
}
