//! Dot Box: le binaire qui installe et gère tout le reste (spécification: les pages Getting Started).

mod docker;
mod init;
mod out;
mod prompt;
mod system;

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
    /// Set up the server: system check, sudo, the user that runs the apps and Docker
    Init(init::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init(args) => init::run(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            out::error(&e);
            ExitCode::FAILURE
        }
    }
}
