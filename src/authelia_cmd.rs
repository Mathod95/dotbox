//! `dotbox authelia`: ce qui sert à se connecter à la page d'Authelia.
//! - password: relit le mot de passe web gardé par init (Journal, D25).
//! - verification: le dernier code de vérification d'identité écrit par Authelia dans un fichier
//!   (D21), celui qu'on tape avant d'enregistrer un appareil, et l'heure où il expire.
//!   Ce n'est jamais le code à 6 chiffres: celui-là, seule l'app du téléphone le connaît.

use crate::config::Config;
use crate::out::say;
use crate::{authelia, secrets, system};

pub const NOTIFICATIONS: &str = "/opt/authelia/config/notification.txt";

/// Durée de vie d'un code de vérification: le défaut d'Authelia
/// (identity_validation.elevated_session.code_lifespan, Journal Q25).
const CODE_LIFESPAN_SECS: i64 = 5 * 60;

#[derive(clap::Subcommand)]
pub enum Action {
    /// Show the web password dotbox init created for an Authelia account (root only)
    Password {
        /// Account name (default: the user of dotbox.yml)
        name: Option<String>,
    },
    /// Show the last verification code Authelia sent, asked before registering a device (root only)
    Verification,
}

pub fn run(action: &Action) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err("readable by root only: run it with sudo".into());
    }
    match action {
        Action::Password { name } => password(name.clone()),
        Action::Verification => verification(),
    }
}

fn password(name: Option<String>) -> Result<(), String> {
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

/// Le dernier code du fichier de notifications: (date écrite par Authelia, code).
pub fn last_code(text: &str) -> Option<(String, String)> {
    let mut date: Option<String> = None;
    let mut expect = false;
    let mut found = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(d) = l.strip_prefix("Date: ") {
            date = Some(d.to_string());
            expect = false;
        } else if l.contains("one-time code should only be used") {
            expect = true;
        } else if expect && !l.is_empty() && !l.chars().all(|c| c == '-') {
            // le code suit la phrase, entouré de lignes vides et de tirets
            if l.len() >= 6 && l.chars().all(|c| c.is_ascii_alphanumeric()) {
                found = date.clone().map(|d| (d, l.to_string()));
            }
            expect = false;
        }
    }
    found
}

/// « 2026-11-16 00:22:14.37599439 +0000 UTC m=+2120.5 » devient « 2026-11-16 00:22:14 +0000 »,
/// une forme que `date -d` comprend.
pub fn date_for_date_cmd(authelia_date: &str) -> Option<String> {
    let mut f = authelia_date.split_whitespace();
    let day = f.next()?;
    let time = f.next()?.split('.').next()?;
    let zone = f.next()?;
    Some(format!("{day} {time} {zone}"))
}

fn epoch(date: &str) -> Result<i64, String> {
    system::run("date", &["-d", date, "+%s"])?
        .parse()
        .map_err(|_| format!("cannot read the date {date:?}"))
}

fn clock(epoch: i64) -> String {
    system::run("date", &["-d", &format!("@{epoch}"), "+%H:%M"]).unwrap_or_default()
}

fn verification() -> Result<(), String> {
    let text = std::fs::read_to_string(NOTIFICATIONS).unwrap_or_default();
    let Some((date, code)) = last_code(&text) else {
        say(
            "code",
            "none yet: Authelia writes one when you click « Register device » in its settings",
        );
        return Ok(());
    };
    let created = date_for_date_cmd(&date)
        .ok_or_else(|| format!("cannot read the date {date:?}"))
        .and_then(|d| epoch(&d))?;
    let expires = created + CODE_LIFESPAN_SECS;
    let now = epoch("now")?;
    if now <= expires {
        say(
            "code",
            &format!(
                "{code}   (created {}, valid until {})",
                clock(created),
                clock(expires)
            ),
        );
    } else {
        say(
            "code",
            &format!(
                "{code}   (created {}, expired at {}: ask for a new one in the browser)",
                clock(created),
                clock(expires)
            ),
        );
    }
    say(
        "note",
        "this is not the 6-digit code: that one comes from the app on your phone",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // le format réel de notification.txt (Authelia 4.39.28), relevé sur le serveur de test
    const NOTE: &str = "Date: {date}\nRecipient: {admin admin@example.com}\nSubject: Confirm your identity\nA ONE-TIME CODE HAS BEEN GENERATED TO COMPLETE A REQUESTED ACTION\n\nHi admin,\n\nThe following one-time code should only be used in the prompt displayed in your browser.\n\n----------------------------------------\n\n{code}\n\n----------------------------------------\n\nIf you did not initiate the process your credentials may have been compromised and you should:\n\n 1. Revoke this code using the provided links below\n\nPowered by Authelia https://www.authelia.com\n\n";
    fn sample() -> String {
        NOTE.replace("{date}", "2026-11-16 00:19:01.1 +0000 UTC m=+1900.1")
            .replace("{code}", "T7TLKAT2")
            + &NOTE
                .replace(
                    "{date}",
                    "2026-11-16 00:22:14.37599439 +0000 UTC m=+2120.527175152",
                )
                .replace("{code}", "CQLM7VEH")
            + "Date: 2026-11-16 00:23:06.2 +0000 UTC m=+2172.2\nSubject: Second Factor Method Added\nA second factor was added.\n"
    }

    #[test]
    fn last_code_is_the_most_recent() {
        let (date, code) = last_code(&sample()).unwrap();
        assert_eq!(code, "CQLM7VEH");
        assert!(date.starts_with("2026-11-16 00:22:14"));
        assert!(last_code("").is_none());
        assert!(last_code("Date: 2026-11-16 00:00:00 +0000 UTC\nnothing\n").is_none());
    }

    #[test]
    fn date_for_the_date_command() {
        assert_eq!(
            date_for_date_cmd("2026-11-16 00:22:14.37599439 +0000 UTC m=+2120.527175152")
                .as_deref(),
            Some("2026-11-16 00:22:14 +0000")
        );
        assert_eq!(date_for_date_cmd("nonsense"), None);
    }
}
