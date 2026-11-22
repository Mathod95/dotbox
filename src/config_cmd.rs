//! `dotbox config get` et `dotbox config set` (Journal, 15.5, D93): lire et changer dotbox.yml,
//! une clé à la fois, après l'avoir vérifiée. config set n'applique rien: dotbox diff montre le
//! plan, dotbox sync l'applique. Une liste fermée de clés.

use serde::{Deserialize, Serialize};

use crate::catalog::{self, Access};
use crate::clack::{self, Mark};
use crate::config::{self, Config};
use crate::{backup, system, traefik};

/// Les clés que config sait changer aujourd'hui.
const KEYS: &str =
    "domain, ca, backup.schedule, backup.keep, apps.<app>.subdomain, apps.<app>.access";

#[derive(Debug, PartialEq)]
enum Key {
    Domain,
    Ca,
    Subdomain(String),
    Access(String),
    /// l'heure de la sauvegarde (D97): daily 04:00 par défaut
    BackupSchedule,
    BackupKeep,
}

fn parse_key(key: &str) -> Result<Key, String> {
    match key {
        "domain" => return Ok(Key::Domain),
        "ca" => return Ok(Key::Ca),
        "backup.schedule" => return Ok(Key::BackupSchedule),
        "backup.keep" => return Ok(Key::BackupKeep),
        "user" => {
            return Err(
                "user cannot be changed with dotbox config: the apps' files belong to it (D93)"
                    .into(),
            );
        }
        _ => {}
    }
    if let Some(rest) = key.strip_prefix("apps.") {
        if let Some((app, field)) = rest.split_once('.') {
            if catalog::valid_name(app) {
                match field {
                    "subdomain" => return Ok(Key::Subdomain(app.into())),
                    "access" => return Ok(Key::Access(app.into())),
                    _ => {}
                }
            }
        }
    }
    if key == "backup.destinations" || key == "backup.exclude" {
        return Err(format!(
            "{key} is a list: write it in {} by hand for now (sudo), then dotbox sync",
            config::PATH
        ));
    }
    let first = key.split('.').next().unwrap_or_default();
    if matches!(first, "paths" | "backup" | "update") {
        return Err(format!("{key}: not yet, it comes with its step"));
    }
    Err(format!("unknown key {key:?}: {KEYS}"))
}

/// Une clé et sa valeur, dans l'ordre du fichier.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub value: String,
}

fn ca_of(cfg: &Config) -> String {
    cfg.get("ca")
        .unwrap_or_else(|| config::DEFAULT_CA.to_string())
}

/// Le sous-domaine et l'accès d'une app installée (le sous-domaine vide veut dire son nom).
fn app_of(cfg: &Config, app: &str) -> Result<(String, String), String> {
    let (sub, access) = cfg
        .app(app)
        .ok_or_else(|| format!("{app} is not installed: dotbox install {app}"))?;
    Ok((if sub.is_empty() { app.to_string() } else { sub }, access))
}

pub fn entries(cfg: &Config) -> Vec<Entry> {
    let mut out = vec![
        Entry {
            key: "domain".into(),
            value: cfg.get("domain").unwrap_or_default(),
        },
        Entry {
            key: "ca".into(),
            value: ca_of(cfg),
        },
        // lue, jamais changée par config (D93)
        Entry {
            key: "user".into(),
            value: cfg.get("user").unwrap_or_default(),
        },
    ];
    // sans section backup: les valeurs par défaut, qui ne sont pas écrites (D97)
    if let Ok(b) = backup::Settings::of(cfg) {
        out.push(Entry {
            key: "backup.schedule".into(),
            value: b.schedule,
        });
        out.push(Entry {
            key: "backup.keep".into(),
            value: b.keep.to_string(),
        });
    }
    for app in cfg.app_names() {
        if let Ok((sub, access)) = app_of(cfg, &app) {
            out.push(Entry {
                key: format!("apps.{app}.subdomain"),
                value: sub,
            });
            out.push(Entry {
                key: format!("apps.{app}.access"),
                value: access,
            });
        }
    }
    out
}

