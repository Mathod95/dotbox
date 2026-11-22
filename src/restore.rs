//! `sudo dotbox restore <latest|date|archive> [--app <app>] [--key <file>]` (Journal, 16.4,
//! D79, D97): en root, sans le service, parce qu'une archive peut contenir n'importe quel fichier
//! de /opt. Une app: l'archive ouverte à côté (/opt/.restore-<app>-<date>), l'app arrêtée, son
//! dossier mis de côté (/opt/<app>.before-restore-<date>, pas effacé), le nouveau mis à sa place,
//! l'app relancée et attendue Running. Si elle ne repart pas, l'ancien dossier est remis.
//! Tout le serveur: /opt/dotbox (par-dessus, l'ancien copié de côté), puis chaque app de la
//! config restaurée, puis dotbox sync.

use std::fs;
use std::path::{Component, Path};
use std::time::Duration;

use crate::backup::{self, Archive, Dest, Settings};
use crate::config::{self, Config};
use crate::out::say;
use crate::{apps, catalog, manage, prompt, sync, system, ui};

const OPT: &str = "/opt";

/// Ce que demande la commande: les archives, dans l'ordre où les restaurer.
fn choose(what: &str, app: Option<&str>, dirs: &[String]) -> Result<Vec<Archive>, String> {
    // une archive précise, par son chemin
    if what.contains('/') || what.ends_with(backup::SUFFIX) {
        let path = Path::new(what);
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (name, stamp) = backup::parse_name(&file).ok_or(format!(
            "{what}: not a Dot Box archive (<app>-<date>.tar.zst.age)"
        ))?;
        if let Some(a) = app {
            if a != name {
                return Err(format!("{what} is an archive of {name}, not of {a}"));
            }
        }
        let bytes = fs::metadata(path)
            .map_err(|e| format!("cannot read {what}: {e}"))?
            .len();
        return Ok(vec![Archive {
            app: name,
            stamp,
            path: what.to_string(),
            bytes,
        }]);
    }
    let date = match what {
        "latest" => "",
        d if d.len() == 10 || d.len() == 15 => d,
        _ => {
            return Err(format!(
                "{what:?}: latest, a date (2026-11-22 or 2026-11-22-0400) or the path of an archive"
            ));
        }
    };
    let mut all: Vec<Archive> = Vec::new();
    for d in dirs {
        for a in backup::list(d) {
            if !all.iter().any(|b| b.app == a.app && b.stamp == a.stamp) {
                all.push(a);
            }
        }
    }
    pick(all, date, app, &dirs.join(", "))
}

/// La plus récente de chaque app (de cette date), dotbox d'abord.
fn pick(
    mut all: Vec<Archive>,
    date: &str,
    app: Option<&str>,
    place: &str,
) -> Result<Vec<Archive>, String> {
    all.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    let mut picked: Vec<Archive> = Vec::new();
    for a in all {
        if !a.stamp.starts_with(date) || app.is_some_and(|x| x != a.app) {
            continue;
        }
        if !picked.iter().any(|p| p.app == a.app) {
            picked.push(a);
        }
    }
    if picked.is_empty() {
        return Err(format!(
            "no archive{}{} in {}",
            app.map(|a| format!(" of {a}")).unwrap_or_default(),
            if date.is_empty() {
                String::new()
            } else {
                format!(" from {date}")
            },
            place
        ));
    }
    // dotbox d'abord: les apps à restaurer sont celles de la config restaurée
    picked.sort_by_key(|a| a.app != "dotbox");
    Ok(picked)
}

