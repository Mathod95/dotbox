//! Lire le catalogue sur GitHub (Journal, 7.6, D41, D64).
//!
//! dotbox demande à l'API de GitHub le dernier commit de main qui touche catalog/, lit l'arbre de
//! ce commit, télécharge chaque fichier de catalog/ à ce commit et vérifie son empreinte git
//! (celle que donne l'arbre). Le tout est rangé d'un seul coup dans /opt/dotbox/catalog/, avec
//! catalog.yml (le commit, sa date, l'heure de la lecture). Ce qui est rangé est exactement ce
//! que montre GitHub à ce commit. Deux appels à l'API par lecture, sans compte.

use std::path::Path;

use crate::out::say;
use crate::system;

pub const DIR: &str = "/opt/dotbox/catalog";
const API: &str = "https://api.github.com/repos/Mathod95/dotbox";
const RAW: &str = "https://raw.githubusercontent.com/Mathod95/dotbox";

/// Le commit du catalogue.
#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub commit: String,
    /// AAAA-MM-JJ
    pub date: String,
}

impl Source {
    pub fn short(&self) -> &str {
        &self.commit[..self.commit.len().min(7)]
    }
}

pub fn get(url: &str) -> Result<String, String> {
    system::run(
        "wget",
        &[
            "-q",
            "-O",
            "-",
            "--header",
            "Accept: application/vnd.github+json",
            url,
        ],
    )
    .map_err(|e| format!("cannot reach GitHub ({url}): {e}"))
}

/// Le dernier commit qui touche catalog/ (réponse de commits?path=catalog&per_page=1).
pub fn parse_commits(json: &str) -> Result<Source, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let c = v
        .as_array()
        .and_then(|a| a.first())
        .ok_or("there is no catalog/ on GitHub yet")?;
    let commit = c["sha"].as_str().unwrap_or_default().to_string();
    let date = c["commit"]["committer"]["date"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(10)
        .collect::<String>();
    if commit.len() != 40 || !commit.chars().all(|ch| ch.is_ascii_hexdigit()) || date.len() != 10 {
        return Err("unexpected answer from GitHub: no commit".into());
    }
    Ok(Source { commit, date })
}

/// Les fichiers de catalog/ dans l'arbre d'un commit: (chemin sous catalog/, empreinte git).
pub fn parse_tree(json: &str) -> Result<Vec<(String, String)>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    if v["truncated"].as_bool() == Some(true) {
        return Err("the catalog tree from GitHub is truncated".into());
    }
    let mut files = Vec::new();
    for e in v["tree"]
        .as_array()
        .ok_or("unexpected answer from GitHub: no tree")?
    {
        let (Some(path), Some(kind), Some(sha)) =
            (e["path"].as_str(), e["type"].as_str(), e["sha"].as_str())
        else {
            continue;
        };
        let Some(rel) = path.strip_prefix("catalog/") else {
            continue;
        };
        if kind != "blob" {
            continue;
        }
        let safe = rel.split('/').all(|p| {
            !p.is_empty()
                && p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        });
        if !safe {
            return Err(format!("unexpected file in the catalog: {path}"));
        }
        files.push((rel.to_string(), sha.to_string()));
    }
    if files.is_empty() {
        return Err("the catalog on GitHub is empty".into());
    }
    Ok(files)
}

/// L'empreinte git d'un fichier: sha1 de « blob <taille>\0 » suivi du contenu.
pub fn git_blob_sha(content: &[u8], tmp: &str) -> Result<String, String> {
    let mut data = format!("blob {}\0", content.len()).into_bytes();
    data.extend_from_slice(content);
    let f = format!("{tmp}/.blob");
    std::fs::write(&f, &data).map_err(|e| format!("cannot write {f}: {e}"))?;
    let out = system::run("sha1sum", &[&f])?;
    let _ = std::fs::remove_file(&f);
    Ok(out
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string())
}

pub fn source_yml(s: &Source, fetched: &str) -> String {
    format!(
        "# Écrit par dotbox: le catalogue lu sur GitHub (Journal, 7.6).\nsource: github.com/Mathod95/dotbox\ncommit: {}\ndate: {}\nfetched: {fetched}\n",
        s.commit, s.date
    )
}

/// Le catalogue déjà rangé sur le serveur, s'il y en a un.
pub fn current() -> Option<Source> {
    let text = std::fs::read_to_string(format!("{DIR}/catalog.yml")).ok()?;
    let field = |k: &str| {
        text.lines().find_map(|l| {
            l.strip_prefix(&format!("{k}: "))
                .map(|v| v.trim().to_string())
        })
    };
    Some(Source {
        commit: field("commit")?,
        date: field("date")?,
    })
}

/// Les droits du catalogue: 755 pour les dossiers, 644 pour les fichiers, modifiables par
/// root seul.
fn set_modes(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let set = |p: &Path, mode: u32| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("cannot set the mode of {}: {e}", p.display()))
    };
    set(dir, 0o755)?;
    for e in std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let p = e
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .path();
        if p.is_dir() {
            set_modes(&p)?;
        } else {
            set(&p, 0o644)?;
        }
    }
    Ok(())
}

