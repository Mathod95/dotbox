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
    /// l'outil du healthcheck: curl (par défaut) ou wget, pour les images sans curl (Seerr)
    #[serde(default)]
    health_tool: Option<String>,
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
    /// les arguments donnés à l'image au démarrage, après son entrypoint (le dossier qu'ouvre
    /// code-server, D131): « command: [/opt] »
    #[serde(default)]
    command: Vec<String>,
    /// des clés que dotbox tire au hasard une seule fois et monte en lecture seule dans
    /// /run/secrets/<nom> (la clé de chiffrement de Homarr, D151): jamais dans compose.yml
    #[serde(default)]
    secrets: Vec<String>,
    /// l'assistant de premier démarrage: derrière Authelia tant qu'il n'est pas fini (D71)
    #[serde(default)]
    setup: Option<RawSetup>,
    /// CrowdSec lit les logs du container (D72, 10.9)
    #[serde(default)]
    crowdsec: Option<RawWatch>,
    /// des chemins de l'app derrière Authelia, même au niveau app ou public (/admin)
    #[serde(default)]
    authelia_paths: Vec<String>,
    /// des droits du noyau pour le container: NET_ADMIN seulement (un VPN, D75)
    #[serde(default)]
    cap_add: Vec<String>,
    /// des réglages réseau du noyau, dans le container seulement (le transfert IPv4 d'un VPN)
    #[serde(default)]
    sysctls: serde_yaml_ng::Mapping,
    /// des modules du noyau que dotbox charge sur le serveur (wireguard), au lieu de donner
    /// SYS_MODULE au container
    #[serde(default)]
    kernel_modules: Vec<String>,
    /// ce que dotbox backup laisse de côté dans /opt/<app>, en plus des logs (D97): des caches
    /// qui se refont seuls. Chemins relatifs au dossier de l'app, * pour n'importe quoi.
    #[serde(default)]
    backup_exclude: Vec<String>,
}

