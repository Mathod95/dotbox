//! `sudo dotbox upgrade` (Journal, 13.8): Dot Box lui-même. La dernière release GitHub, son
//! SHA-256 vérifié comme le fait install.sh, l'ancien binaire gardé, puis le service redémarré.
//! En root, sans le service: il remplace son propre binaire. Ne touche ni aux apps ni au core.

use crate::out::say;
use crate::{apps, catalog, crowdsec, system, ui};

pub const REPO: &str = "Mathod95/dotbox";
const ASSET: &str = "dotbox-linux-amd64";
const BIN: &str = "/usr/local/bin/dotbox";
const PREVIOUS: &str = "/usr/local/bin/dotbox.previous";

/// « v0.0.5 » est-il plus récent que « 0.0.4 »?
pub fn newer(tag: &str, current: &str) -> bool {
    match (
        catalog::parse_version(tag.trim_start_matches('v')),
        catalog::parse_version(current),
    ) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Le tag de la dernière release, dans la réponse de l'API de GitHub.
pub fn tag_of(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("tag_name")?.as_str().map(String::from)
}

pub fn run(verbose: bool) -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err("dotbox upgrade replaces /usr/local/bin/dotbox: sudo dotbox upgrade".into());
    }
    apps::begin("upgrade", "", 1, verbose)?;
    ui::step("Dot Box", "the latest release on GitHub");
    let current = env!("CARGO_PKG_VERSION");
    let mut changed = false;
    let result = (|| {
        let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let tag = tag_of(&crate::github::get(&api)?).ok_or("GitHub gave no latest release")?;
        say("check", &format!("latest release {tag}, this is {current}"));
        if !newer(&tag, current) {
            ui::done(&format!("{current}, the latest"));
            return Ok(());
        }
        let latest = tag.trim_start_matches('v').to_string();
        let tmp = format!("/tmp/dotbox-upgrade-{}", std::process::id());
        std::fs::create_dir_all(&tmp).map_err(|e| format!("cannot create {tmp}: {e}"))?;
        let bin = format!("{tmp}/{ASSET}");
        let url = format!("https://github.com/{REPO}/releases/download/{tag}/{ASSET}");
        system::run("wget", &["-q", "-O", &bin, &url])?;
        system::run(
            "wget",
            &[
                "-q",
                "-O",
                &format!("{bin}.sha256"),
                &format!("{url}.sha256"),
            ],
        )?;
        let want = std::fs::read_to_string(format!("{bin}.sha256"))
            .ok()
            .and_then(|t| t.split_whitespace().next().map(String::from))
            .unwrap_or_default();
        let got = crowdsec::sha256(&bin).unwrap_or_default();
        if want.is_empty() || want != got {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "{ASSET} {tag}: its SHA-256 does not match, nothing changed"
            ));
        }
        say("check", &format!("{ASSET} {tag}, sha256 verified"));
        system::run("chmod", &["755", &bin])?;
        let says = system::run(&bin, &["--version"]).unwrap_or_default();
        if !says.contains(&latest) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "the new binary says {says:?}, not {latest}: nothing changed"
            ));
        }
        system::run("cp", &["-p", BIN, PREVIOUS])?;
        say("keep", PREVIOUS);
        system::run("install", &["-m", "755", &bin, BIN])?;
        say("install", &format!("{BIN} {latest}"));
        let _ = std::fs::remove_dir_all(&tmp);
        if system::run("systemctl", &["is-active", "dotbox"]).as_deref() == Ok("active") {
            system::run("systemctl", &["restart", "dotbox"])?;
            say("restart", "dotbox.service");
        }
        changed = true;
        ui::done(&format!("{current} → {latest}, service restarted"));
        Ok(())
    })();
    apps::finish(result)?;
    if changed {
        eprintln!();
        eprintln!(
            "  {} The apps and the core are untouched: sudo dotbox init brings the core to the catalog versions.",
            console::style("→").cyan()
        );
        eprintln!("    Back to the old binary: sudo install -m 755 {PREVIOUS} {BIN}");
        eprintln!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compared() {
        assert!(newer("v0.0.5", "0.0.4"));
        assert!(newer("v0.1.0", "0.0.9"));
        assert!(!newer("v0.0.4", "0.0.4"));
        assert!(!newer("v0.0.3", "0.0.4"));
        assert!(!newer("nightly", "0.0.4"));
    }

    #[test]
    fn latest_tag() {
        assert_eq!(
            tag_of(r#"{"tag_name": "v0.0.4", "name": "v0.0.4"}"#).as_deref(),
            Some("v0.0.4")
        );
        assert_eq!(tag_of("{}"), None);
    }
}
