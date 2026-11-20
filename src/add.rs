//! `sudo dotbox add <compose.yml> --name <app>` (Journal, 14.3): un compose devient un manifest
//! dans /opt/dotbox/apps/<app>.yml (apps locales, même format que le catalogue), que
//! `dotbox install` trouve avant le catalogue. En root (D80): un compose peut monter n'importe
//! quel dossier du serveur. Un seul service pour l'instant: le moteur ne sait pas encore faire une
//! app à plusieurs containers (D74, D76).

use crate::catalog;
use crate::system;
use serde_yaml_ng::Value;
use std::path::{Path, PathBuf};

/// Les apps ajoutées avec dotbox add.
pub const LOCAL: &str = "/opt/dotbox/apps";

/// Des clés du service qui n'ont pas leur place dans un manifest, et que Dot Box remplace.
const IGNORED: &[&str] = &[
    "container_name",
    "restart",
    "networks",
    "labels",
    "ports",
    "expose",
    "hostname",
    "logging",
];

/// Ce que add a lu et écrit, pour l'écran.
#[derive(Debug, PartialEq)]
pub struct Added {
    pub service: String,
    pub image: String,
    pub port: u16,
    pub manifest: String,
    /// les dossiers du compose rangés dans /opt/<app> (« ./data → data »)
    pub moved: Vec<String>,
    pub ignored: Vec<String>,
}

/// Le port du container dans « 8080:80 », « 80 », « 127.0.0.1:8080:80/tcp » ou { target: 80 }.
fn container_port(v: &Value) -> Option<u16> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
        Value::String(s) => {
            let s = s.split('/').next()?;
            s.rsplit(':').next()?.parse().ok()
        }
        Value::Mapping(m) => container_port(m.get("target")?),
        _ => None,
    }
}

/// Un nom de dossier sous /opt/<app> à partir d'un volume du compose (« ./data », « db »,
/// « /srv/photos »): le dernier morceau, en minuscules.
fn folder(src: &str) -> String {
    let last = src
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let f: String = last
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let f = f.trim_matches('-').to_string();
    if f.is_empty() { "data".into() } else { f }
}

/// Un volume du compose en volume de manifest: /data reste tel quel (mêmes chemins, hardlinks),
/// le reste va dans /opt/<app>/<dossier>.
fn volume(v: &Value, moved: &mut Vec<String>) -> Result<String, String> {
    let (src, dst, ro) = match v {
        Value::String(s) => {
            let p: Vec<&str> = s.split(':').collect();
            match p.as_slice() {
                [d] => (String::new(), d.to_string(), false),
                [s, d] => (s.to_string(), d.to_string(), false),
                [s, d, o] => (
                    s.to_string(),
                    d.to_string(),
                    o.split(',').any(|x| x == "ro"),
                ),
                _ => return Err(format!("volume {s:?}: not understood")),
            }
        }
        Value::Mapping(m) => {
            let get = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or_default();
            let ro = m.get("read_only").and_then(Value::as_bool).unwrap_or(false);
            (get("source").to_string(), get("target").to_string(), ro)
        }
        _ => return Err("a volume must be text or a mapping".into()),
    };
    if !dst.starts_with('/') {
        return Err(format!(
            "volume {dst:?}: the container path must be absolute"
        ));
    }
    let ro = if ro { ":ro" } else { "" };
    if catalog::in_data(&src) && src == dst {
        return Ok(format!("{src}:{dst}{ro}"));
    }
    // un volume sans dossier du serveur (anonyme): son nom vient du chemin du container
    let name = folder(if src.is_empty() { &dst } else { &src });
    moved.push(format!(
        "{} → {name}",
        if src.is_empty() { &dst } else { &src }
    ));
    Ok(format!("{name}:{dst}{ro}"))
}

/// L'environnement du compose: une liste « K=V » ou une table.
fn environment(v: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let text = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some(String::new()),
        _ => None,
    };
    match v {
        None | Some(Value::Null) => {}
        Some(Value::Sequence(items)) => {
            for i in items {
                let s = i.as_str().ok_or("environment: every entry must be K=V")?;
                let (k, v) = s.split_once('=').unwrap_or((s, ""));
                out.push((k.to_string(), v.to_string()));
            }
        }
        Some(Value::Mapping(m)) => {
            for (k, v) in m {
                let k = k.as_str().ok_or("environment: every name must be text")?;
                let v =
                    text(v).ok_or_else(|| format!("environment {k}: the value must be text"))?;
                out.push((k.to_string(), v));
            }
        }
        _ => return Err("environment: a list or a mapping".into()),
    }
    for (k, v) in &out {
        if v.contains("${") || v.starts_with('$') {
            return Err(format!(
                "environment {k}={v}: Dot Box does not read .env files, write the value itself"
            ));
        }
    }
    Ok(out)
}