/// Lit le catalogue sur GitHub et le range dans /opt/dotbox/catalog/.
pub fn fetch() -> Result<Source, String> {
    let source = parse_commits(&get(&format!("{API}/commits?path=catalog&per_page=1"))?)?;
    let files = parse_tree(&get(&format!(
        "{API}/git/trees/{}?recursive=1",
        source.commit
    ))?)?;
    say(
        "download",
        &format!(
            "catalog/ at {} ({}), {} files",
            source.short(),
            source.date,
            files.len()
        ),
    );
    let new = format!("{DIR}.new");
    let old = format!("{DIR}.old");
    let _ = std::fs::remove_dir_all(&new);
    std::fs::create_dir_all(&new).map_err(|e| format!("cannot create {new}: {e}"))?;
    let result = (|| {
        for (rel, sha) in &files {
            let dest = format!("{new}/{rel}");
            if let Some(parent) = Path::new(&dest).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            system::run(
                "wget",
                &[
                    "-q",
                    "-O",
                    &dest,
                    &format!("{RAW}/{}/catalog/{rel}", source.commit),
                ],
            )
            .map_err(|e| format!("cannot download catalog/{rel}: {e}"))?;
            let content = std::fs::read(&dest).map_err(|e| format!("cannot read {dest}: {e}"))?;
            if git_blob_sha(&content, &new)? != *sha {
                return Err(format!(
                    "catalog/{rel} does not match GitHub's checksum: catalog not updated"
                ));
            }
        }
        let now = system::run("date", &["+%Y-%m-%d %H:%M:%S %Z"]).unwrap_or_default();
        std::fs::write(format!("{new}/catalog.yml"), source_yml(&source, &now))
            .map_err(|e| format!("cannot write {new}/catalog.yml: {e}"))?;
        // des droits fixes, quel que soit l'umask de celui qui lance dotbox (J140)
        set_modes(Path::new(&new))
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&new);
        return Err(e);
    }
    // le nouveau catalogue remplace l'ancien d'un seul coup
    let _ = std::fs::remove_dir_all(&old);
    if Path::new(DIR).exists() {
        std::fs::rename(DIR, &old).map_err(|e| format!("cannot move {DIR}: {e}"))?;
    }
    std::fs::rename(&new, DIR).map_err(|e| format!("cannot move {new} to {DIR}: {e}"))?;
    let _ = std::fs::remove_dir_all(&old);
    say(
        "verify",
        &format!("{} files, git checksums ok, in {DIR}", files.len()),
    );
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_commit_of_the_catalog() {
        let json = r#"[{"sha":"0123456789abcdef0123456789abcdef01234567","commit":{"committer":{"date":"2026-11-17T08:30:00Z"}}}]"#;
        let s = parse_commits(json).unwrap();
        assert_eq!(s.date, "2026-11-17");
        assert_eq!(s.short(), "0123456");
        assert!(parse_commits("[]").unwrap_err().contains("no catalog"));
        assert!(parse_commits("{}").is_err());
    }

    #[test]
    fn only_the_files_of_the_catalog() {
        let json = r#"{"truncated":false,"tree":[
            {"path":"catalog","type":"tree","sha":"t"},
            {"path":"catalog/radarr.yml","type":"blob","sha":"aaa"},
            {"path":"catalog/core","type":"tree","sha":"t2"},
            {"path":"catalog/core/traefik.yml","type":"blob","sha":"bbb"},
            {"path":"src/main.rs","type":"blob","sha":"ccc"}]}"#;
        assert_eq!(
            parse_tree(json).unwrap(),
            [
                ("radarr.yml".to_string(), "aaa".to_string()),
                ("core/traefik.yml".to_string(), "bbb".to_string())
            ]
        );
        assert!(parse_tree(r#"{"truncated":true,"tree":[]}"#).is_err());
        assert!(
            parse_tree(
                r#"{"truncated":false,"tree":[{"path":"catalog/../x","type":"blob","sha":"a"}]}"#
            )
            .is_err()
        );
        assert!(parse_tree(r#"{"truncated":false,"tree":[]}"#).is_err());
    }

    #[test]
    fn git_checksum_of_a_file() {
        // git hash-object d'un fichier qui contient « hello » et un retour à la ligne
        let tmp = std::env::temp_dir().join(format!("dotbox-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(
            git_blob_sha(b"hello\n", tmp.to_str().unwrap()).unwrap(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_catalog_is_writable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("dotbox-modes-{}", std::process::id()));
        let core = tmp.join("core");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(tmp.join("radarr.yml"), "x").unwrap();
        std::fs::write(core.join("traefik.yml"), "x").unwrap();
        for p in [&tmp, &core] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o777)).unwrap();
        }
        set_modes(&tmp).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&tmp), 0o755);
        assert_eq!(mode(&core), 0o755);
        assert_eq!(mode(&tmp.join("radarr.yml")), 0o644);
        assert_eq!(mode(&core.join("traefik.yml")), 0o644);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_source_file_reads_back() {
        let s = Source {
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            date: "2026-11-17".into(),
        };
        let text = source_yml(&s, "2026-11-17 10:00:00 CET");
        assert!(text.contains("commit: 0123456789abcdef0123456789abcdef01234567\n"));
        assert!(text.contains("date: 2026-11-17\n"));
    }
}