/// Ce qu'un manifest peut demander au noyau: une liste fermée, chaque entrée vérifiée.
const CAPS: &[&str] = &["NET_ADMIN"];
const SYSCTLS: &[&str] = &["net.ipv4.ip_forward", "net.ipv4.conf.all.src_valid_mark"];
const MODULES: &[&str] = &["wireguard"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWatch {
    collection: String,
    #[serde(rename = "type")]
    kind: String,
}

/// CrowdSec lit les logs du container de l'app, par l'API Docker (D72, 10.9): `collection`,
/// la collection du hub à installer (« LePresidente/jellyfin »), et `kind`, l'étiquette que lit
/// son parseur (« jellyfin »).
#[derive(Debug)]
pub struct Watch {
    pub collection: String,
    pub kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSetup {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    done: Option<String>,
    #[serde(default)]
    probe: Option<String>,
    #[serde(default)]
    pending: Option<String>,
}

/// L'assistant de premier démarrage d'une app ouverte à Internet (niveaux app et public): tant
/// qu'il n'est pas fini, l'app reste derrière Authelia (D71). Sans lui, n'importe qui pourrait
/// créer le compte admin avant toi (Jellyfin: FirstTimeSetupHandler.cs).
#[derive(Debug, PartialEq, Eq)]
pub enum Setup {
    /// fini quand `file` (sous /opt/<app>) contient `done` (Jellyfin, Seerr)
    File { file: String, done: String },
    /// fini quand la page `probe` ne renvoie plus vers une adresse qui contient `pending`,
    /// demandé à l'app elle-même avec curl (Wizarr: sa base est en SQLite)
    Probe { probe: String, pending: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettings {
    file: String,
    format: String,
    /// format xml: l'élément racine du fichier, Config par défaut
    #[serde(default)]
    root: Option<String>,
    /// le fichier est créé par l'app à son premier démarrage, jamais par dotbox (Seerr)
    #[serde(default)]
    created_by_app: bool,
    values: serde_yaml_ng::Mapping,
}

/// Des réglages que dotbox écrit dans le fichier de configuration de l'app, avant son premier
/// démarrage et à chaque install, sans toucher aux autres (config.xml de Radarr, par exemple).
#[derive(Debug)]
pub struct Settings {
    /// chemin sous /opt/<app> (« config/config.xml »)
    pub file: String,
    /// « xml »: des éléments directement sous la racine (<Config><Clé>valeur</Clé></Config>).
    /// « Clé/Enfant »: un seul élément dans Clé (<KnownProxies><string>valeur</string></KnownProxies>,
    /// une liste d'une valeur pour Jellyfin).
    /// « ini »: un fichier de Qt (qBittorrent.conf), la clé « Section/Clé/Sous-clé » va dans
    /// [Section] en « Clé\Sous-clé ».
    /// « json »: « a/b/c » est la clé c de l'objet b de l'objet a. true et false sont des
    /// booléens, le reste du texte.
    /// « yaml »: des clés du premier niveau (« bind-addr », le config.yaml de code-server).
    /// true et false sont des booléens, le reste du texte.
    pub format: String,
    /// format xml: l'élément racine (« Config », « NetworkConfiguration »)
    pub root: String,
    /// l'app crée le fichier au premier démarrage: dotbox attend qu'il existe, y pose ses
    /// réglages, puis relance l'app (ses migrations refusent un fichier incomplet)
    pub created_by_app: bool,
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
    /// wget au lieu de curl pour le healthcheck
    pub health_wget: bool,
    /// dans l'ordre du fichier
    pub env: Vec<(String, String)>,
    pub volumes: Vec<Volume>,
    pub data: Vec<String>,
    pub settings: Option<Settings>,
    pub ports: Vec<Port>,
    /// le container tourne avec l'utilisateur de dotbox.yml (user: dans le compose)
    pub run_as_user: bool,
    /// les arguments donnés à l'image au démarrage (command: dans le compose)
    pub command: Vec<String>,
    /// les secrets de l'app, montés dans /run/secrets/<nom> (D151)
    pub secrets: Vec<String>,
    pub setup: Option<Setup>,
    pub crowdsec: Option<Watch>,
    /// des chemins derrière Authelia, en plus du niveau de l'app (/admin de Vaultwarden)
    pub authelia_paths: Vec<String>,
    pub cap_add: Vec<String>,
    /// dans l'ordre du fichier
    pub sysctls: Vec<(String, String)>,
    pub kernel_modules: Vec<String>,
    /// relatifs à /opt/<app> (D97)
    pub backup_exclude: Vec<String>,
}

/// Un dossier du serveur dans /data (les médias et les téléchargements).
pub fn in_data(host: &str) -> bool {
    host == "/data" || host.starts_with("/data/")
}

impl Manifest {
    /// L'app monte /data ou un dossier dedans (/data/media pour Jellyfin).
    pub fn uses_data(&self) -> bool {
        self.volumes.iter().any(|v| in_data(&v.host))
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
    let health_wget = match raw.health_tool.as_deref() {
        None | Some("curl") => false,
        Some("wget") => true,
        Some(t) => return Err(format!("health_tool {t:?}: curl or wget")),
    };
    if health_wget && !raw.health.as_ref().is_some_and(|h| h.starts_with('/')) {
        return Err("health_tool: only with a health path, like /ping".into());
    }
    let mut env = Vec::new();
    for (k, v) in &raw.env {
        let key = k.as_str().ok_or("env: every name must be text")?;
        // une majuscule d'abord, puis des lettres, des chiffres et _ (JELLYFIN_PublishedServerUrl)
        if !key.starts_with(|c: char| c.is_ascii_uppercase())
            || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(format!(
                "env {key:?}: an uppercase letter first, then letters, digits and _"
            ));
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
    for arg in &raw.command {
        if arg.is_empty() || arg.chars().any(char::is_control) {
            return Err(format!(
                "command {arg:?}: one argument per line, not empty, on a single line"
            ));
        }
        check_placeholders(&format!("command {arg}"), arg)?;
    }
    let mut seen = Vec::new();
    for s in &raw.secrets {
        if s.is_empty()
            || s.len() > 30
            || !s.starts_with(|c: char| c.is_ascii_lowercase())
            || !s
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!(
                "secrets {s:?}: lowercase letters, digits and _, 30 characters at most"
            ));
        }
        if seen.contains(&s) {
            return Err(format!("secrets {s:?}: listed twice"));
        }
        seen.push(s);
    }
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
            let (ini, json, yaml) = match st.format.as_str() {
                "xml" => (false, false, false),
                "ini" => (true, false, false),
                "json" => (false, true, false),
                "yaml" => (false, false, true),
                other => {
                    return Err(format!(
                        "settings: format {other:?}, xml, ini, json or yaml"
                    ));
                }
            };
            let word = |w: &str| {
                w.starts_with(|c: char| c.is_ascii_alphabetic())
                    && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            };
            let root = match (&st.root, ini || json || yaml) {
                (None, _) => "Config".to_string(),
                (Some(r), false) if word(r) => r.clone(),
                (Some(r), false) => {
                    return Err(format!("settings: root {r:?}, letters, digits and _"));
                }
                (Some(_), true) => return Err("settings: root is only for the format xml".into()),
            };
            let mut values = Vec::new();
            for (k, v) in &st.values {
                let key = k.as_str().ok_or("settings: every name must be text")?;
                if ini && !(key.contains('/') && key.split('/').all(word)) {
                    return Err(format!(
                        "settings {key:?}: Section/Key, letters, digits and _ (format ini)"
                    ));
                }
                // une clé du premier niveau, avec des tirets (« bind-addr »)
                if yaml
                    && !(key.starts_with(|c: char| c.is_ascii_alphabetic())
                        && key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                {
                    return Err(format!(
                        "settings {key:?}: a top-level key, letters, digits, _ and - (format yaml)"
                    ));
                }
                let parts: Vec<&str> = key.split('/').collect();
                if json && !parts.iter().all(|w| word(w)) {
                    return Err(format!(
                        "settings {key:?}: a/b/c, letters, digits and _ (format json)"
                    ));
                }
                if !ini && !json && !yaml && !(parts.len() <= 2 && parts.iter().all(|w| word(w))) {
                    return Err(format!(
                        "settings {key:?}: Key or Key/Child, letters, digits and _ (format xml)"
                    ));
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
                root,
                created_by_app: st.created_by_app,
                values,
            })
        }
    };
    let setup = match raw.setup {
        None => None,
        Some(s) => {
            if raw.access.behind_authelia() {
                return Err(
                    "setup: only for the levels app and public, Authelia already protects the others"
                        .into(),
                );
            }
            match (s.file, s.done, s.probe, s.pending) {
                (Some(file), Some(done), None, None) => {
                    if !settings_file(&file) {
                        return Err(format!(
                            "setup: file {file:?} must be under /opt/{name}, like config/system.xml"
                        ));
                    }
                    if done.trim().is_empty() {
                        return Err(
                            "setup: done, the text the app writes once its setup is done".into(),
                        );
                    }
                    Some(Setup::File { file, done })
                }
                (None, None, Some(probe), Some(pending)) => {
                    if !probe.starts_with('/') || pending.trim().is_empty() {
                        return Err("setup: probe, a path like /, and pending, a part of the address it redirects to while the setup is not done, like /setup".into());
                    }
                    if health_wget {
                        return Err("setup: probe uses curl, not with health_tool: wget".into());
                    }
                    Some(Setup::Probe { probe, pending })
                }
                _ => {
                    return Err("setup: either file and done, or probe and pending".into());
                }
            }
        }
    };
    let crowdsec = match raw.crowdsec {
        None => None,
        Some(w) => {
            let part = |p: &str| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            };
            if !w
                .collection
                .split_once('/')
                .is_some_and(|(a, b)| part(a) && part(b))
            {
                return Err(format!(
                    "crowdsec: collection {:?}, a hub collection like LePresidente/jellyfin",
                    w.collection
                ));
            }
            if !part(&w.kind) {
                return Err(format!(
                    "crowdsec: type {:?}, the label its parser reads, like jellyfin",
                    w.kind
                ));
            }
            Some(Watch {
                collection: w.collection,
                kind: w.kind,
            })
        }
    };
    for p in &raw.authelia_paths {
        if !p.starts_with('/')
            || p.len() < 2
            || p.ends_with('/')
            || p.contains("//")
            || !p
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "/-_".contains(c))
        {
            return Err(format!(
                "authelia_paths {p:?}: a path like /admin, lowercase letters, digits, - and _"
            ));
        }
    }
    for c in &raw.cap_add {
        if !CAPS.contains(&c.as_str()) {
            return Err(format!("cap_add {c:?}: only {}", CAPS.join(", ")));
        }
    }
    let mut sysctls = Vec::new();
    for (k, v) in &raw.sysctls {
        let key = k.as_str().ok_or("sysctls: every name must be text")?;
        if !SYSCTLS.contains(&key) {
            return Err(format!("sysctls {key:?}: only {}", SYSCTLS.join(", ")));
        }
        let value = v
            .as_str()
            .filter(|v| *v == "0" || *v == "1")
            .ok_or(format!("sysctls {key}: \"0\" or \"1\""))?;
        sysctls.push((key.to_string(), value.to_string()));
    }
    for m in &raw.kernel_modules {
        if !MODULES.contains(&m.as_str()) {
            return Err(format!("kernel_modules {m:?}: only {}", MODULES.join(", ")));
        }
    }
    for e in &raw.backup_exclude {
        if e.is_empty() || e.starts_with('/') || e.split('/').any(|p| p == "..") {
            return Err(format!(
                "backup_exclude {e:?}: a path inside the app's folder, like cache or config/log"
            ));
        }
    }
    if !raw.authelia_paths.is_empty() && raw.access.behind_authelia() {
        return Err("authelia_paths: only for the levels app and public, the whole app is already behind Authelia".into());
    }
    Ok(Manifest {
        name,
        description: raw.description,
        min_dotbox: raw.min_dotbox,
        image: raw.image,
        port: raw.port,
        access: raw.access,
        health: raw.health,
        health_wget,
        env,
        volumes,
        data: raw.data,
        settings,
        ports,
        run_as_user,
        command: raw.command,
        secrets: raw.secrets,
        setup,
        crowdsec,
        authelia_paths: raw.authelia_paths,
        cap_add: raw.cap_add,
        sysctls,
        kernel_modules: raw.kernel_modules,
        backup_exclude: raw.backup_exclude,
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
    // des espaces aussi (Plex: « Application Support/Plex Media Server »): dotbox ouvre ces
    // fichiers lui-même, jamais par un shell
    p.split('/')
        .all(|seg| !seg.starts_with(' ') && !seg.ends_with(' '))
        && relative_path(&p.to_ascii_lowercase().replace(' ', "_"), true)
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
    /// le binaire à garder, son chemin dans l'archive et son empreinte (le bouncer pare-feu)
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub binary_sha256: Option<String>,
    /// ou le dossier de l'archive à garder tel quel (le plugin de Traefik, 10.7)
    #[serde(default)]
    pub dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCore {
    name: String,
    min_dotbox: String,
    #[serde(default)]
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
    if images.is_empty() && raw.files.is_empty() {
        return Err("images or files: at least one".into());
    }
    for (key, f) in &raw.files {
        if !f.url.starts_with("https://") {
            return Err(format!("files {key}: the url must be https"));
        }
        if !is_sha256(&f.sha256) || f.binary_sha256.as_deref().is_some_and(|h| !is_sha256(h)) {
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
        let inside = |p: &str| {
            !p.is_empty() && !p.starts_with('/') && p.split('/').all(|s| !s.is_empty() && s != "..")
        };
        match (&f.binary, &f.binary_sha256, &f.dir) {
            (Some(b), Some(_), None) if inside(b) => {}
            (None, None, Some(d)) if inside(d) => {}
            _ => {
                return Err(format!(
                    "files {key}: either binary and binary_sha256 (paths inside the archive), or dir"
                ));
            }
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
    let current = env!("CARGO_PKG_VERSION");
    // min_dotbox d'abord, comme pour les apps (load)
    if let Some(min) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        .ok()
        .and_then(|v| v.get("min_dotbox")?.as_str().map(str::to_string))
    {
        if parse_version(&min).is_some() && !supported(&min, current) {
            return Err(format!(
                "{} needs Dot Box {min} or newer, this is {current}: install a newer Dot Box first",
                path.display()
            ));
        }
    }
    let c = parse_core(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if c.name != name {
        return Err(format!(
            "{}: its name is {}, not {name}",
            path.display(),
            c.name
        ));
    }
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
        for name in ["traefik", "authelia", "crowdsec", "crowdsec-plugin"] {
            load_core(&catalog_dir(), name).unwrap_or_else(|e| panic!("{e}"));
        }
        let t = load_core(&catalog_dir(), "traefik").unwrap();
        assert!(t.image("traefik").unwrap().starts_with("traefik:v"));
        assert!(t.image("socket-proxy").is_ok());
        assert!(t.image("nope").is_err());
        let c = load_core(&catalog_dir(), "crowdsec").unwrap();
        let b = c.file("firewall-bouncer").unwrap();
        assert!(b.url.contains(&format!("/download/{}/", b.version)));
        let p = load_core(&catalog_dir(), "crowdsec-plugin").unwrap();
        let p = p.file("traefik-plugin").unwrap();
        assert_eq!(
            p.dir.as_deref(),
            Some(
                format!(
                    "crowdsec-bouncer-traefik-plugin-{}",
                    p.version.trim_start_matches('v')
                )
                .as_str()
            )
        );
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
        // un dossier de l'archive au lieu d'un binaire (le plugin de Traefik), sans image
        let plugin = format!(
            "name: p\nmin_dotbox: 0.0.4\nfiles:\n  p:\n    version: v1\n    url: https://x/v1.tar.gz\n    sha256: {}\n    dir: p-1\n",
            "a".repeat(64)
        );
        assert!(parse_core(&plugin).is_ok());
        assert!(parse_core(&plugin.replace("dir: p-1", "dir: ../p")).is_err());
        assert!(parse_core(&plugin.replace("    dir: p-1\n", "")).is_err());
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
        // la forme seulement: Renovate change la version (Q73)
        let v = m.version().strip_prefix('v').unwrap_or_default();
        assert!(
            parse_version(v).is_some(),
            "qui: {} is not v<x.y.z>",
            m.version()
        );
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
        // yaml: des clés du premier niveau, avec des tirets (code-server, D131), sans root
        let yaml = "settings:\n  file: config/.config/code-server/config.yaml\n  format: yaml\n  values:\n";
        assert!(with(&format!("{yaml}    bind-addr: \"0.0.0.0:8080\"\n")).is_ok());
        assert!(with(&format!("{yaml}    a/b: x\n")).is_err());
        assert!(with(&format!("{yaml}    -a: x\n")).is_err());
        assert!(with(&format!("{yaml}    a b: x\n")).is_err());
        assert!(
            with(
                "settings:\n  file: config/c.yaml\n  format: yaml\n  root: X\n  values:\n    a: x\n"
            )
            .is_err()
        );
        // secrets: des noms simples, chacun une seule fois (D151)
        assert_eq!(
            with("secrets:\n  - encryption_key\n").unwrap().secrets,
            ["encryption_key"]
        );
        assert!(with("secrets:\n  - Key\n").is_err());
        assert!(with("secrets:\n  - a/b\n").is_err());
        assert!(with("secrets:\n  - k\n  - k\n").is_err());
        // command: les arguments de l'image, un par ligne, les {{...}} connus seulement
        assert_eq!(with("command:\n  - /opt\n").unwrap().command, ["/opt"]);
        assert!(with("command:\n  - \"--app={{app}}\"\n").is_ok());
        assert!(with("command:\n  - \"{{home}}\"\n").is_err());
        assert!(with("command:\n  - \"\"\n").is_err());
        assert!(with("command:\n  - \"a\\nb\"\n").is_err());
        // ini: Section/Clé, le fichier peut avoir des majuscules (c'est l'app qui le nomme)
        let ini =
            "settings:\n  file: config/qBittorrent/qBittorrent.conf\n  format: ini\n  values:\n";
        assert!(with(&format!("{ini}    WebUI/Port: \"8080\"\n")).is_ok());
        assert!(with(&format!("{ini}    Port: \"8080\"\n")).is_err());
        assert!(with(&format!("{ini}    WebUI//Port: \"8080\"\n")).is_err());
        // xml: Clé/Enfant est un élément dans un autre (Jellyfin), pas une section
        assert!(with(&format!("{st}    WebUI/Port: \"8080\"\n")).is_ok());
        assert!(with(&format!("{st}    A/B/C: \"8080\"\n")).is_err());
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

    #[test]
    fn jellyfin() {
        let m = load(&catalog_dir(), "jellyfin").unwrap();
        assert_eq!(m.port, 8096);
        assert_eq!(m.access, Access::App);
        assert!(m.run_as_user);
        // seulement /data/media, en lecture seule: dotbox prépare quand même /data
        assert!(m.uses_data());
        assert!(
            m.volumes
                .iter()
                .any(|v| v.host == "/data/media" && v.read_only)
        );
        assert!(m.data.is_empty());
        let st = m.settings.unwrap();
        assert_eq!(
            (st.format.as_str(), st.root.as_str()),
            ("xml", "NetworkConfiguration")
        );
        assert_eq!(
            st.values,
            [("KnownProxies/string".to_string(), "{{subnet}}".to_string())]
        );
        let w = m.crowdsec.unwrap();
        assert_eq!(
            (w.collection.as_str(), w.kind.as_str()),
            ("LePresidente/jellyfin", "jellyfin")
        );
        assert_eq!(
            m.setup.unwrap(),
            Setup::File {
                file: "config/config/system.xml".into(),
                done: "<IsStartupWizardCompleted>true</IsStartupWizardCompleted>".into()
            }
        );
    }

    #[test]
    fn seerr() {
        let m = load(&catalog_dir(), "seerr").unwrap();
        assert_eq!((m.port, m.access), (5055, Access::App));
        assert!(m.health_wget && m.run_as_user);
        let st = m.settings.unwrap();
        assert!(st.created_by_app);
        assert_eq!(st.format, "json");
        assert_eq!(
            st.values,
            [("network/trustProxy".to_string(), "true".to_string())]
        );
        assert!(
            matches!(m.setup.unwrap(), Setup::File { done, .. } if done == "\"initialized\": true")
        );
    }

    #[test]
    fn wizarr() {
        let m = load(&catalog_dir(), "wizarr").unwrap();
        assert_eq!((m.port, m.access), (5690, Access::Public));
        assert_eq!(
            m.setup.unwrap(),
            Setup::Probe {
                probe: "/".into(),
                pending: "/setup".into()
            }
        );
        assert!(m.crowdsec.is_none());
    }

    #[test]
    fn vaultwarden() {
        let m = load(&catalog_dir(), "vaultwarden").unwrap();
        assert_eq!((m.port, m.access), (8080, Access::App));
        assert_eq!(m.authelia_paths, ["/admin"]);
        assert!(m.run_as_user && m.setup.is_none());
        assert!(m.env.contains(&("SIGNUPS_ALLOWED".into(), "false".into())));
    }

    #[test]
    fn plex() {
        let m = load(&catalog_dir(), "plex").unwrap();
        assert_eq!((m.port, m.access), (32400, Access::App));
        assert!(
            m.volumes
                .iter()
                .any(|v| v.host == "/data/media" && v.read_only)
        );
        assert!(
            matches!(m.setup, Some(Setup::File { ref done, .. }) if done == "PlexOnlineToken=\"")
        );
        assert!(m.crowdsec.is_none());
    }

    #[test]
    fn wg_easy() {
        let m = load(&catalog_dir(), "wg-easy").unwrap();
        assert_eq!((m.port, m.access), (51821, Access::TwoFactor));
        assert_eq!(m.cap_add, ["NET_ADMIN"]);
        assert_eq!(m.kernel_modules, ["wireguard"]);
        assert_eq!(m.sysctls.len(), 2);
        assert_eq!(m.ports.len(), 1);
    }

    #[test]
    fn setup_and_xml_rules() {
        let with = |extra: &str| parse(&format!("{OK}{extra}"));
        let setup = "setup:\n  file: config/system.xml\n  done: <Done>true</Done>\n";
        assert!(with(setup).unwrap().setup.is_some());
        assert!(with("setup:\n  file: ../system.xml\n  done: x\n").is_err());
        assert!(with("setup:\n  file: config/system.xml\n  done: \" \"\n").is_err());
        // ou en demandant à l'app (Wizarr)
        assert!(with("setup:\n  probe: /\n  pending: /setup\n").is_ok());
        assert!(with("setup:\n  probe: x\n  pending: /setup\n").is_err());
        assert!(with("setup:\n  file: config/s.xml\n  probe: /\n  pending: /setup\n").is_err());
        assert!(with("setup:\n  probe: /\n").is_err());
        // des espaces dans le chemin (Plex), mais pas au bord d'un dossier
        assert!(
            with("setup:\n  file: config/Plex Media Server/Preferences.xml\n  done: x\n").is_ok()
        );
        assert!(with("setup:\n  file: config/ x/p.xml\n  done: x\n").is_err());
        // derrière Authelia, l'assistant est déjà protégé
        let tf = parse(&format!(
            "{}{setup}",
            OK.replace("access: public", "access: two_factor")
        ));
        assert!(
            tf.unwrap_err()
                .contains("only for the levels app and public")
        );
        // xml: une racine au choix, Clé ou Clé/Enfant, pas plus profond
        let st = "settings:\n  file: config/network.xml\n  format: xml\n";
        let ok = with(&format!(
            "{st}  root: NetworkConfiguration\n  values:\n    KnownProxies/string: x\n"
        ))
        .unwrap();
        assert_eq!(ok.settings.unwrap().root, "NetworkConfiguration");
        assert_eq!(
            with(&format!("{st}  values:\n    A: x\n"))
                .unwrap()
                .settings
                .unwrap()
                .root,
            "Config"
        );
        assert!(with(&format!("{st}  values:\n    A/B/C: x\n")).is_err());
        assert!(with(&format!("{st}  root: <x>\n  values:\n    A: x\n")).is_err());
        assert!(
            with(
                "settings:\n  file: config/x.ini\n  format: ini\n  root: X\n  values:\n    S/K: x\n"
            )
            .is_err()
        );
        // crowdsec: une collection du hub et l'étiquette de son parseur
        assert!(with("crowdsec:\n  collection: a/b\n  type: b\n").is_ok());
        assert!(with("crowdsec:\n  collection: b\n  type: b\n").is_err());
        assert!(with("crowdsec:\n  collection: a/b c\n  type: b\n").is_err());
        assert!(with("crowdsec:\n  collection: a/b\n  type: \"b\\nc\"\n").is_err());
        // authelia_paths: des chemins simples, seulement sans Authelia devant toute l'app
        assert!(with("authelia_paths:\n  - /admin\n").is_ok());
        assert!(with("authelia_paths:\n  - admin\n").is_err());
        assert!(with("authelia_paths:\n  - /admin/\n").is_err());
        assert!(with("authelia_paths:\n  - /a`b\n").is_err());
        assert!(
            parse(&format!(
                "{}authelia_paths:\n  - /admin\n",
                OK.replace("access: public", "access: two_factor")
            ))
            .is_err()
        );
        // cap_add, sysctls, kernel_modules: des listes fermées
        assert!(with("cap_add:\n  - NET_ADMIN\n").is_ok());
        assert!(with("cap_add:\n  - SYS_MODULE\n").is_err());
        assert!(with("cap_add:\n  - SYS_ADMIN\n").is_err());
        assert!(with("sysctls:\n  net.ipv4.ip_forward: \"1\"\n").is_ok());
        assert!(with("sysctls:\n  net.ipv4.ip_forward: \"2\"\n").is_err());
        assert!(with("sysctls:\n  kernel.panic: \"1\"\n").is_err());
        assert!(with("kernel_modules:\n  - wireguard\n").is_ok());
        assert!(with("kernel_modules:\n  - overlay\n").is_err());
        // backup_exclude: dans le dossier de l'app seulement (D97)
        assert!(with("backup_exclude:\n  - cache\n  - config/log\n").is_ok());
        assert!(with("backup_exclude:\n  - /etc\n").is_err());
        assert!(with("backup_exclude:\n  - ../dotbox\n").is_err());
        // health_tool: curl ou wget, avec un chemin
        assert!(with("health: /ping\nhealth_tool: wget\n").is_ok());
        assert!(with("health: image\nhealth_tool: wget\n").is_err());
        assert!(with("health: /ping\nhealth_tool: nc\n").is_err());
        // json: a/b/c
        let js = "settings:\n  file: config/settings.json\n  format: json\n  created_by_app: true\n  values:\n";
        assert!(with(&format!("{js}    a/b/c: x\n")).is_ok());
        assert!(with(&format!("{js}    a//c: x\n")).is_err());
        // env: une majuscule d'abord, comme JELLYFIN_PublishedServerUrl
        assert!(with("env:\n  JELLYFIN_PublishedServerUrl: x\n").is_ok());
        assert!(with("env:\n  _A: x\n").is_err());
        // un dossier sous /data, au même chemin
        assert!(with("volumes:\n  - /data/media:/data/media:ro\n").is_ok());
        assert!(with("volumes:\n  - /data/media:/media\n").is_err());
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