/// Le manifest d'un compose, vérifié par le même lecteur que le catalogue.
pub fn manifest(compose: &str, name: &str, port: Option<u16>, from: &str) -> Result<Added, String> {
    let doc: Value =
        serde_yaml_ng::from_str(compose).map_err(|e| format!("not a compose file: {e}"))?;
    let services = doc
        .get("services")
        .and_then(Value::as_mapping)
        .ok_or("not a compose file: no services")?;
    let names: Vec<String> = services
        .keys()
        .filter_map(|k| k.as_str().map(String::from))
        .collect();
    if names.len() != 1 {
        return Err(format!(
            "{} services ({}): Dot Box installs apps of one container for now, an app with a database alongside comes later",
            names.len(),
            names.join(", ")
        ));
    }
    let service = names[0].clone();
    let s = services
        .get(service.as_str())
        .and_then(Value::as_mapping)
        .ok_or_else(|| format!("service {service}: empty"))?;
    let mut ignored = Vec::new();
    for k in s.keys().filter_map(Value::as_str) {
        match k {
            "image" | "environment" | "volumes" => {}
            k if IGNORED.contains(&k) => ignored.push(k.to_string()),
            k => {
                return Err(format!(
                    "service {service}: {k} is not supported yet by Dot Box manifests"
                ));
            }
        }
    }
    let image = s
        .get("image")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("service {service}: no image (build is not supported)"))?
        .to_string();
    let port = match port {
        Some(p) => p,
        None => s
            .get("ports")
            .or_else(|| s.get("expose"))
            .and_then(Value::as_sequence)
            .and_then(|p| p.first())
            .and_then(container_port)
            .ok_or("no web port in the compose: give it with --port")?,
    };
    let mut env = environment(s.get("environment"))?;
    for (k, v) in [("PUID", "{{uid}}"), ("PGID", "{{gid}}"), ("TZ", "{{tz}}")] {
        if !env.iter().any(|(e, _)| e == k) {
            env.push((k.to_string(), v.to_string()));
        }
    }
    let mut moved = Vec::new();
    let mut vols = Vec::new();
    if let Some(v) = s.get("volumes") {
        for item in v.as_sequence().ok_or("volumes: a list")? {
            vols.push(volume(item, &mut moved)?);
        }
    }
    let q = |t: &str| serde_json::to_string(t).unwrap_or_default();
    let mut y = format!(
        "# Ajoutée par dotbox add depuis {from}. Relis-la avant dotbox install {name}.\n\
         # Même format que le catalogue (Journal, étape 7): les {{{{...}}}} sont remplis à l'installation.\n\
         name: {name}\n\
         description: {}\n\
         min_dotbox: {}\n\
         image: {}\n\
         port: {port}\n\
         access: two_factor\n",
        q(&format!("Added with dotbox add ({service})")),
        env!("CARGO_PKG_VERSION"),
        q(&image),
    );
    y += "env:\n";
    for (k, v) in &env {
        y += &format!("  {k}: {}\n", q(v));
    }
    if !vols.is_empty() {
        y += "volumes:\n";
        for v in &vols {
            y += &format!("  - {}\n", q(v));
        }
    }
    let m = catalog::parse(&y)?;
    if m.name != name {
        return Err(format!("{name:?} is not an app name"));
    }
    Ok(Added {
        service,
        image,
        port,
        manifest: y,
        moved,
        ignored,
    })
}

/// Le dossier où lire le manifest d'une app: ses apps à soi d'abord, puis le catalogue.
pub fn manifest_dir(catalog: &Path, app: &str) -> PathBuf {
    let local = Path::new(LOCAL);
    if local.join(format!("{app}.yml")).exists() {
        local.to_path_buf()
    } else {
        catalog.to_path_buf()
    }
}

