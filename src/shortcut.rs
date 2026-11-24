//! Le raccourci db (D143): un lien /usr/local/bin/db vers dotbox, posé par dotbox init seulement
//! si aucune commande db n'existe déjà, et la complétion du shell pour les deux noms. Un lien
//! symbolique, pas un lien dur: dotbox upgrade remplace le fichier dotbox, un lien dur garderait
//! l'ancien binaire.

use std::io::ErrorKind;
use std::path::Path;

use crate::out::say;

/// Le raccourci.
pub const LINK: &str = "/usr/local/bin/db";
const NAME: &str = "db";
/// Là où le shell trouverait une autre commande db (le PATH de root, comme system::command).
const PATH_DIRS: &[&str] = &[
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
];

/// Ce que l'on trouve à la place du raccourci.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    /// le lien vers dotbox, déjà là
    Ours,
    /// rien: le raccourci peut être créé
    Free,
    /// une autre commande db (un autre outil, un script de l'utilisateur): on n'y touche pas
    Taken(String),
}

fn look(link: &Path, target: &Path, dirs: &[&str]) -> Found {
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            if std::fs::read_link(link).is_ok_and(|t| t == target) {
                return Found::Ours;
            }
            return Found::Taken(link.display().to_string());
        }
        Ok(_) => return Found::Taken(link.display().to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(_) => return Found::Taken(link.display().to_string()),
    }
    for dir in dirs {
        let other = Path::new(dir).join(NAME);
        if other != link && other.exists() {
            return Found::Taken(other.display().to_string());
        }
    }
    Found::Free
}

/// Pose le raccourci s'il manque et que db est libre. Rend le résumé de la carte (« db shortcut »,
/// ou pourquoi il n'y est pas). Ne fait jamais échouer init: le raccourci n'est qu'un confort.
pub fn setup() -> String {
    let link = Path::new(LINK);
    let target = Path::new(crate::upgrade::BIN);
    match look(link, target, PATH_DIRS) {
        Found::Ours => {
            say("check", &format!("{LINK} → {}", target.display()));
            "db shortcut".into()
        }
        Found::Free => match std::os::unix::fs::symlink(target, link) {
            Ok(()) => {
                say("create", &format!("{LINK} → {}", target.display()));
                "db shortcut".into()
            }
            Err(e) => {
                say("skip", &format!("cannot create {LINK}: {e}"));
                "no db shortcut".into()
            }
        },
        Found::Taken(other) => {
            say(
                "skip",
                &format!("{other} already exists: no db shortcut, use dotbox"),
            );
            "db taken, no shortcut".into()
        }
    }
}

/// Le script de complétion de dotbox, déclaré aussi pour db.
pub fn completion(shell: clap_complete::Shell) -> String {
    use clap::CommandFactory;
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut crate::Cli::command(), "dotbox", &mut buf);
    for_both(shell, &String::from_utf8_lossy(&buf))
}

/// Ajoute db là où le script déclare dotbox. bash: la fonction _dotbox sert aux deux noms (elle
/// reconnaît la commande par son premier mot). zsh: #compdef et compdef. fish: db reprend les
/// complétions de dotbox (--wraps).
fn for_both(shell: clap_complete::Shell, script: &str) -> String {
    use clap_complete::Shell;
    match shell {
        Shell::Bash => {
            script
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("complete -F _dotbox") && l.ends_with(" dotbox") {
                        format!("{l} {NAME}")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
        Shell::Zsh => script
            .replacen("#compdef dotbox", &format!("#compdef dotbox {NAME}"), 1)
            .replace(
                "compdef _dotbox dotbox",
                &format!("compdef _dotbox dotbox {NAME}"),
            ),
        Shell::Fish => format!("{script}complete -c {NAME} --wraps dotbox\n"),
        _ => script.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_complete::Shell;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("dotbox-shortcut-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn free_ours_or_taken() {
        let d = tmp("look");
        let bin = d.join("dotbox");
        std::fs::write(&bin, "").unwrap();
        let link = d.join("db");
        let other_dir = d.join("other");
        std::fs::create_dir_all(&other_dir).unwrap();
        let dirs = [other_dir.to_str().unwrap()];
        // rien: libre
        assert_eq!(look(&link, &bin, &dirs), Found::Free);
        // le lien vers dotbox: le nôtre
        std::os::unix::fs::symlink(&bin, &link).unwrap();
        assert_eq!(look(&link, &bin, &dirs), Found::Ours);
        // un lien vers autre chose: pris
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(d.join("ailleurs"), &link).unwrap();
        assert!(matches!(look(&link, &bin, &dirs), Found::Taken(_)));
        // un vrai fichier: pris, jamais écrasé
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "#!/bin/sh\n").unwrap();
        assert!(matches!(look(&link, &bin, &dirs), Found::Taken(_)));
        // une autre commande db ailleurs dans le PATH: prise aussi
        std::fs::remove_file(&link).unwrap();
        std::fs::write(other_dir.join("db"), "").unwrap();
        assert_eq!(
            look(&link, &bin, &dirs),
            Found::Taken(other_dir.join("db").display().to_string())
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn completion_for_both_names() {
        let bash = completion(Shell::Bash);
        assert!(bash.contains("-o default dotbox db\n"));
        assert!(!bash.contains("dotbox db db"));
        let zsh = completion(Shell::Zsh);
        assert!(zsh.starts_with("#compdef dotbox db\n"));
        assert!(zsh.contains("compdef _dotbox dotbox db"));
        let fish = completion(Shell::Fish);
        assert!(fish.ends_with("complete -c db --wraps dotbox\n"));
    }
}
