//! /opt/dotbox/dotbox.yml, le fichier unique de configuration (spec: Basics). Pour l'instant:
//! le domaine, l'autorité de certification et l'utilisateur. Les clés inconnues sont gardées.

use serde_yaml_ng::{Mapping, Value};

pub const PATH: &str = "/opt/dotbox/dotbox.yml";

/// Autorités de certification acceptées: Let's Encrypt seulement (Journal D19), et son
/// environnement de staging pour les tests (D17).
pub const CAS: &[&str] = &["letsencrypt", "letsencrypt-staging"];
pub const DEFAULT_CA: &str = "letsencrypt";

pub struct Config {
    map: Mapping,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        if text.trim().is_empty() {
            return Ok(Config {
                map: Mapping::new(),
            });
        }
        match serde_yaml_ng::from_str::<Value>(text) {
            Ok(Value::Mapping(map)) => Ok(Config { map }),
            Ok(Value::Null) => Ok(Config {
                map: Mapping::new(),
            }),
            Ok(_) => Err(format!("{PATH} must be a YAML mapping (key: value)")),
            Err(e) => Err(format!("{PATH} is not valid YAML: {e}")),
        }
    }

    pub fn load() -> Result<Config, String> {
        match std::fs::read_to_string(PATH) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::parse(""),
            Err(e) => Err(format!("cannot read {PATH}: {e}")),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match self.map.get(key) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.map
            .insert(Value::String(key.into()), Value::String(value.into()));
    }

    pub fn render(&self) -> Result<String, String> {
        let body = serde_yaml_ng::to_string(&self.map).map_err(|e| e.to_string())?;
        Ok(format!(
            "# Dot Box: configuration du serveur (spec: Getting Started).\n{body}"
        ))
    }
}

/// Nom de domaine: au moins deux labels, lettres minuscules, chiffres et « - », 253 caractères au plus.
pub fn valid_domain(d: &str) -> bool {
    let labels: Vec<&str> = d.split('.').collect();
    d.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_unknown_keys() {
        let mut c = Config::parse("domain: example.com\nfuture: [1, 2]\n").unwrap();
        assert_eq!(c.get("domain").as_deref(), Some("example.com"));
        assert_eq!(c.get("ca"), None);
        c.set("ca", "letsencrypt-staging");
        c.set("domain", "example.org");
        let text = c.render().unwrap();
        let again = Config::parse(&text).unwrap();
        assert_eq!(again.get("domain").as_deref(), Some("example.org"));
        assert_eq!(again.get("ca").as_deref(), Some("letsencrypt-staging"));
        assert!(text.contains("future:"));
        assert!(text.starts_with("# Dot Box"));
    }

    #[test]
    fn empty_and_invalid() {
        assert_eq!(Config::parse("").unwrap().get("domain"), None);
        assert_eq!(Config::parse("# rien\n").unwrap().get("domain"), None);
        assert!(Config::parse("- a\n- b\n").is_err());
        assert!(Config::parse("domain: [unclosed\n").is_err());
    }

    #[test]
    fn domains() {
        for ok in [
            "example.com",
            "dotbox.example.com",
            "a-b.example.org",
            "x1.io",
        ] {
            assert!(valid_domain(ok), "{ok}");
        }
        for bad in [
            "",
            "example",
            "Example.com",
            "-a.example.com",
            "a-.example.com",
            "a..example.com",
            "*.example.com",
            "example.com.",
            "exa mple.com",
            "https://example.com",
        ] {
            assert!(!valid_domain(bad), "{bad}");
        }
    }
}
