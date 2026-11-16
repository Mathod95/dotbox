//! Authelia, installé par `dotbox init` (Journal, phase A): le portail de connexion sur
//! auth.<domaine> et le middleware authelia@docker de Traefik (forward auth), comme dans la doc
//! d'intégration d'Authelia. Un seul compte, au nom de l'utilisateur Linux, dans le groupe admins
//! (D22), mot de passe et code à 6 chiffres (D23), codes de vérification dans un fichier (D21).
//! Relancé, il ne refait que ce qui manque: secrets et compte ne sont jamais recréés.

use crate::out::say;
use crate::{secrets, system};
use std::time::{Duration, Instant};

/// Dernière version stable, fixée (D15).
pub const IMAGE: &str = "authelia/authelia:4.39.28";

const COMPOSE: &str = "/opt/authelia/compose.yml";
const CONFIG: &str = "/opt/authelia/config/configuration.yml";
const USERS: &str = "/opt/authelia/config/users_database.yml";

/// Les secrets d'Authelia: nom du fichier dans /opt/dotbox/secrets, chemin dans le container,
/// variable qui donne ce chemin à Authelia (méthode des secrets de sa doc).
const SECRETS: &[(&str, &str, &str)] = &[
    (
        "authelia_session_secret",
        "/secrets/session_secret",
        "AUTHELIA_SESSION_SECRET_FILE",
    ),
    (
        "authelia_storage_encryption_key",
        "/secrets/storage_encryption_key",
        "AUTHELIA_STORAGE_ENCRYPTION_KEY_FILE",
    ),
    (
        "authelia_jwt_secret",
        "/secrets/jwt_secret",
        "AUTHELIA_IDENTITY_VALIDATION_RESET_PASSWORD_JWT_SECRET_FILE",
    ),
];

pub fn compose(domain: &str) -> String {
    let mut env = String::new();
    let mut vols = String::from("      - /opt/authelia/config:/config\n");
    for (name, target, var) in SECRETS {
        env.push_str(&format!("      {var}: {target}\n"));
        vols.push_str(&format!("      - {}:{target}:ro\n", secrets::path(name)));
    }
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Vos changements vont dans compose.override.yml, à côté.
name: authelia
services:
  authelia:
    image: {IMAGE}
    container_name: authelia
    restart: unless-stopped
    networks:
      - dotbox
    environment:
{env}    volumes:
{vols}    security_opt:
      - no-new-privileges:true
    labels:
      traefik.enable: "true"
      traefik.http.routers.authelia.rule: Host(`auth.{domain}`)
      traefik.http.routers.authelia.entrypoints: websecure
      traefik.http.services.authelia.loadbalancer.server.port: "9091"
      # Le middleware qu'une app ajoute à son routeur pour être protégée (authelia@docker)
      traefik.http.middlewares.authelia.forwardauth.address: http://authelia:9091/api/authz/forward-auth
      traefik.http.middlewares.authelia.forwardauth.trustForwardHeader: "true"
      traefik.http.middlewares.authelia.forwardauth.maxResponseBodySize: "8192"
      traefik.http.middlewares.authelia.forwardauth.authResponseHeaders: Remote-User,Remote-Groups,Remote-Email,Remote-Name

networks:
  dotbox:
    external: true
"#
    )
}

pub fn configuration(domain: &str) -> String {
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Les secrets sont lus dans /opt/dotbox/secrets (variables *_FILE du compose.yml).
server:
  address: 'tcp://:9091'

log:
  level: 'info'

theme: 'auto'

# Double authentification: code à 6 chiffres ou passkey, au choix de chacun (Journal, D23 et D27).
# Duo (service payant) désactivé.
totp:
  issuer: '{domain}'
webauthn:
  display_name: 'Dot Box'
duo_api:
  disable: true

authentication_backend:
  file:
    path: '/config/users_database.yml'

# Tout est refusé, sauf ce qu'une règle autorise
access_control:
  default_policy: 'deny'
  rules:
    - domain: 'traefik.{domain}'
      subject: 'group:admins'
      policy: 'two_factor'

session:
  cookies:
    - domain: '{domain}'
      authelia_url: 'https://auth.{domain}'

# Essais de mot de passe en rafale: 3 ratés en 2 minutes, bloqué 5 minutes
regulation:
  max_retries: 3
  find_time: '2 minutes'
  ban_time: '5 minutes'

storage:
  local:
    path: '/config/db.sqlite3'

# Codes de vérification écrits dans un fichier, pas d'e-mail (Journal, D21)
notifier:
  filesystem:
    filename: '/config/notification.txt'
"#
    )
}

