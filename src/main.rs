//! Dot Box: le binaire qui installe et gère tout le reste (spécification: les pages Getting Started).

mod apps;
mod authelia;
mod authelia_cmd;
mod card;
mod catalog;
mod config;
mod crowdsec;
mod docker;
mod engine;
mod github;
mod init;
mod log;
mod out;
mod prompt;
mod secrets;
mod system;
mod traefik;
mod ui;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Self-hosted apps on Docker Compose, behind Traefik and Authelia.
#[derive(Parser)]
#[command(name = "dotbox", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// L'aide ne liste que les commandes qui marchent vraiment: les autres arriveront à mesure.
#[derive(Subcommand)]
enum Command {
    /// Set up the server: checks, sudo, the user that runs the apps, Docker, Traefik, Authelia and CrowdSec
    Init(init::Args),

    /// Install apps from the catalog, behind Traefik and Authelia
    Install(apps::InstallArgs),

    /// Remove an app, keeping its configuration (--purge deletes it)
    Remove(apps::RemoveArgs),

    /// Authelia, the login page: web password and verification codes
    Authelia {
        #[command(subcommand)]
        action: authelia_cmd::Action,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init(args) => init::run(&args),
        Command::Install(args) => apps::install(&args),
        Command::Remove(args) => apps::remove(&args),
        Command::Authelia { action } => authelia_cmd::run(&action),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // déjà affichée (dotbox init montre l'étape en échec et la fin du log)
        Err(e) if e.is_empty() => ExitCode::FAILURE,
        Err(e) => {
            out::error(&e);
            ExitCode::FAILURE
        }
    }
}
