//! Dot Box: le binaire qui installe et gère tout le reste (spécification: les pages Getting Started).

mod add;
mod apps;
mod authelia;
mod authelia_cmd;
mod card;
mod catalog;
mod clack;
mod client;
mod config;
mod crowdsec;
mod docker;
mod doctor;
mod engine;
mod github;
mod help;
mod init;
mod log;
mod manage;
mod out;
mod prompt;
mod rootcmd;
mod secrets;
mod serve;
mod status;
mod system;
mod traefik;
mod ui;
mod upgrade;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Self-hosted apps on Docker Compose, behind Traefik and Authelia.
#[derive(Parser)]
#[command(name = "dotbox", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Do the work in this command, as root, without the Dot Box service (rescue over SSH)
    #[arg(long, global = true)]
    local: bool,

    /// JSON output, for scripts (list, info, status, doctor, diff, update, version)
    #[arg(long, global = true)]
    json: bool,

    /// Answer yes to confirmations
    #[arg(short = 'y', long, global = true)]
    yes: bool,

    /// Only print errors
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Output without colors (NO_COLOR is respected too)
    #[arg(long, global = true)]
    no_color: bool,
}

// L'aide ne liste que les commandes qui marchent vraiment: les autres arriveront à mesure.
#[derive(Subcommand)]
enum Command {
    /// Set up the server: checks, sudo, the user that runs the apps, Docker, Traefik, Authelia, CrowdSec and the Dot Box service
    Init(init::Args),

    /// The core and the apps: version, state, address, and the last doctor's summary
    Status,

    /// Check the server and say what to do: Docker, DNS, certificates, CrowdSec, updates, SSH, disk
    Doctor,

    /// The catalog, with the installed apps
    List {
        /// Only the apps whose name or role contains this
        search: Option<String>,
    },

    /// What Dot Box knows about an app: image, state, address, access, folders, ports
    Info {
        #[arg(value_name = "APP")]
        app: String,
    },

    /// Install apps from the catalog, behind Traefik and Authelia
    Install(apps::InstallArgs),

    /// Remove an app, keeping its configuration (--purge deletes it)
    Remove(apps::RemoveArgs),

    /// Start an app
    Start(AppArg),

    /// Stop an app
    Stop(AppArg),

    /// Restart an app
    Restart(AppArg),

    /// An app's logs, or a core container's (traefik, authelia, crowdsec)
    Logs {
        #[arg(value_name = "APP")]
        app: String,
        /// Follow them live, until Ctrl+C
        #[arg(short, long)]
        follow: bool,
        /// How many lines from the end
        #[arg(long, default_value_t = 100)]
        tail: u32,
    },

    /// Run a command in an app's container, or a shell without one (root)
    Exec {
        #[arg(value_name = "APP")]
        app: String,
        /// The command and its arguments, after --
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "COMMAND"
        )]
        command: Vec<String>,
    },

    /// Open the app's compose.override.yml in your editor, check it, then apply it (root)
    Edit(AppArg),

    /// Turn a compose file into an app of your own, in /opt/dotbox/apps (root)
    Add {
        /// The compose file (one service for now)
        #[arg(value_name = "COMPOSE.YML")]
        file: String,
        /// The app's name, also its subdomain
        #[arg(long, value_name = "APP")]
        name: String,
        /// The web port in the container, when the compose does not say it
        #[arg(long)]
        port: Option<u16>,
    },

    /// Read the catalog on GitHub and update apps, one at a time (no app: only say what is behind)
    Update {
        #[arg(value_name = "APP")]
        apps: Vec<String>,
        /// Every app that is behind
        #[arg(long)]
        all: bool,
        /// Put back the image from before the last update of this app
        #[arg(long)]
        rollback: bool,
        /// Show every sub-step as it happens (they are always in the log)
        #[arg(long)]
        verbose: bool,
    },

    /// The gap between what is wanted and what runs, without changing anything
    Diff,

    /// The version of this command and of the Dot Box service
    Version,

    /// Update Dot Box itself to the latest release (root)
    Upgrade {
        /// Show every sub-step as it happens (they are always in the log)
        #[arg(long)]
        verbose: bool,
    },

    /// The shell completion script (bash, zsh or fish), on standard output
    Completion { shell: clap_complete::Shell },

    /// Authelia, the login page: web password and verification codes
    Authelia {
        #[command(subcommand)]
        action: authelia_cmd::Action,
    },

    /// Run the Dot Box service, which the CLI talks to (started by systemd: dotbox.service)
    // Caché de l'aide: on ne la tape pas soi-même (choix de Mathod95 du 20 novembre 2026).
    #[command(hide = true)]
    Serve,
}

#[derive(clap::Args)]
struct AppArg {
    #[arg(value_name = "APP")]
    app: String,
    /// Show every sub-step as it happens (they are always in the log)
    #[arg(long)]
    verbose: bool,
}

