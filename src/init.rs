//! `dotbox init` (décision D8 du Journal): vérifier le système, le domaine et les ports, installer
//! sudo, préparer l'utilisateur qui fait tourner les apps, puis Docker (étape 3) et Traefik
//! (étape 4), Authelia (étape 5) et CrowdSec (étape 10), aux versions du catalogue lu sur GitHub
//! juste après les vérifications (Journal, 7.6, 7.12, D64). Les réponses sont gardées dans
//! /opt/dotbox/dotbox.yml: relancé, il ne redemande rien et ne refait que ce qui manque.
//! L'affichage suit le mockup final de CLI Output (étape 6): une ligne par étape, le détail dans
//! /opt/dotbox/logs/, une carte à la fin.

use crate::card::{self, Component};
use crate::catalog::{self, Core};
use crate::config::{self, Config};
use crate::out::say;
use crate::{
    authelia, backup, crowdsec, docker, github, log, prompt, secrets, serve, system, traefik, ui,
};

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

    /// Read the catalog from this folder instead of GitHub (for tests)
    #[arg(long, value_name = "DIR")]
    catalog: Option<std::path::PathBuf>,

    /// Show every sub-step as it happens (they are always in the log)
    #[arg(long)]
    verbose: bool,
}

pub fn run(args: &Args) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err("run dotbox init as root: su -, then run it again".into());
    }
    // le log de ce lancement (D37), avant toute autre commande
    let stamp = system::run("date", &["+%Y-%m-%d-%H%M"])?;
    let now = system::run("date", &["+%Y-%m-%d %H:%M:%S %Z"]).unwrap_or_default();
    log::open(
        "init",
        &stamp,
        &format!("dotbox init {}, started {now}", env!("CARGO_PKG_VERSION")),
    )?;
    ui::start(args.verbose);
    match steps(args) {
        Ok(()) => Ok(()),
        // l'erreur est déjà affichée par ui::fail, avec la fin du log: rien à ajouter
        Err(e) if ui::fail(&e) => Err(String::new()),
        Err(e) => Err(e),
    }
}

/// « Debian 13 », « Ubuntu 24.04 ».
fn short_os(os: &system::OsRelease) -> String {
    let mut id = os.id.clone();
    if let Some(c) = id.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    format!("{id} {}", os.version_id)
}

/// Le résumé de l'étape System: sans les ports s'il ne tient pas sur la ligne.
fn system_summary(os: &str, arch: &str, domain: &str) -> String {
    let full = format!("{os} {arch}, *.{domain}, ports 80 443");
    if full.chars().count() <= ui::SUMMARY_W {
        full
    } else {
        format!("{os} {arch}, *.{domain}")
    }
}

/// La version d'une image: « traefik:v3.7.19 » donne « v3.7.19 ».
fn tag(image: &str) -> String {
    image.rsplit(':').next().unwrap_or("").to_string()
}

