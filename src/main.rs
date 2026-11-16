//! Dot Box: le binaire qui installe et gère tout le reste (spécification: les pages Getting Started).

mod authelia;
mod config;
mod docker;
mod init;
mod out;
mod prompt;
mod secrets;
mod system;
mod traefik;
mod user;

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
    /// Set up the server: checks, sudo, the user that runs the apps, Docker, Traefik and Authelia
    Init(init::Args),

    /// Manage Authelia accounts
    User {
        #[command(subcommand)]
        action: user::Action,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init(args) => init::run(&args),
        Command::User { action } => user::run(&action),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            out::error(&e);
            ExitCode::FAILURE
        }
    }
}
