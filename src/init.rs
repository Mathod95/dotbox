//! `dotbox init` (décision D8 du Journal): vérifier le système, le domaine et les ports, installer
//! sudo, préparer l'utilisateur qui fait tourner les apps, puis Docker (phase C) et Traefik
//! (phase T) et Authelia (phase A). CrowdSec viendra ensuite. Les réponses sont gardées dans
//! /opt/dotbox/dotbox.yml: relancé, il ne redemande rien et ne refait que ce qui manque.

use crate::config::{self, Config};
use crate::out::say;
use crate::{authelia, docker, prompt, secrets, system, traefik};

#[derive(clap::Args)]
pub struct Args {
    /// User that runs the apps and Dot Box (asked if not given)
    #[arg(long, value_name = "NAME")]
    user: Option<String>,

    /// Domain of the server: *.DOMAIN must point to it (asked if not given)
    #[arg(long, value_name = "DOMAIN")]
    domain: Option<String>,

    /// Certificate authority: letsencrypt, or letsencrypt-staging for tests
    #[arg(long, value_name = "CA", value_parser = config::CAS.to_vec())]
    ca: Option<String>,
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

    // 3. le domaine et l'autorité de certification (options, sinon dotbox.yml, sinon question),
    // puis le DNS et les ports: tout est vérifié avant de modifier quoi que ce soit
    let mut cfg = Config::load()?;
    let domain = match args.domain.clone().or_else(|| cfg.get("domain")) {
        Some(d) => d,
        None => ask_domain()?,
    };
    if !config::valid_domain(&domain) {
        return Err(format!(
            "invalid domain {domain:?}: for example example.com or dotbox.example.com"
        ));
    }
    let ca = args
        .ca
        .clone()
        .or_else(|| cfg.get("ca"))
        .unwrap_or_else(|| config::DEFAULT_CA.to_string());
    if !config::CAS.contains(&ca.as_str()) {
        return Err(format!(
            "unknown ca {ca:?} in {}: {}",
            config::PATH,
            config::CAS.join(" or ")
        ));
    }
    traefik::check(&domain)?;

    // 4. sudo
    if system::package_installed("sudo") {
        say("check", "sudo");
    } else {
        say("install", "sudo");
        system::apt_install("sudo")?;
    }

    // 5. l'utilisateur qui fait tourner les apps
    let name = match args.user.clone().or_else(|| cfg.get("user")) {
        Some(n) => n,
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

    // 6. Docker
    docker::setup(&os, &arch, &name)?;

    // 7. dotbox.yml, puis Traefik
    cfg.set("domain", &domain);
    cfg.set("ca", &ca);
    cfg.set("user", &name);
    if system::write_if_changed(config::PATH, &cfg.render()?, 0o644)? {
        say("write", config::PATH);
    }
    traefik::setup(&domain, &ca)?;

    // 8. Authelia (portail auth.<domaine>, un compte au nom de l'utilisateur)
    let password = authelia::setup(&domain, &name)?;

    // 9. le récapitulatif (Journal, D26)
    summary(&os.pretty, &arch, &domain, &ca, &name, password.as_deref());
    Ok(())
}

/// Ce qui tourne, comment s'en servir, et ce qui manque encore.
fn summary(os: &str, arch: &str, domain: &str, ca: &str, user: &str, password: Option<&str>) {
    let docker =
        system::run("docker", &["version", "--format", "{{.Server.Version}}"]).unwrap_or_default();
    let tag = |image: &str| image.rsplit(':').next().unwrap_or("").to_string();
    crate::out::blank();
    say("summary", "Dot Box core is running");
    say("server", &format!("{os} {arch}, Docker {docker}"));
    say("domain", &format!("{domain} (certificates: {ca})"));
    say(
        "traefik",
        &format!(
            "{}, dashboard https://traefik.{domain} (admins, two factors)",
            tag(traefik::IMAGE)
        ),
    );
    say(
        "authelia",
        &format!("{}, login page https://auth.{domain}", tag(authelia::IMAGE)),
    );
    say("login", &format!("{user} (group admins)"));
    match password {
        Some(pw) => say(
            "password",
            &format!("{pw}   (shown once, kept for root: sudo dotbox user password)"),
        ),
        None if secrets::read(&authelia::password_secret(user)).is_some() => {
            say("password", "kept for root: sudo dotbox user password")
        }
        None => say(
            "password",
            "not kept (account created before Dot Box kept passwords)",
        ),
    }
    say(
        "2fa",
        "at first login, register a 6-digit code app or a passkey",
    );
    say(
        "codes",
        "verification codes: sudo tail -20 /opt/authelia/config/notification.txt",
    );
    say("next", "CrowdSec: not available yet in this version");
}

/// Demande le domaine dans le terminal.
fn ask_domain() -> Result<String, String> {
    if !prompt::is_terminal() {
        return Err("no terminal to ask for the domain: pass --domain <domain>".into());
    }
    loop {
        let Some(line) = prompt::ask(
            "domain",
            "domain of this server (*.domain must point here):",
        )?
        else {
            return Err("no answer: pass --domain <domain>".into());
        };
        if !line.is_empty() {
            return Ok(line.to_lowercase());
        }
    }
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