/// dotbox config get: toutes les clés, ou une seule.
pub fn get(key: Option<&str>) -> Result<Vec<Entry>, String> {
    let cfg = Config::load()?;
    let all = entries(&cfg);
    let Some(key) = key else {
        return Ok(all);
    };
    if key == "user" {
        return Ok(all.into_iter().filter(|e| e.key == "user").collect());
    }
    let k = parse_key(key)?;
    if let Key::Subdomain(app) | Key::Access(app) = &k {
        app_of(&cfg, app)?;
    }
    Ok(all.into_iter().filter(|e| e.key == key).collect())
}

/// Ce que config set a fait.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Change {
    pub key: String,
    pub old: String,
    pub new: String,
    /// false: la même valeur qu'avant, rien n'est écrit
    pub changed: bool,
    /// ce qu'il faut savoir avant d'appliquer (les passkeys, pour le domaine: Q61)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub warning: String,
}

/// La valeur vérifiée, sans rien écrire. `dns`: vérifie que *.<domaine> pointe vers le serveur.
fn check(
    cfg: &Config,
    key: &Key,
    value: &str,
    dns: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    match key {
        Key::Domain => {
            if !config::valid_domain(value) {
                return Err(format!(
                    "invalid domain {value:?}: for example example.com or dotbox.example.com"
                ));
            }
            if Some(value.to_string()) != cfg.get("domain") {
                if let Some(why) = dns(value) {
                    return Err(format!(
                        "{why}: point *.{value} to this server first, then run it again"
                    ));
                }
            }
            Ok(cfg.get("domain").unwrap_or_default())
        }
        Key::Ca => {
            if !config::CAS.contains(&value) {
                return Err(format!(
                    "unknown ca {value:?}: {}",
                    config::CAS.join(" or ")
                ));
            }
            Ok(ca_of(cfg))
        }
        Key::Subdomain(app) => {
            let (sub, _) = app_of(cfg, app)?;
            if !catalog::valid_name(value) {
                return Err(format!(
                    "invalid subdomain {value:?}: lowercase letters, digits and -"
                ));
            }
            if catalog::RESERVED.contains(&value) {
                return Err(format!("the subdomain {value} is taken by the core"));
            }
            for other in cfg.app_names().into_iter().filter(|o| o != app) {
                if app_of(cfg, &other).is_ok_and(|(s, _)| s == value) {
                    return Err(format!("the subdomain {value} is taken by {other}"));
                }
            }
            Ok(sub)
        }
        Key::BackupSchedule => {
            backup::on_calendar(value)?;
            Ok(backup::Settings::of(cfg)?.schedule)
        }
        Key::BackupKeep => {
            match value.parse::<usize>() {
                Ok(n) if (1..=365).contains(&n) => {}
                _ => return Err(format!("backup.keep {value:?}: a number from 1 to 365")),
            }
            Ok(backup::Settings::of(cfg)?.keep.to_string())
        }
        Key::Access(app) => {
            let (_, access) = app_of(cfg, app)?;
            match Access::parse(value) {
                None => Err(format!(
                    "unknown access {value:?}: two_factor, app or public"
                )),
                // Authelia n'a pas encore de règle par app: ce serait two_factor (Q60)
                Some(Access::OneFactor) => Err(
                    "one_factor: not yet, Authelia has no rule per app so it would ask for two factors (Q60)"
                        .into(),
                ),
                Some(_) => Ok(access),
            }
        }
    }
}

fn apply(cfg: &mut Config, key: &Key, value: &str) -> Result<(), String> {
    match key {
        Key::Domain => cfg.set("domain", value),
        Key::Ca => cfg.set("ca", value),
        Key::Subdomain(app) => {
            let (_, access) = app_of(cfg, app)?;
            cfg.set_app(app, value, &access);
        }
        Key::Access(app) => {
            let (sub, _) = app_of(cfg, app)?;
            cfg.set_app(app, &sub, value);
        }
        Key::BackupSchedule => cfg.set_in("backup", "schedule", value.into()),
        Key::BackupKeep => {
            let n: u64 = value
                .parse()
                .map_err(|_| "backup.keep: a number".to_string())?;
            cfg.set_in("backup", "keep", n.into());
        }
    }
    Ok(())
}