const USERS_HEADER: &str =
    "# Créé par dotbox init. Les comptes se gèrent ensuite ici (dotbox user, plus tard).\n";

/// La base d'utilisateurs: un seul compte, dans le groupe admins. Seul le hash du mot de passe.
/// L'adresse est factice: le notifier écrit dans un fichier, rien n'est envoyé (Journal, J63).
pub fn users_database(user: &str, digest: &str, email: &str) -> String {
    format!(
        r#"{USERS_HEADER}users:
  {user}:
    disabled: false
    displayname: '{user}'
    password: '{digest}'
    email: '{email}'
    groups:
      - 'admins'
"#
    )
}

/// Ajoute l'adresse au compte s'il n'en a pas (comptes créés avant J63), sans toucher au reste.
/// Rend le nouveau contenu, ou None s'il n'y a rien à changer.
pub fn add_missing_email(text: &str, user: &str, email: &str) -> Result<Option<String>, String> {
    use serde_yaml_ng::Value;
    let mut root: Value =
        serde_yaml_ng::from_str(text).map_err(|e| format!("{USERS} is not valid YAML: {e}"))?;
    let Some(account) = root
        .get_mut("users")
        .and_then(|u| u.get_mut(user))
        .and_then(Value::as_mapping_mut)
    else {
        return Err(format!("{USERS} has no account {user}"));
    };
    if account
        .get("email")
        .and_then(Value::as_str)
        .is_some_and(|e| !e.is_empty())
    {
        return Ok(None);
    }
    account.insert("email".into(), email.into());
    let body = serde_yaml_ng::to_string(&root).map_err(|e| e.to_string())?;
    Ok(Some(format!("{USERS_HEADER}{body}")))
}

/// Nom du secret qui garde le mot de passe d'un compte (Journal, D25).
pub fn password_secret(user: &str) -> String {
    format!("authelia_password_{user}")
}

/// Lit la sortie de `authelia crypto hash generate argon2 --random`: le mot de passe et son hash.
/// La ligne « Digest: sha256:... » d'un téléchargement d'image est ignorée.
pub fn parse_hash_output(text: &str) -> Option<(String, String)> {
    let password = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Random Password: "))?;
    let digest = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("Digest: "))
        .find(|d| d.starts_with("$argon2id$"))?;
    Some((password.to_string(), digest.to_string()))
}