fn steps(args: &Args) -> Result<(), String> {
    // 1. le système, le domaine et les ports: tout est vérifié avant de modifier quoi que ce soit
    ui::step("System", "checking this server");
    let os = system::os_release()?;
    let arch = system::architecture()?;
    if !system::supported(&os) || arch != "amd64" {
        return Err(format!(
            "{} {arch} is not supported: Debian 12 or 13, Ubuntu 24.04 or 26.04, amd64 only",
            os.pretty
        ));
    }
    say("check", &format!("{} {arch}", os.pretty));
    let mut cfg = Config::load()?;
    let domain = match args.domain.clone().or_else(|| cfg.get("domain")) {
        Some(d) => {
            if !config::valid_domain(&d) {
                return Err(format!(
                    "invalid domain {d:?}: for example example.com or dotbox.example.com"
                ));
            }
            traefik::check_dns(&d)?;
            d
        }
        None => {
            let d = ui::ask(ask_domain)?;
            say("check", &format!("*.{d} points to this server"));
            d
        }
    };
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
    traefik::check_ports()?;
    ui::done(&system_summary(&short_os(&os), &arch, &domain));

    // 2. le catalogue: les versions du core et des apps, lues sur GitHub (D64); tout le core y
    // est lu et vérifié avant d'installer quoi que ce soit
    ui::step("Catalog", "versions of the core and the apps");
    let (dir, from) = match &args.catalog {
        Some(dir) => {
            say("check", &format!("local catalog {}", dir.display()));
            (dir.clone(), format!("local {}", dir.display()))
        }
        None => {
            let source = github::fetch()?;
            (
                std::path::PathBuf::from(github::DIR),
                format!("{} of {}", source.short(), source.date),
            )
        }
    };
    let load = |name: &str| -> Result<Core, String> {
        let core = catalog::load_core(&dir, name)?;
        say(
            "check",
            &format!(
                "core/{name}.yml: {}",
                core.images
                    .iter()
                    .map(|(_, i)| i.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        Ok(core)
    };
    let (traefik_core, authelia_core, crowdsec_core) =
        (load("traefik")?, load("authelia")?, load("crowdsec")?);
    // le plugin de Traefik, dans son propre manifest (D70): sans image, une archive
    let plugin_core = catalog::load_core(&dir, "crowdsec-plugin")?;
    let plugin = plugin_core.file("traefik-plugin")?;
    say(
        "check",
        &format!(
            "core/crowdsec-plugin.yml: traefik-plugin {}",
            plugin.version
        ),
    );
    let bouncer_version = crowdsec_core.file("firewall-bouncer")?.version.clone();
    let apps = catalog::app_names(&dir).len();
    ui::done(&format!("{from}, {apps} apps"));

    // 3. sudo, l'utilisateur qui fait tourner les apps, son mot de passe Authelia
    ui::step("User", "the account that runs the apps");
    if system::package_installed("sudo") {
        say("check", "sudo");
    } else {
        say("install", "sudo");
        system::apt_install("sudo")?;
    }
    let name = match args.user.clone().or_else(|| cfg.get("user")) {
        Some(n) => n,
        None => ui::ask(ask_user)?,
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
    // le groupe dotbox: la CLI parle au service sans sudo (D79). Pas le groupe docker (D33).
    if system::run("getent", &["group", serve::GROUP]).is_err() {
        system::run("groupadd", &["--system", serve::GROUP])?;
        say("create", &format!("group {}", serve::GROUP));
    }
    if system::user_groups(&name)?
        .iter()
        .any(|g| g == serve::GROUP)
    {
        say("check", &format!("{name} in group {}", serve::GROUP));
    } else {
        system::run("usermod", &["-aG", serve::GROUP, &name])?;
        say("group", &format!("{name} added to {}", serve::GROUP));
    }
    // le mot de passe Authelia, demandé seulement si le compte n'existe pas encore (D34, D39);
    // sans terminal, il est tiré au hasard
    let chosen = if !authelia::account_exists(&name) && prompt::is_terminal() {
        Some(ui::ask(|| {
            prompt::password(
                "Authelia password, 12 characters or more",
                "Enter alone for a random one",
                authelia::password_ok,
            )
        })?)
    } else {
        None
    };
    ui::done(&format!("{name}, groups sudo and dotbox"));

    // 4. Docker
    ui::step("Docker", "official repository");
    let docker_version = docker::setup(&os, &arch)?;
    ui::done(&format!("{docker_version}, local logs, live-restore"));

    // 5. dotbox.yml, puis Traefik
    ui::step("Traefik", "certificates and routing");
    cfg.set("domain", &domain);
    cfg.set("ca", &ca);
    cfg.set("user", &name);
    if system::write_if_changed(config::PATH, &cfg.render()?, 0o644)? {
        say("write", config::PATH);
    }
    // la porte web d'abord (11.13): Traefik la lit au démarrage
    traefik::write_web_route(&domain)?;
    traefik::setup(&domain, &ca, &traefik_core, plugin)?;
    let traefik_image = traefik_core.image("traefik")?;
    let certs = if ca == "letsencrypt-staging" {
        "staging certs"
    } else {
        "Let's Encrypt certs"
    };
    ui::done(&format!("{}, socket proxy, {certs}", tag(traefik_image)));

    // 6. Authelia (portail auth.<domaine>, un compte au nom de l'utilisateur)
    ui::step("Authelia", "login page, password and 6-digit code");
    let password = authelia::setup(&domain, &name, chosen.as_deref(), &authelia_core)?;
    let authelia_image = authelia_core.image("authelia")?;
    ui::done(&format!("{}, account {name}", tag(authelia_image)));

    // 7. CrowdSec: l'agent en container, le bouncer pare-feu sur le système (étape 10)
    ui::step("CrowdSec", "attack detection and firewall bouncer");
    // son socket proxy (les logs des apps, 10.9): la même image que celui de Traefik
    crowdsec::setup(&crowdsec_core, traefik_core.image("socket-proxy")?)?;
    let crowdsec_version = crowdsec::version(crowdsec_core.image("crowdsec")?);
    ui::done(&format!(
        "{crowdsec_version}, SSH, Traefik and AppSec, bouncer {bouncer_version}"
    ));

    // 8. le service de Dot Box, que la CLI appelle (D79, 11.7)
    ui::step("Service", "the Dot Box service, behind the dotbox command");
    let service = serve::setup()?;
    // la sauvegarde (16.2, 16.6, D97): sa clé, et le timer qui suit backup.schedule
    if backup::ensure_key()? {
        say("create", backup::KEY);
    } else {
        say("check", backup::KEY);
    }
    let planned = backup::setup_timer(&backup::Settings::of(&cfg)?)?;
    ui::done(&format!("{service}, {planned}"));

    // la carte de fin (D26, D32, D37)
    let docker_state =
        if system::run("systemctl", &["is-active", "docker"]).is_ok_and(|s| s == "active") {
            "running".to_string()
        } else {
            "stopped".to_string()
        };
    let info = card::Info {
        password_kept: secrets::read(&authelia::password_secret(&name)).is_some(),
        user: name,
        password,
        log: log::path().unwrap_or_default(),
        components: vec![
            Component {
                name: "Docker".into(),
                version: docker_version,
                state: docker_state,
                address: String::new(),
            },
            Component {
                name: "Traefik".into(),
                version: tag(traefik_image),
                state: traefik::container_state("traefik"),
                address: format!("https://traefik.{domain}"),
            },
            Component {
                name: "Authelia".into(),
                version: tag(authelia_image),
                state: traefik::container_state("authelia"),
                address: format!("https://auth.{domain}"),
            },
            Component {
                name: "CrowdSec".into(),
                version: crowdsec_version,
                state: crowdsec::state(),
                address: String::new(),
            },
            Component {
                name: "Dot Box".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                state: match system::run("systemctl", &["is-active", "dotbox"]) {
                    Ok(s) if s == "active" => "running".into(),
                    Ok(s) => s,
                    Err(_) => String::new(),
                },
                address: format!("https://dotbox.{domain}"),
            },
        ],
        elapsed: ui::duration(ui::elapsed()),
    };
    log::write("init finished");
    let term = console::Term::stderr();
    let _ = term.write_line("");
    for line in card::render(&info) {
        let _ = term.write_line(&line);
    }
    let _ = term.write_line("");
    Ok(())
}

/// Demande le domaine. La réponse est vérifiée tout de suite: si *.domaine ne pointe pas vers
/// ce serveur, elle est refusée avec la raison et la question reposée.
fn ask_domain() -> Result<String, String> {
    if !prompt::is_terminal() {
        return Err("no terminal to ask for the domain: pass --domain <domain>".into());
    }
    let d = prompt::input(
        "Domain of this server",
        "*.domain must point to this server",
        None,
        |answer| {
            let d = answer.to_lowercase();
            if d.is_empty() {
                Err("a domain is needed, for example dotbox.example.com".into())
            } else if !config::valid_domain(&d) {
                Err(format!(
                    "{d} is not a domain, for example dotbox.example.com"
                ))
            } else {
                traefik::dns_problem(&d).map_or(Ok(()), Err)
            }
        },
    )?;
    Ok(d.to_lowercase())
}

/// Demande l'utilisateur. Le compte connecté avant su est proposé, Entrée le garde.
fn ask_user() -> Result<String, String> {
    if !prompt::is_terminal() {
        return Err("no terminal to ask for the user: pass --user <name>".into());
    }
    let default = system::login_user();
    prompt::input(
        "User that runs the apps",
        "created if it does not exist",
        default.as_deref(),
        |answer| {
            if answer == "root" {
                Err("the apps do not run as root: choose another user".into())
            } else if !system::valid_username(answer) {
                Err("lowercase letters, digits, _ and - only".into())
            } else {
                Ok(())
            }
        },
    )
}

/// Crée l'utilisateur avec le mot de passe Linux choisi (deux fois, masqué), appliqué par chpasswd.
fn create_user(name: &str) -> Result<(), String> {
    if !prompt::is_terminal() {
        return Err(format!(
            "user {name} does not exist and there is no terminal to set its password"
        ));
    }
    let question = format!("Linux password for {name}");
    let pw = ui::ask(|| {
        let first = prompt::password(&question, "for sudo and SSH", |p| {
            if p.is_empty() {
                Err("a password is needed".into())
            } else {
                Ok(())
            }
        })?;
        let again = first.clone();
        prompt::password("Again", "the same password", move |p| {
            if p == again {
                Ok(())
            } else {
                Err("not the same password".into())
            }
        })?;
        Ok(first)
    })?;
    system::run("useradd", &["--create-home", "--shell", "/bin/bash", name])?;
    say("create", &format!("user {name}"));
    system::run_with_input("chpasswd", &[], &format!("{name}:{pw}\n"))?;
    say("password", &format!("set for {name}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_summary_fits() {
        assert_eq!(
            system_summary("Debian 13", "amd64", "example.com"),
            "Debian 13 amd64, *.example.com, ports 80 443"
        );
        assert_eq!(
            system_summary("Debian 13", "amd64", "dotbox.example.com"),
            "Debian 13 amd64, *.dotbox.example.com"
        );
    }

    #[test]
    fn os_and_tags() {
        let os = system::parse_os_release("ID=ubuntu\nVERSION_ID=\"24.04\"\n");
        assert_eq!(short_os(&os), "Ubuntu 24.04");
        assert_eq!(tag("traefik:v3.7.19"), "v3.7.19");
        assert_eq!(
            tag("lscr.io/linuxserver/socket-proxy:3.4.7-r0-ls106"),
            "3.4.7-r0-ls106"
        );
    }
}