/// dotbox config set: vérifie, puis écrit dotbox.yml. N'applique rien (D93). En root.
pub fn set(key: &str, value: &str) -> Result<Change, String> {
    if system::effective_uid()? != 0 {
        return Err("run dotbox config --local as root: sudo dotbox config set ... --local".into());
    }
    let k = parse_key(key)?;
    let mut cfg = Config::load()?;
    let old = check(&cfg, &k, value, &traefik::dns_problem)?;
    let changed = old != value;
    if changed {
        apply(&mut cfg, &k, value)?;
        system::write_if_changed(config::PATH, &cfg.render()?, 0o644)?;
    }
    let warning = if changed && k == Key::Domain {
        "passkeys registered in Authelia will no longer work after dotbox sync: register yours again. 6-digit codes keep working, everyone logs in again".into()
    } else {
        String::new()
    };
    Ok(Change {
        key: key.into(),
        old,
        new: value.into(),
        changed,
        warning,
    })
}

pub fn print_get(list: &[Entry], one: bool) {
    if one {
        for e in list {
            clack::put(&e.value);
        }
        return;
    }
    let w = list.iter().map(|e| e.key.len()).max().unwrap_or(0) + 2;
    for e in list {
        clack::put(&format!("{:<w$}{}", e.key, e.value));
    }
}