/// Les utilisateurs déjà présents dans users_database.yml.
fn existing_users(text: &str) -> Vec<String> {
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text) {
        Ok(v) => v
            .get("users")
            .and_then(|u| u.as_mapping())
            .map(|m| {
                m.keys()
                    .filter_map(|k| k.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Installe ou vérifie Authelia. Rend le mot de passe du compte s'il vient d'être créé
/// (à afficher une seule fois), None sinon.
pub fn setup(domain: &str, user: &str) -> Result<Option<String>, String> {
    let email = format!("{user}@{domain}");
    for (name, _, _) in SECRETS {
        if secrets::ensure(name)? {
            say("create", &format!("secret {}", secrets::path(name)));
        }
    }

    // le compte: créé une seule fois, jamais réécrit (seule l'adresse peut être ajoutée)
    let mut password = None;
    let mut users_changed = false;
    match std::fs::read_to_string(USERS) {
        Ok(text) => {
            if existing_users(&text).iter().any(|u| u == user) {
                say("check", &format!("Authelia account {user}"));
                if let Some(new) = add_missing_email(&text, user, &email)? {
                    system::write_if_changed(USERS, &new, 0o600)?;
                    say(
                        "write",
                        &format!("{USERS} (address {email}, never used to send)"),
                    );
                    users_changed = true;
                }
            } else {
                return Err(format!(
                    "{USERS} exists without the account {user}: add it there, or move the file away and run dotbox init again"
                ));
            }
        }
        Err(_) => {
            // l'image d'abord, pour que la sortie du hash ne contienne que lui
            system::run("docker", &["pull", "-q", IMAGE])?;
            let out = system::run(
                "docker",
                &[
                    "run",
                    "--rm",
                    IMAGE,
                    "authelia",
                    "crypto",
                    "hash",
                    "generate",
                    "argon2",
                    "--random",
                    "--random.length",
                    "24",
                ],
            )?;
            let (pw, digest) = parse_hash_output(&out)
                .ok_or("unexpected output from authelia crypto hash generate")?;
            system::write_if_changed(USERS, &users_database(user, &digest, &email), 0o600)?;
            secrets::store(&password_secret(user), &pw)?;
            say("create", &format!("Authelia account {user} (group admins)"));
            password = Some(pw);
        }
    }

    let compose_changed = system::write_if_changed(COMPOSE, &compose(domain), 0o644)?;
    if compose_changed {
        say("write", COMPOSE);
    }
    let config_changed = system::write_if_changed(CONFIG, &configuration(domain), 0o644)?;
    if config_changed {
        say("write", CONFIG);
    }

    let running = system::run(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", "authelia"],
    )
    .is_ok_and(|s| s == "true");
    if compose_changed || !running {
        system::run(
            "docker",
            &["compose", "-f", COMPOSE, "up", "-d", "--quiet-pull"],
        )?;
        say("start", "authelia");
    } else if config_changed || users_changed || password.is_some() {
        system::run("docker", &["restart", "authelia"])?;
        say("restart", "authelia (new configuration)");
    }
    wait_healthy(Duration::from_secs(90))?;
    say("check", &format!("authelia healthy ({IMAGE})"));
    Ok(password)
}

fn wait_healthy(timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let status = system::run(
            "docker",
            &["inspect", "-f", "{{.State.Health.Status}}", "authelia"],
        )
        .unwrap_or_default();
        if status == "healthy" {
            return Ok(());
        }
        if start.elapsed() > timeout {
            let logs =
                system::run("docker", &["logs", "--tail", "20", "authelia"]).unwrap_or_default();
            return Err(format!(
                "authelia is not healthy after {}s ({status}), last logs:\n{logs}",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_the_expected_ones() {
        assert_eq!(
            compose("example.com"),
            include_str!("../tests/expected/authelia/compose.yml")
        );
        assert_eq!(
            configuration("example.com"),
            include_str!("../tests/expected/authelia/configuration.yml")
        );
    }

    #[test]
    fn generated_yaml_is_valid() {
        for text in [
            compose("example.com"),
            configuration("example.com"),
            users_database(
                "admin",
                "$argon2id$v=19$m=65536,t=3,p=4$abc$def",
                "admin@example.com",
            ),
        ] {
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text).unwrap();
        }
    }

    #[test]
    fn users_file_holds_only_the_hash() {
        let text = users_database(
            "admin",
            "$argon2id$v=19$m=65536,t=3,p=4$abc$def",
            "admin@example.com",
        );
        assert!(text.contains("password: '$argon2id$"));
        assert!(text.contains("email: 'admin@example.com'"));
        assert!(text.contains("- 'admins'"));
        assert_eq!(existing_users(&text), ["admin"]);
    }

    #[test]
    fn email_added_without_touching_the_hash() {
        let old = "users:\n  admin:\n    disabled: false\n    displayname: 'admin'\n    password: '$argon2id$v=19$m=65536,t=3,p=4$abc$def'\n    groups:\n      - 'admins'\n";
        let new = add_missing_email(old, "admin", "admin@example.com")
            .unwrap()
            .unwrap();
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&new).unwrap();
        let a = &v["users"]["admin"];
        assert_eq!(a["email"].as_str(), Some("admin@example.com"));
        assert_eq!(
            a["password"].as_str(),
            Some("$argon2id$v=19$m=65536,t=3,p=4$abc$def")
        );
        assert_eq!(a["groups"][0].as_str(), Some("admins"));
        // déjà là: rien à changer
        assert!(
            add_missing_email(&new, "admin", "x@example.com")
                .unwrap()
                .is_none()
        );
        assert!(add_missing_email(old, "nobody", "x@example.com").is_err());
    }

    #[test]
    fn hash_output() {
        let out = "Digest: sha256:bd97cff4\nStatus: Downloaded\nRandom Password: Abc123xyz\nDigest: $argon2id$v=19$m=65536,t=3,p=4$salt$hash\n";
        let (pw, digest) = parse_hash_output(out).unwrap();
        assert_eq!(pw, "Abc123xyz");
        assert_eq!(digest, "$argon2id$v=19$m=65536,t=3,p=4$salt$hash");
        assert!(parse_hash_output("Random Password: x\n").is_none());
        assert!(parse_hash_output("Digest: $argon2id$x\n").is_none());
    }

    #[test]
    fn secrets_are_files_never_values() {
        let c = compose("example.com");
        for (name, target, var) in SECRETS {
            assert!(c.contains(&format!("{var}: {target}")));
            assert!(c.contains(&format!("/opt/dotbox/secrets/{name}:{target}:ro")));
        }
    }
}