/// La plus récente de chaque app sur une destination distante, rapportée dans le premier dossier
/// local (D99, 8). Un serveur neuf reste 16.7.
fn from_remote(what: &str, app: Option<&str>, dirs: &[String]) -> Result<Vec<Archive>, String> {
    Dest::parse(what)?;
    let local = dirs
        .first()
        .cloned()
        .unwrap_or_else(|| backup::DEFAULT_LOCAL.to_string());
    let found: Vec<Archive> = backup::remote_list(what, &local)?
        .into_iter()
        .map(|(file, app, stamp)| Archive {
            app,
            stamp,
            path: format!("{local}/{file}"),
            bytes: 0,
        })
        .collect();
    let picked = pick(found, "", app, what)?;
    let files: Vec<String> = picked
        .iter()
        .filter_map(|a| {
            Path::new(&a.path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .collect();
    eprintln!(
        "  fetching {} from {what} into {local}",
        backup::count(files.len())
    );
    backup::fetch(what, &local, &files)?;
    Ok(picked
        .into_iter()
        .map(|mut a| {
            a.bytes = fs::metadata(&a.path).map(|m| m.len()).unwrap_or(0);
            a
        })
        .collect())
}

/// Ouvre l'archive dans `staging`. Chaque entrée doit être dans <app>/, sans .. ni chemin absolu.
fn unpack(
    archive: &Archive,
    identity: &age::x25519::Identity,
    staging: &Path,
) -> Result<u64, String> {
    let mut ar = backup::open(Path::new(&archive.path), identity)?;
    ar.set_preserve_permissions(true);
    ar.set_preserve_ownerships(true);
    ar.set_preserve_mtime(true);
    ar.set_overwrite(true);
    fs::create_dir_all(staging).map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    let mut n = 0;
    for entry in ar.entries().map_err(|e| format!("{}: {e}", archive.path))? {
        let mut entry = entry.map_err(|e| format!("{}: {e}", archive.path))?;
        let path = entry
            .path()
            .map_err(|e| format!("{}: {e}", archive.path))?
            .into_owned();
        let mut parts = path.components();
        let first = parts.next();
        let inside = matches!(first, Some(Component::Normal(f)) if f == archive.app.as_str())
            && path.components().all(|c| matches!(c, Component::Normal(_)));
        if !inside {
            return Err(format!(
                "{}: {} is outside {}/, refused",
                archive.path,
                path.display(),
                archive.app
            ));
        }
        if !entry
            .unpack_in(staging)
            .map_err(|e| format!("cannot extract {}: {e}", path.display()))?
        {
            return Err(format!("{}: {} refused", archive.path, path.display()));
        }
        n += 1;
    }
    Ok(n)
}

/// /opt/dotbox: l'ancien copié de côté, puis l'archive posée par-dessus (les logs, absents de
/// l'archive, restent, et le log de ce restore continue d'y être écrit).
fn restore_dotbox(
    archive: &Archive,
    identity: &age::x25519::Identity,
    now: &str,
) -> Result<String, String> {
    let staging = format!("{OPT}/.restore-dotbox-{now}");
    let aside = format!("{OPT}/dotbox.before-restore-{now}");
    let result = (|| {
        let n = unpack(archive, identity, Path::new(&staging))?;
        say("extract", &format!("{} ({n} entries)", archive.path));
        system::run("cp", &["-a", &format!("{OPT}/dotbox"), &aside])?;
        say("keep", &aside);
        system::run(
            "cp",
            &[
                "-a",
                &format!("{staging}/dotbox/."),
                &format!("{OPT}/dotbox/"),
            ],
        )?;
        say("write", &format!("{OPT}/dotbox"));
        Ok(format!("from {}, old one kept", archive.stamp))
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Une app: ouverte à côté, arrêtée, son dossier mis de côté, remplacée, relancée. Si elle ne
/// repart pas, l'ancien dossier revient.
fn restore_app(
    archive: &Archive,
    identity: &age::x25519::Identity,
    now: &str,
    stopped: bool,
) -> Result<String, String> {
    let app = &archive.app;
    let dir = format!("{OPT}/{app}");
    let staging = format!("{OPT}/.restore-{app}-{now}");
    let aside = format!("{OPT}/{app}.before-restore-{now}");
    let unpacked = unpack(archive, identity, Path::new(&staging));
    let n = match unpacked {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    say("extract", &format!("{} ({n} entries)", archive.path));
    let had = Path::new(&dir).exists();
    if had {
        if Path::new(&backup_compose(app)).exists() {
            say("stop", app);
            apps::compose(app, &["stop"])?;
        }
        fs::rename(&dir, &aside).map_err(|e| format!("cannot move {dir}: {e}"))?;
        say("keep", &aside);
    }
    fs::rename(format!("{staging}/{app}"), &dir)
        .map_err(|e| format!("cannot move into {dir}: {e}"))?;
    let _ = fs::remove_dir_all(&staging);
    say("write", &dir);
    if stopped {
        return Ok(format!(
            "from {}, left stopped (dotbox stop)",
            archive.stamp
        ));
    }
    let started = apps::compose(app, &["up", "-d", "--quiet-pull"]).and_then(|_| {
        say("start", app);
        apps::wait_ready(app, manage::has_health(app), Duration::from_secs(180))
    });
    match started {
        Ok(state) => Ok(format!(
            "from {}, {state}{}",
            archive.stamp,
            if had { ", old folder kept" } else { "" }
        )),
        Err(e) if had => {
            // l'ancien dossier revient, l'app repart avec lui
            let failed = format!("{OPT}/{app}.restore-failed-{now}");
            let _ = apps::compose(app, &["stop"]);
            let _ = fs::rename(&dir, &failed);
            fs::rename(&aside, &dir)
                .map_err(|e2| format!("{e}\nand cannot put {aside} back: {e2}"))?;
            let _ = apps::compose(app, &["up", "-d"]);
            Err(format!(
                "{e}\nthe old {dir} is back in place, the restored one is in {failed}"
            ))
        }
        Err(e) => Err(e),
    }
}

fn backup_compose(app: &str) -> String {
    crate::engine::compose_path(app)
}

fn confirm(list: &[Archive], yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !prompt::is_terminal() {
        return Err(
            "dotbox restore asks for a confirmation: run it in a terminal, or add -y".into(),
        );
    }
    eprintln!();
    for a in list {
        eprintln!(
            "  {:<14}{}  {}",
            a.app,
            a.stamp,
            console::style(backup::human(a.bytes)).dim()
        );
    }
    eprintln!(
        "  {}",
        console::style(
            "each folder is replaced, the current one kept as /opt/<app>.before-restore-<date>"
        )
        .dim()
    );
    eprintln!();
    if prompt::confirm("Restore these archives?", "←/→ to choose, Enter to confirm")? {
        Ok(())
    } else {
        eprintln!("  cancelled, nothing changed");
        Err(String::new())
    }
}

pub fn run(
    what: &str,
    app: Option<&str>,
    key: Option<&str>,
    yes: bool,
    verbose: bool,
) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err(
            "dotbox restore is for root only (an archive can hold any file of /opt): sudo dotbox restore ..."
                .into(),
        );
    }
    if !Path::new(config::PATH).exists() {
        return Err(format!(
            "no {}: restoring a new server comes with the Journal's 16.7. For now: sudo dotbox init first",
            config::PATH
        ));
    }
    let cfg = Config::load()?;
    if let Some(a) = app {
        if a != "dotbox" && !cfg.app_names().iter().any(|i| i == a) {
            return Err(format!(
                "{a} is not installed: dotbox install {a}, then restore it"
            ));
        }
    }
    let settings = Settings::of(&cfg)?;
    let dirs: Vec<String> = settings
        .destinations
        .iter()
        .filter_map(|d| match d {
            Dest::Local(p) => Some(p.clone()),
            Dest::Remote(_) => None,
        })
        .collect();
    let key_path = key.unwrap_or(backup::KEY);
    let identity = backup::load_key(key_path)?;
    let list = if what.starts_with("sftp:") || what.starts_with("rclone:") {
        from_remote(what, app, &dirs)?
    } else {
        choose(what, app, &dirs)?
    };
    let whole = app.is_none() && list.iter().any(|a| a.app == "dotbox") && list.len() > 1;
    confirm(&list, yes)?;
    let label = [Some(what.to_string()), app.map(|a| format!("--app {a}"))]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    apps::begin("restore", &label, list.len(), verbose)?;
    let now = system::run("date", &["+%Y-%m-%d-%H%M%S"])?;
    let result = (|| {
        for a in &list {
            ui::step(
                &apps::title(&a.app),
                &format!("from {}", backup::human(a.bytes)),
            );
            let done = if a.app == "dotbox" {
                restore_dotbox(a, &identity, &now)?
            } else {
                // la config de l'app, telle qu'elle est maintenant (restaurée si dotbox l'a été)
                let cfg = Config::load()?;
                if !cfg.app_names().iter().any(|i| i == &a.app) {
                    say(
                        "skip",
                        &format!("{}: not in the restored dotbox.yml", a.app),
                    );
                    ui::done("skipped, not in dotbox.yml");
                    continue;
                }
                if !catalog::valid_name(&a.app) {
                    return Err(format!("{:?} is not an app name", a.app));
                }
                restore_app(a, &identity, &now, cfg.app_stopped(&a.app))?
            };
            ui::done(&done);
        }
        Ok(())
    })();
    apps::finish(result)?;
    // les anciens dossiers, gardés de côté: leur chemin en entier, à effacer quand tout va bien
    let kept: Vec<String> = fs::read_dir(OPT)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(&format!(".before-restore-{now}")))
                .map(|n| format!("{OPT}/{n}"))
                .collect()
        })
        .unwrap_or_default();
    if !kept.is_empty() {
        eprintln!();
        eprintln!("  the old folders, kept: delete them once all is fine (sudo rm -rf ...)");
        for k in &kept {
            eprintln!("    {}", console::style(k).dim());
        }
    }
    if whole {
        // tout ce qui tourne doit correspondre à la config restaurée (spec: Restore)
        eprintln!();
        sync::run(true, verbose)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), "x").unwrap();
    }

    #[test]
    fn chosen() {
        let tmp = std::env::temp_dir().join(format!("dotbox-restore-test-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        for f in [
            "dotbox-2026-11-21-0400.tar.zst.age",
            "radarr-2026-11-21-0400.tar.zst.age",
            "dotbox-2026-11-22-0400.tar.zst.age",
            "radarr-2026-11-22-0400.tar.zst.age",
            "sonarr-2026-11-21-0400.tar.zst.age",
        ] {
            touch(&tmp, f);
        }
        let dirs = vec![tmp.display().to_string()];
        let l = choose("latest", None, &dirs).unwrap();
        let got: Vec<(&str, &str)> = l
            .iter()
            .map(|a| (a.app.as_str(), a.stamp.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("dotbox", "2026-11-22-0400"),
                ("radarr", "2026-11-22-0400"),
                ("sonarr", "2026-11-21-0400")
            ]
        );
        let l = choose("2026-11-21", Some("radarr"), &dirs).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].stamp, "2026-11-21-0400");
        assert!(choose("2026-11-20", None, &dirs).is_err());
        assert!(choose("yesterday", None, &dirs).is_err());
        assert!(choose("rclone:b2:x", None, &dirs).is_err());
        let one = tmp
            .join("radarr-2026-11-21-0400.tar.zst.age")
            .display()
            .to_string();
        assert_eq!(choose(&one, None, &dirs).unwrap()[0].app, "radarr");
        assert!(choose(&one, Some("sonarr"), &dirs).is_err());
        fs::remove_dir_all(&tmp).unwrap();
    }

    /// Une archive qui sort de son dossier est refusée, rien n'est écrit hors de staging.
    #[test]
    fn escape_refused() {
        let tmp = std::env::temp_dir().join(format!("dotbox-escape-test-{}", std::process::id()));
        fs::create_dir_all(tmp.join("src/other")).unwrap();
        fs::create_dir_all(tmp.join("src/demo")).unwrap();
        fs::write(tmp.join("src/other/x"), "x").unwrap();
        let id = age::x25519::Identity::generate();
        // une archive de « other » présentée comme celle de « demo »
        let path = tmp.join("demo-2026-11-22-0400.tar.zst.age");
        backup::seal(&tmp.join("src"), "other", &[], &id.to_public(), &path).unwrap();
        let a = Archive {
            app: "demo".into(),
            stamp: "2026-11-22-0400".into(),
            path: path.display().to_string(),
            bytes: 0,
        };
        let staging = tmp.join("staging");
        assert!(unpack(&a, &id, &staging).is_err());
        assert!(!staging.join("other/x").exists());
        // la bonne: tout revient
        backup::seal(&tmp.join("src"), "demo", &[], &id.to_public(), &path).unwrap();
        fs::write(tmp.join("src/demo/y"), "y").unwrap();
        backup::seal(&tmp.join("src"), "demo", &[], &id.to_public(), &path).unwrap();
        assert_eq!(unpack(&a, &id, &staging).unwrap(), 2);
        assert_eq!(fs::read_to_string(staging.join("demo/y")).unwrap(), "y");
        fs::remove_dir_all(&tmp).unwrap();
    }
}