fn json_or<T: serde::Serialize>(json: bool, v: &T, print: impl FnOnce(&T)) {
    if json {
        clack::put(&serde_json::to_string_pretty(v).unwrap_or_default());
    } else {
        print(v);
    }
}

/// --local: le travail se fait ici, ce qui demande root.
fn local_root(what: &str) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err(format!("--local works as root: sudo dotbox {what} --local"));
    }
    Ok(())
}

fn local_status(json: bool) -> Result<(), String> {
    local_root("status")?;
    let st = status::collect()?;
    if json {
        clack::put(&serde_json::to_string_pretty(&st).unwrap_or_default());
    } else {
        status::print(&st);
    }
    Ok(())
}

fn local_doctor(json: bool) -> Result<(), String> {
    local_root("doctor")?;
    let r = clack::waiting("checking the server", doctor::run)?;
    if json {
        clack::put(&serde_json::to_string_pretty(&r).unwrap_or_default());
    } else {
        doctor::print(&r);
    }
    if r.problems() > 0 {
        Err(String::new())
    } else {
        Ok(())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(asked) = help::wanted(&args) {
        if args.iter().any(|a| a == "--no-color")
            || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
        {
            console::set_colors_enabled(false);
            console::set_colors_enabled_stderr(false);
        }
        // Demandée: sur la sortie standard. Sans commande: sur la sortie d'erreur, code 2, comme clap.
        if asked {
            print!("{}", help::text());
            return ExitCode::SUCCESS;
        }
        let text = help::text();
        if console::colors_enabled_stderr() {
            eprint!("{text}");
        } else {
            eprint!("{}", console::strip_ansi_codes(&text));
        }
        return ExitCode::from(2);
    }
    let cli = Cli::parse();
    let (local, json) = (cli.local, cli.json);
    out::set_quiet(cli.quiet);
    if cli.no_color || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
    let result = match cli.command {
        Command::Init(args) => init::run(&args),
        Command::Status if local => local_status(json),
        Command::Status => client::status(json),
        Command::Doctor if local => local_doctor(json),
        Command::Doctor => client::doctor(json),
        Command::List { search } if local => local_root("list").and_then(|_| {
            manage::list(search.as_deref().unwrap_or_default())
                .map(|l| json_or(json, &l, |l| manage::print_list(l)))
        }),
        Command::List { search } => client::list(search.as_deref().unwrap_or_default(), json),
        Command::Info { app } if local => local_root("info")
            .and_then(|_| manage::info(&app).map(|i| json_or(json, &i, manage::print_info))),
        Command::Info { app } => client::info(&app, json),
        Command::Install(args) if local => apps::install(&args),
        Command::Install(args) => client::install(&args),
        Command::Remove(mut args) if local => {
            args.yes = cli.yes;
            apps::remove(&args)
        }
        Command::Remove(mut args) => {
            args.yes = cli.yes;
            client::remove(&args)
        }
        Command::Start(a) if local => manage::lifecycle("start", &a.app, a.verbose),
        Command::Start(a) => client::lifecycle("start", &a.app, a.verbose),
        Command::Stop(a) if local => manage::lifecycle("stop", &a.app, a.verbose),
        Command::Stop(a) => client::lifecycle("stop", &a.app, a.verbose),
        Command::Restart(a) if local => manage::lifecycle("restart", &a.app, a.verbose),
        Command::Restart(a) => client::lifecycle("restart", &a.app, a.verbose),
        Command::Logs { app, follow, tail } if local => {
            local_root("logs").and_then(|_| manage::logs_local(&app, tail, follow))
        }
        Command::Logs { app, follow, tail } => client::logs(&app, tail, follow),
        Command::Update {
            apps,
            all,
            rollback,
            verbose,
        } if local => manage::update(&apps, all, rollback, verbose),
        Command::Update {
            apps,
            all,
            rollback,
            verbose,
        } => client::update(&apps, all, rollback, verbose, json),
        Command::Exec { app, command } => rootcmd::exec(&app, &command),
        Command::Edit(a) => rootcmd::edit(&a.app, a.verbose),
        Command::Add { file, name, port } => add::run(&file, &name, port),
        Command::Diff if local => local_root("diff").and_then(|_| {
            let g = manage::diff()?;
            json_or(json, &g, |g| manage::print_diff(g));
            if g.is_empty() {
                Ok(())
            } else {
                Err(String::new())
            }
        }),
        Command::Diff => client::diff(json),
        Command::Version => client::version(json),
        Command::Upgrade { verbose } => upgrade::run(verbose),
        Command::Completion { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(shell, &mut Cli::command(), "dotbox", &mut std::io::stdout());
            Ok(())
        }
        Command::Authelia { action } => authelia_cmd::run(&action),
        Command::Serve => serve::run(),
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
