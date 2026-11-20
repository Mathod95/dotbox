//! `sudo dotbox exec` et `sudo dotbox edit` (Journal, 14.1 et 14.2): les commandes qui valent
//! root (D80), en root seulement, sans le service. Les apps seulement, le core se gère par init.

use crate::config::Config;
use crate::out::say;
use crate::status::CORE;
use crate::{apps, catalog, engine, system, ui};
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Root, une app (pas le core), installée.
fn root_app(cmd: &str, app: &str) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err(format!(
            "dotbox {cmd} is for root only (it could reach the whole server): sudo dotbox {cmd} {app}"
        ));
    }
    if CORE.contains(&app) {
        return Err(format!(
            "{app} is part of the core: sudo dotbox init manages it"
        ));
    }
    if !catalog::valid_name(app) {
        return Err(format!("{app:?} is not an app name"));
    }
    if Config::load()?.app(app).is_none() || !Path::new(&engine::compose_path(app)).exists() {
        return Err(format!("{app} is not installed: dotbox install {app}"));
    }
    Ok(())
}

// ---------- 14.1 sudo dotbox exec <app> -- <commande>

/// Les arguments de docker exec: -i toujours (une entrée envoyée par un tube passe), -t avec un
/// terminal, et sh sans commande.
pub fn exec_args(app: &str, command: &[String], tty: bool) -> Vec<String> {
    let mut a = vec!["exec".to_string(), "-i".into()];
    if tty {
        a.push("-t".into());
    }
    a.push(app.into());
    if command.is_empty() {
        a.push("sh".into());
    } else {
        a.extend(command.iter().cloned());
    }
    a
}

/// Lance la commande dans le container, et sort avec son code.
pub fn exec(app: &str, command: &[String]) -> Result<(), String> {
    root_app("exec", app)?;
    let state = crate::traefik::container_state(app);
    if !apps::running(&state) {
        return Err(format!(
            "{app} is not running ({state}): dotbox start {app}"
        ));
    }
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let status = Command::new("docker")
        .args(exec_args(app, command, tty))
        .status()
        .map_err(|e| format!("cannot run docker: {e}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

// ---------- 14.2 sudo dotbox edit <app>

/// Le fichier proposé quand l'app n'a pas encore de compose.override.yml.
pub fn template(app: &str) -> String {
    format!(
        "# compose.override.yml de {app}: tes changements à son compose.yml.\n\
         # Dot Box réécrit compose.yml à chaque install et update, jamais ce fichier.\n\
         # Enregistre et ferme l'éditeur: dotbox edit le vérifie, puis relance {app}.\n\
         # Sans changement, rien n'est fait.\n\
         #\n\
         # Exemple:\n\
         # services:\n\
         #   {app}:\n\
         #     environment:\n\
         #       TZ: Europe/Paris\n"
    )
}

/// Un fichier qui ne contient que des commentaires et des lignes vides ne change rien.
pub fn only_comments(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .all(|l| l.is_empty() || l.starts_with('#'))
}

/// L'éditeur: $VISUAL, $EDITOR (sudo les garde rarement), sinon nano, sinon vi.
fn editor() -> String {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(e) = std::env::var(var) {
            if !e.trim().is_empty() {
                return e;
            }
        }
    }
    if system::run("sh", &["-c", "command -v nano"]).is_ok() {
        "nano".into()
    } else {
        "vi".into()
    }
}

/// Ouvre le fichier dans l'éditeur, qui peut avoir ses options (« code --wait »).
fn open_editor(path: &str) -> Result<(), String> {
    let ed = editor();
    let status = Command::new("sh")
        .args(["-c", &format!("{ed} \"$1\""), "sh", path])
        .status()
        .map_err(|e| format!("cannot run {ed}: {e}"))?;
    if !status.success() {
        return Err(format!("{ed} ended with an error, nothing changed"));
    }
    Ok(())
}

fn write(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))
}

/// Remet le fichier d'avant (ou aucun), et relance l'app avec.
fn put_back(app: &str, path: &str, before: &Option<String>) {
    match before {
        Some(text) => {
            let _ = write(path, text);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
    say("restore", &format!("{path} as it was"));
    let _ = apps::compose(app, &["up", "-d", "--quiet-pull"]);
}

pub fn edit(app: &str, verbose: bool) -> Result<(), String> {
    root_app("edit", app)?;
    let path = format!("{}/compose.override.yml", engine::dir(app));
    let before = std::fs::read_to_string(&path).ok();
    // l'éditeur travaille sur une copie: le vrai fichier ne change qu'une fois vérifié
    let work = format!("{}/.compose.override.yml.edit", engine::dir(app));
    let start = before.clone().unwrap_or_else(|| template(app));
    write(&work, &start)?;
    let edited = open_editor(&work).and_then(|_| {
        std::fs::read_to_string(&work).map_err(|e| format!("cannot read {work}: {e}"))
    });
    let _ = std::fs::remove_file(&work);
    let after = edited?;
    let unchanged = match &before {
        Some(b) => *b == after,
        None => only_comments(&after),
    };
    if unchanged {
        eprintln!("  compose.override.yml of {app} unchanged, nothing done");
        return Ok(());
    }
    apps::begin("edit", app, 1, verbose)?;
    ui::step(&apps::title(app), "compose.override.yml");
    apps::finish((|| {
        // vide, ou que des commentaires: le fichier est retiré, l'app revient à son compose.yml
        if only_comments(&after) {
            let _ = std::fs::remove_file(&path);
            say("remove", &format!("{path} (nothing left in it)"));
        } else {
            write(&path, &after)?;
            say("write", &path);
            if let Err(e) = apps::compose(app, &["config", "-q"]) {
                put_back(app, &path, &before);
                return Err(format!(
                    "docker compose refuses it, the old file is back:\n{e}"
                ));
            }
            say("check", "docker compose config");
        }
        if let Err(e) = apps::compose(app, &["up", "-d", "--quiet-pull"]) {
            put_back(app, &path, &before);
            return Err(format!("{app} did not start, the old file is back:\n{e}"));
        }
        say("up", app);
        match apps::wait_ready(app, has_health(app), Duration::from_secs(180)) {
            Ok(state) => {
                ui::done(&format!("applied, {state}"));
                Ok(())
            }
            Err(e) => {
                put_back(app, &path, &before);
                Err(format!("{e}\nthe old file is back"))
            }
        }
    })())
}

/// Le container a-t-il un healthcheck (le sien ou celui de son image)?
fn has_health(app: &str) -> bool {
    system::run(
        "docker",
        &["inspect", "-f", "{{if .Config.Healthcheck}}yes{{end}}", app],
    )
    .is_ok_and(|s| s == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn exec_arguments() {
        assert_eq!(
            exec_args("radarr", &s(&["ls", "/data/media"]), false),
            s(&["exec", "-i", "radarr", "ls", "/data/media"])
        );
        assert_eq!(
            exec_args("radarr", &[], true),
            s(&["exec", "-i", "-t", "radarr", "sh"])
        );
    }

    #[test]
    fn comments_only() {
        assert!(only_comments(&template("qbittorrent")));
        assert!(only_comments("\n  \n# rien\n"));
        assert!(!only_comments(
            "services:\n  qbittorrent:\n    environment:\n      TZ: Europe/Paris\n"
        ));
    }
}
