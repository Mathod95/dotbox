//! `dotbox user`: les comptes Authelia (spec: page CLI, `dotbox user add|list|remove|passwd`).
//! Pour l'instant: `dotbox user password`, qui relit le mot de passe gardé par init (Journal, D25).

use crate::config::Config;
use crate::out::say;
use crate::{authelia, secrets, system};

#[derive(clap::Subcommand)]
pub enum Action {
    /// Show the password dotbox init set for an Authelia account (root only)
    Password {
        /// Account name (default: the user of dotbox.yml)
        name: Option<String>,
    },
}

pub fn run(action: &Action) -> Result<(), String> {
    match action {
        Action::Password { name } => password(name.clone()),
    }
}

fn password(name: Option<String>) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err("the password is readable by root only: sudo dotbox user password".into());
    }
    let name = match name.or_else(|| Config::load().ok().and_then(|c| c.get("user"))) {
        Some(n) => n,
        None => return Err("no account given and no user in /opt/dotbox/dotbox.yml".into()),
    };
    match secrets::read(&authelia::password_secret(&name)) {
        Some(pw) => {
            say("password", &format!("{pw}   ({name}, set by dotbox init)"));
            say(
                "note",
                "if it was changed in Authelia since, this one no longer works",
            );
            Ok(())
        }
        None => Err(format!(
            "no password kept for {name}: the account was created before Dot Box kept passwords, or by hand"
        )),
    }
}