pub fn print_set(c: &Change) {
    use console::style;
    if !c.changed {
        clack::put(&format!(
            "  {} {} is already {}",
            Mark::Ok.symbol(),
            c.key,
            c.new
        ));
        return;
    }
    let old = if c.old.is_empty() { "(none)" } else { &c.old };
    clack::put(&format!(
        "  {} {}",
        style("✓").green(),
        Mark::Ok.paint(&format!("{}: {old} → {}", c.key, c.new))
    ));
    if !c.warning.is_empty() {
        clack::put(&format!(
            "  {} {}",
            Mark::Warn.symbol(),
            Mark::Warn.paint(&c.warning)
        ));
    }
    // keep n'a rien à appliquer: la prochaine sauvegarde le lit
    let then = if c.key == "backup.keep" {
        "it counts from the next backup"
    } else {
        "then: dotbox diff to see the plan, dotbox sync to apply it"
    };
    clack::put(&format!("  {}", style(then).dim()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const YML: &str = "domain: example.com\nca: letsencrypt\nuser: admin\napps:\n  radarr:\n    subdomain: radarr\n    access: two_factor\n  sonarr:\n    subdomain: series\n    access: two_factor\n";

    fn no_dns_problem(_: &str) -> Option<String> {
        None
    }

    fn dns_elsewhere(_: &str) -> Option<String> {
        Some("*.example.org points to 198.51.100.7, not to this server".into())
    }

    #[test]
    fn keys() {
        assert_eq!(parse_key("domain"), Ok(Key::Domain));
        assert_eq!(parse_key("ca"), Ok(Key::Ca));
        assert_eq!(
            parse_key("apps.radarr.subdomain"),
            Ok(Key::Subdomain("radarr".into()))
        );
        assert_eq!(
            parse_key("apps.radarr.access"),
            Ok(Key::Access("radarr".into()))
        );
        assert!(parse_key("user").unwrap_err().contains("D93"));
        assert!(parse_key("paths.data").unwrap_err().contains("not yet"));
        assert_eq!(parse_key("backup.keep"), Ok(Key::BackupKeep));
        assert_eq!(parse_key("backup.schedule"), Ok(Key::BackupSchedule));
        assert!(
            parse_key("backup.destinations")
                .unwrap_err()
                .contains("by hand")
        );
        assert!(parse_key("backup.stream").unwrap_err().contains("not yet"));
        assert!(
            parse_key("apps.radarr.image")
                .unwrap_err()
                .contains("unknown")
        );
        assert!(
            parse_key("apps.../x.access")
                .unwrap_err()
                .contains("unknown")
        );
        assert!(parse_key("email").unwrap_err().contains("domain, ca"));
    }

    /// L'heure de la sauvegarde se change (D97), sa valeur par défaut n'est pas écrite.
    #[test]
    fn backup_keys() {
        let mut c = Config::parse(YML).unwrap();
        let get = |c: &Config, k: &str| {
            entries(c)
                .into_iter()
                .find(|e| e.key == k)
                .map(|e| e.value)
                .unwrap_or_default()
        };
        assert_eq!(get(&c, "backup.schedule"), "daily 04:00");
        assert_eq!(get(&c, "backup.keep"), "7");
        let k = parse_key("backup.schedule").unwrap();
        assert_eq!(
            check(&c, &k, "daily 02:30", &no_dns_problem).unwrap(),
            "daily 04:00"
        );
        assert!(check(&c, &k, "at night", &no_dns_problem).is_err());
        apply(&mut c, &k, "daily 02:30").unwrap();
        let k = parse_key("backup.keep").unwrap();
        assert!(check(&c, &k, "0", &no_dns_problem).is_err());
        assert!(check(&c, &k, "ten", &no_dns_problem).is_err());
        apply(&mut c, &k, "14").unwrap();
        assert_eq!(get(&c, "backup.schedule"), "daily 02:30");
        assert_eq!(get(&c, "backup.keep"), "14");
        assert!(
            c.render()
                .unwrap()
                .contains("backup:\n  schedule: daily 02:30\n  keep: 14\n")
        );
    }

    #[test]
    fn values_checked() {
        let c = Config::parse(YML).unwrap();
        let ok = |k: &str, v: &str| check(&c, &parse_key(k).unwrap(), v, &no_dns_problem);
        assert_eq!(ok("domain", "example.org"), Ok("example.com".into()));
        assert!(ok("domain", "Example.org").is_err());
        assert!(
            check(&c, &Key::Domain, "example.org", &dns_elsewhere)
                .unwrap_err()
                .contains("point *.example.org")
        );
        // la même valeur: pas de vérification DNS
        assert!(check(&c, &Key::Domain, "example.com", &dns_elsewhere).is_ok());
        assert_eq!(ok("ca", "letsencrypt-staging"), Ok("letsencrypt".into()));
        assert!(ok("ca", "zerossl").is_err());
        assert_eq!(ok("apps.radarr.subdomain", "films"), Ok("radarr".into()));
        assert!(
            ok("apps.radarr.subdomain", "auth")
                .unwrap_err()
                .contains("core")
        );
        assert!(
            ok("apps.radarr.subdomain", "series")
                .unwrap_err()
                .contains("sonarr")
        );
        assert!(ok("apps.radarr.subdomain", "Films").is_err());
        assert!(
            ok("apps.lidarr.subdomain", "music")
                .unwrap_err()
                .contains("not installed")
        );
        assert_eq!(ok("apps.radarr.access", "app"), Ok("two_factor".into()));
        assert!(
            ok("apps.radarr.access", "one_factor")
                .unwrap_err()
                .contains("Q60")
        );
        assert!(ok("apps.radarr.access", "open").is_err());
    }

    #[test]
    fn written_back() {
        let mut c = Config::parse(YML).unwrap();
        c.set_stopped("radarr", true);
        apply(&mut c, &Key::Subdomain("radarr".into()), "films").unwrap();
        apply(&mut c, &Key::Access("radarr".into()), "app").unwrap();
        apply(&mut c, &Key::Domain, "example.org").unwrap();
        let again = Config::parse(&c.render().unwrap()).unwrap();
        assert_eq!(again.app("radarr"), Some(("films".into(), "app".into())));
        assert!(again.app_stopped("radarr"));
        assert_eq!(again.get("domain").as_deref(), Some("example.org"));
        let e = entries(&again);
        assert_eq!(
            e[0],
            Entry {
                key: "domain".into(),
                value: "example.org".into()
            }
        );
        assert_eq!(
            e[2],
            Entry {
                key: "user".into(),
                value: "admin".into()
            }
        );
        assert!(e.contains(&Entry {
            key: "apps.sonarr.subdomain".into(),
            value: "series".into()
        }));
    }
}