pub fn run(file: &str, name: &str, port: Option<u16>) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err(format!(
            "dotbox add is for root only (a compose can mount any folder of the server): sudo dotbox add {file} --name {name}"
        ));
    }
    if !catalog::valid_name(name) || catalog::RESERVED.contains(&name) {
        return Err(format!("{name:?} cannot be an app name"));
    }
    let target = format!("{LOCAL}/{name}.yml");
    if Path::new(&target).exists() {
        return Err(format!(
            "{name} is already added: change {target}, or remove it first"
        ));
    }
    let catalog = Path::new("/opt/dotbox/catalog");
    if catalog.join(format!("{name}.yml")).exists() {
        return Err(format!(
            "{name} is in the catalog: dotbox install {name}, or another --name"
        ));
    }
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let a = manifest(&text, name, port, file)?;
    std::fs::create_dir_all(LOCAL).map_err(|e| format!("cannot create {LOCAL}: {e}"))?;
    std::fs::write(&target, &a.manifest).map_err(|e| format!("cannot write {target}: {e}"))?;
    let line = |w: &str, t: &str| eprintln!("  {w:<8} {t}");
    line("read", &format!("1 service: {} ({})", a.service, a.image));
    for m in &a.moved {
        line("folder", &format!("{m} (in /opt/{name})"));
    }
    if !a.ignored.is_empty() {
        line(
            "ignored",
            &format!("{} (Dot Box sets them)", a.ignored.join(", ")),
        );
    }
    line(
        "write",
        &format!(
            "{target} (subdomain {name}, access two_factor, port {})",
            a.port
        ),
    );
    eprintln!(
        "  {}",
        console::style(format!("ready    dotbox install {name}")).green()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IT_TOOLS: &str = "services:\n  it-tools:\n    image: corentinth/it-tools:2024.10.22-7ca5933\n    container_name: it-tools\n    restart: unless-stopped\n    ports:\n      - \"8080:80\"\n";

    #[test]
    fn one_service() {
        let a = manifest(IT_TOOLS, "it-tools", None, "./compose.yml").unwrap();
        assert_eq!(a.port, 80);
        assert_eq!(a.service, "it-tools");
        assert_eq!(a.ignored, vec!["container_name", "restart", "ports"]);
        assert!(
            a.manifest
                .contains("image: \"corentinth/it-tools:2024.10.22-7ca5933\"\n")
        );
        assert!(a.manifest.contains("  PUID: \"{{uid}}\"\n"));
        let m = catalog::parse(&a.manifest).unwrap();
        assert_eq!(m.port, 80);
        assert_eq!(m.access, catalog::Access::TwoFactor);
    }

    #[test]
    fn folders_and_data() {
        let c = "services:\n  app:\n    image: x/app:1.2.3\n    expose: [3000]\n    environment:\n      - TZ=Europe/Paris\n      - MODE=prod\n    volumes:\n      - ./data:/app/data\n      - db:/var/lib/db\n      - /srv/Photos:/photos:ro\n      - /data/media:/data/media\n";
        let a = manifest(c, "app", None, "c.yml").unwrap();
        assert_eq!(a.port, 3000);
        assert!(a.manifest.contains("  - \"data:/app/data\"\n"));
        assert!(a.manifest.contains("  - \"db:/var/lib/db\"\n"));
        assert!(a.manifest.contains("  - \"photos:/photos:ro\"\n"));
        assert!(a.manifest.contains("  - \"/data/media:/data/media\"\n"));
        assert!(a.manifest.contains("  TZ: \"Europe/Paris\"\n"));
        assert!(!a.manifest.contains("{{tz}}"));
        assert_eq!(a.moved.len(), 3);
        catalog::parse(&a.manifest).unwrap();
    }

    #[test]
    fn refused() {
        let two = "services:\n  server:\n    image: a/b:1\n  db:\n    image: postgres:16\n";
        assert!(
            manifest(two, "immich", None, "c")
                .unwrap_err()
                .contains("2 services")
        );
        let latest = "services:\n  a:\n    image: a/b:latest\n    ports: [\"80\"]\n";
        assert!(
            manifest(latest, "a", None, "c")
                .unwrap_err()
                .contains("latest")
        );
        let priv_ =
            "services:\n  a:\n    image: a/b:1\n    privileged: true\n    ports: [\"80\"]\n";
        assert!(
            manifest(priv_, "a", None, "c")
                .unwrap_err()
                .contains("privileged")
        );
        let noport = "services:\n  a:\n    image: a/b:1\n";
        assert!(
            manifest(noport, "a", None, "c")
                .unwrap_err()
                .contains("--port")
        );
        assert_eq!(manifest(noport, "a", Some(8000), "c").unwrap().port, 8000);
        let envfile = "services:\n  a:\n    image: a/b:1\n    ports: [\"80\"]\n    environment:\n      KEY: ${KEY}\n";
        assert!(
            manifest(envfile, "a", None, "c")
                .unwrap_err()
                .contains(".env")
        );
    }
}
