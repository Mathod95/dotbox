//! `dotbox init` (décision D8 du Journal): vérifier le système, installer sudo, préparer
//! l'utilisateur qui fait tourner les apps, puis Docker (phase C). Traefik, Authelia et CrowdSec
//! viendront ensuite. Relancé, il ne refait que ce qui manque.

use crate::out::say;
use crate::{docker, prompt, system};

#[derive(clap::Args)]
pub struct Args {
    /// User that runs the apps and Dot Box (asked if not given)
    #[arg(long, value_name = "NAME")]
    user: Option<String>,
}

pub fn run(args: &Args) -> Result<(), String> {
    // 1. root
    if system::effective_uid()? != 0 {
        return Err("run dotbox init as root: su -, then run it again".into());
    }
    say("check", "root");

    // 2. système pris en charge, sinon rien n'est modifié
    let os = system::os_release()?;
    let arch = system::architecture()?;
    if !system::supported(&os) || arch != "amd64" {
        return Err(format!(
            "{} {arch} is not supported: Debian 12 or 13, Ubuntu 24.04 or 26.04, amd64 only",
            os.pretty
        ));
    }
    say("check", &format!("{} {arch}", os.pretty));

    // 3. sudo
    if system::package_installed("sudo") {
        say("check", "sudo");
    } else {
        say("install", "sudo");
        system::apt_install("sudo")?;
    }

    // 4. l'utilisateur qui fait tourner les apps
    let name = match &args.user {
        Some(n) => n.clone(),
        None => ask_user()?,
    };
    if name == "root" {
        return Err("the apps do not run as root: choose another user".into());
    }
    if !system::valid_username(&name) {
        return Err(format!(
            "invalid user name {name:?}: lowercase letters, digits, _ and - only"
        ));
    }
    if system::user_exists(&name) {
        say("check", &format!("user {name}"));
    } else {
        create_user(&name)?;
    }
    if system::user_groups(&name)?.iter().any(|g| g == "sudo") {
        say("check", &format!("{name} in group sudo"));
    } else {
        system::run("usermod", &["-aG", "sudo", &name])?;
        say("group", &format!("{name} added to sudo"));
    }

    // 5. Docker
    docker::setup(&os, &arch, &name)?;

    // 6. la suite n'existe pas encore
    say(
        "next",
        "Traefik, Authelia and CrowdSec: not available yet in this version",
    );
    Ok(())
}

/// Demande le nom dans le terminal. Entrée accepte le nom proposé (le compte connecté avant su).
fn ask_user() -> Result<String, String> {
    if !prompt::is_terminal() {
        return Err("no terminal to ask for the user: pass --user <name>".into());
    }
    let default = system::login_user();
    loop {
        let question = match &default {
            Some(d) => format!("user that runs the apps [{d}]:"),
            None => "user that runs the apps:".to_string(),
        };
        let Some(line) = prompt::ask("user", &question)? else {
            return Err("no answer: pass --user <name>".into());
        };
        match (line.as_str(), &default) {
            ("", Some(d)) => return Ok(d.clone()),
            ("", None) => continue,
            (a, _) => return Ok(a.to_string()),
        }
    }
}

/// Crée l'utilisateur et lui fait choisir son mot de passe (passwd, dans le terminal).
fn create_user(name: &str) -> Result<(), String> {
    if !prompt::is_terminal() {
        return Err(format!(
            "user {name} does not exist and there is no terminal to set its password"
        ));
    }
    say("create", &format!("user {name}"));
    system::run("useradd", &["--create-home", "--shell", "/bin/bash", name])?;
    let ok = system::command("passwd")
        .arg(name)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(format!(
            "password not set: run passwd {name}, then dotbox init again"
        ));
    }
    Ok(())
}
