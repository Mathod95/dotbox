//! /opt/dotbox/secrets/: les secrets générés par dotbox (spec: Basics), lisibles par root seul.
//! Un secret est tiré une seule fois et n'est jamais réécrit: le changer casserait ce qui en dépend
//! (sessions, base chiffrée d'Authelia...).

use std::fs;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

pub const DIR: &str = "/opt/dotbox/secrets";
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Une chaîne aléatoire de `len` caractères alphanumériques, tirée de /dev/urandom
/// (rejet des octets hors de la plage multiple de 62, pour ne favoriser aucun caractère).
pub fn random(len: usize) -> Result<String, String> {
    let mut urandom =
        fs::File::open("/dev/urandom").map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
    let limit = 256 - (256 % ALPHABET.len());
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 64];
    while out.len() < len {
        urandom
            .read_exact(&mut buf)
            .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
        for b in buf {
            if (b as usize) < limit && out.len() < len {
                out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
            }
        }
    }
    Ok(out)
}

pub fn path(name: &str) -> String {
    format!("{DIR}/{name}")
}

/// Crée le secret s'il n'existe pas (64 caractères, fichier 0600 dans un dossier 0700).
/// Rend true s'il vient d'être créé.
pub fn ensure(name: &str) -> Result<bool, String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(DIR)
        .map_err(|e| format!("cannot create {DIR}: {e}"))?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot set the permissions of {DIR}: {e}"))?;
    let p = path(name);
    if fs::metadata(&p).is_ok_and(|m| m.len() > 0) {
        return Ok(false);
    }
    let value = random(64)?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)
        .map_err(|e| format!("cannot write {p}: {e}"))?;
    std::io::Write::write_all(&mut f, value.as_bytes())
        .map_err(|e| format!("cannot write {p}: {e}"))?;
    Ok(true)
}

/// Garde une valeur donnée (par exemple un mot de passe au moment où il est créé), fichier 0600.
pub fn store(name: &str, value: &str) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(DIR)
        .map_err(|e| format!("cannot create {DIR}: {e}"))?;
    let p = path(name);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)
        .map_err(|e| format!("cannot write {p}: {e}"))?;
    std::io::Write::write_all(&mut f, value.as_bytes())
        .map_err(|e| format!("cannot write {p}: {e}"))
}

/// Lit un secret, None s'il n'existe pas.
pub fn read(name: &str) -> Option<String> {
    fs::read_to_string(path(name))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_strings() {
        let a = random(64).unwrap();
        let b = random(64).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| ALPHABET.contains(&c)));
        assert_eq!(random(0).unwrap(), "");
        assert_eq!(random(200).unwrap().len(), 200);
    }
}
