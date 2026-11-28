//! Authelia, installé par `dotbox init` (Journal, étape 5): le portail de connexion sur
//! auth.<domaine> et le middleware authelia@docker de Traefik (forward auth), comme dans la doc
//! d'intégration d'Authelia. Un seul compte, au nom de l'utilisateur Linux, dans le groupe admins
//! (D22), mot de passe et code à 6 chiffres (D23), codes de vérification dans un fichier (D21).
//! Relancé, il ne refait que ce qui manque: secrets et compte ne sont jamais recréés.

use crate::config::Config;
use crate::out::say;
use crate::{docker, secrets, system};
use std::time::{Duration, Instant};

// La version d'Authelia, fixée (D15), est dans le catalogue: catalog/core/authelia.yml (D64).

pub const COMPOSE: &str = "/opt/authelia/compose.yml";
pub const CONFIG: &str = "/opt/authelia/config/configuration.yml";
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
    (
        REDIS_PASSWORD,
        "/secrets/redis_password",
        "AUTHELIA_SESSION_REDIS_PASSWORD_FILE",
    ),
];

/// Les sessions d'Authelia dans un Redis à lui (D178): son mot de passe, lu par Authelia, et le
/// fichier de réglages de Redis qui le contient (requirepass), lisible par l'utilisateur redis.
const REDIS_PASSWORD: &str = "authelia_redis_password";
const REDIS_CONF: &str = "authelia_redis_conf";
const REDIS_CONF_TARGET: &str = "/usr/local/etc/redis/redis.conf";

/// Le mot de passe de Redis et ses réglages, écrits une fois, réécrits seulement s'ils changent.
pub fn ensure_redis_secrets() -> Result<(), String> {
    if secrets::ensure(REDIS_PASSWORD)? {
        say(
            "create",
            &format!("secret {}", secrets::path(REDIS_PASSWORD)),
        );
    }
    let pw = secrets::read(REDIS_PASSWORD)
        .ok_or_else(|| format!("cannot read {}", secrets::path(REDIS_PASSWORD)))?;
    // appendonly: les sessions écrites sur le disque, gardées aussi au redémarrage du serveur
    let conf = format!("requirepass {pw}\nappendonly yes\n");
    if secrets::read(REDIS_CONF).as_deref() != Some(conf.trim_end()) {
        secrets::store(REDIS_CONF, &conf)?;
        say("write", &secrets::path(REDIS_CONF));
    }
    secrets::mountable(REDIS_CONF)
}

pub fn compose(domain: &str, image: &str, redis: &str) -> String {
    let mut env = String::new();
    let mut vols = String::from("      - /opt/authelia/config:/config\n");
    for (name, target, var) in SECRETS {
        env.push_str(&format!("      {var}: {target}\n"));
        vols.push_str(&format!("      - {}:{target}:ro\n", secrets::path(name)));
    }
    let conf = secrets::path(REDIS_CONF);
    format!(
        r#"# Généré par dotbox init: ne pas modifier, il est réécrit à chaque init.
# Vos changements vont dans compose.override.yml, à côté.
name: authelia
services:
  authelia:
    image: {image}
    container_name: authelia
    restart: unless-stopped
    networks:
      - dotbox
      - private
    depends_on:
      redis:
        condition: service_healthy
    environment:
{env}    volumes:
{vols}    security_opt:
      - no-new-privileges:true
    labels:
      traefik.docker.network: dotbox
      traefik.enable: "true"
      traefik.http.routers.authelia.rule: Host(`auth.{domain}`)
      traefik.http.routers.authelia.entrypoints: websecure
      traefik.http.services.authelia.loadbalancer.server.port: "9091"
      # sa page de connexion est ouverte à Internet: AppSec devant (10.7, D60)
      traefik.http.routers.authelia.middlewares: crowdsec@docker
      # Le middleware qu'une app ajoute à son routeur pour être protégée (authelia@docker)
      traefik.http.middlewares.authelia.forwardauth.address: http://authelia:9091/api/authz/forward-auth
      traefik.http.middlewares.authelia.forwardauth.trustForwardHeader: "true"
      traefik.http.middlewares.authelia.forwardauth.maxResponseBodySize: "8192"
      traefik.http.middlewares.authelia.forwardauth.authResponseHeaders: Remote-User,Remote-Groups,Remote-Email,Remote-Name
  # ses sessions (D178), sur un réseau qu'il ne partage qu'avec Authelia, sans accès à Internet
  redis:
    image: {redis}
    container_name: authelia-redis
    restart: unless-stopped
    networks:
      - private
    command: ["redis-server", "{REDIS_CONF_TARGET}"]
    volumes:
      - /opt/authelia/redis:/data
      - {conf}:{REDIS_CONF_TARGET}:ro
    security_opt:
      - no-new-privileges:true
    healthcheck:
      test: ["CMD-SHELL", "redis-cli --no-auth-warning -a \"$$(sed -n 's/^requirepass //p' {REDIS_CONF_TARGET})\" ping | grep -q PONG"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 10s

networks:
  dotbox:
    external: true
  private:
    internal: true
"#
    )
}

/// Les adresses des apps en one_factor (D178), d'après dotbox.yml: chacune a sa règle, avant
/// celle de toutes les apps (*.<domaine>, two_factor). Triées: le fichier ne change que si la
/// liste change.
pub fn one_factor_hosts(cfg: &Config, domain: &str) -> Vec<String> {
    let mut hosts: Vec<String> = cfg
        .app_names()
        .iter()
        .filter_map(|app| {
            let (sub, access) = cfg.app(app)?;
            (access == "one_factor").then(|| {
                let sub = if sub.is_empty() { app.clone() } else { sub };
                format!("{sub}.{domain}")
            })
        })
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Les règles des apps en one_factor, après install, remove ou un changement de dotbox.yml:
/// Authelia ne lit sa configuration qu'au démarrage, il redémarre seulement si elle change.
/// Ses sessions sont dans Redis: personne n'est déconnecté. Rend true s'il a redémarré.
pub fn refresh_rules(cfg: &Config, domain: &str) -> Result<bool, String> {
    if !std::path::Path::new(CONFIG).exists() {
        return Ok(false);
    }
    let text = configuration(domain, &one_factor_hosts(cfg, domain));
    if !system::write_if_changed(CONFIG, &text, 0o644)? {
        return Ok(false);
    }
    say("write", &format!("{CONFIG} (the one_factor rules)"));
    system::run("docker", &["restart", "authelia"])?;
    say("restart", "authelia (new rules, sessions kept in Redis)");
    wait_healthy(Duration::from_secs(90))?;
    say("check", "authelia healthy");
    Ok(true)
}

pub fn configuration(domain: &str, one_factor: &[String]) -> String {
    let mut rules = String::new();
    for host in one_factor {
        rules.push_str(&format!(
            "    - domain: '{host}'\n      subject: 'group:admins'\n      policy: 'one_factor'\n"
        ));
    }
    if !rules.is_empty() {
        rules.insert_str(
            0,
            "    # Les apps en one_factor (D178): mot de passe seulement, avant la règle de toutes\n",
        );
    }
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
{rules}    # Toute app derrière authelia@docker: groupe admins, deux facteurs (Journal, 7.3)
    - domain: '*.{domain}'
      subject: 'group:admins'
      policy: 'two_factor'

session:
  cookies:
    - domain: '{domain}'
      authelia_url: 'https://auth.{domain}'
  # les sessions dans Redis (D178): un redémarrage d'Authelia ne déconnecte personne
  redis:
    host: 'authelia-redis'
    port: 6379

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

/// Le hash argon2id d'un mot de passe, avec les réglages par défaut d'Authelia
/// (m=65536, t=3, p=4, 32 octets): calculé par dotbox, sans lancer de container.
pub fn hash_password(password: &str) -> Result<String, String> {
    use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
    use std::io::Read;
    let params = Params::new(65536, 3, 4, Some(32)).map_err(|e| e.to_string())?;
    // un sel de 16 octets tirés de /dev/urandom, comme les secrets
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_with_salt(password.as_bytes(), &bytes)
        .map(|h| h.to_string())
        .map_err(|e| format!("cannot hash the password: {e}"))
}

/// Longueur minimale du mot de passe Authelia (Journal, D34).
pub const MIN_PASSWORD: usize = 12;

/// Un mot de passe Authelia accepté: vide (tiré au hasard) ou 12 caractères au moins.
pub fn password_ok(pw: &str) -> Result<(), String> {
    match pw.chars().count() {
        0 => Ok(()),
        n if n < MIN_PASSWORD => Err(format!(
            "{n} characters, {MIN_PASSWORD} or more needed (Enter alone for a random one)"
        )),
        _ => Ok(()),
    }
}

/// Le compte existe-t-il déjà? (dans ce cas, init ne demande pas de mot de passe)
pub fn account_exists(user: &str) -> bool {
    std::fs::read_to_string(USERS).is_ok_and(|t| existing_users(&t).iter().any(|u| u == user))
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

/// Installe ou vérifie Authelia. `chosen`: le mot de passe tapé pendant init (vide ou absent: tiré
/// au hasard). Rend le mot de passe du compte s'il vient d'être créé (à afficher une seule fois),
/// None sinon.
/// La règle qui couvre les apps two_factor est-elle en place? (posée par dotbox init, 7.3)
pub fn apps_rule_present(domain: &str) -> bool {
    std::fs::read_to_string(CONFIG).is_ok_and(|t| t.contains(&format!("- domain: '*.{domain}'")))
}

pub fn setup(
    domain: &str,
    user: &str,
    chosen: Option<&str>,
    core: &crate::catalog::Core,
    one_factor: &[String],
) -> Result<Option<String>, String> {
    let image = core.image("authelia")?;
    let redis = core.image("redis")?;
    let email = format!("{user}@{domain}");
    for (name, _, _) in SECRETS {
        if secrets::ensure(name)? {
            say("create", &format!("secret {}", secrets::path(name)));
        }
    }
    ensure_redis_secrets()?;

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
            let pw = match chosen {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => secrets::random(24)?,
            };
            let digest = hash_password(&pw)?;
            system::write_if_changed(USERS, &users_database(user, &digest, &email), 0o600)?;
            secrets::store(&password_secret(user), &pw)?;
            say("create", &format!("Authelia account {user} (group admins)"));
            password = Some(pw);
        }
    }

    let compose_changed = system::write_if_changed(COMPOSE, &compose(domain, image, redis), 0o644)?;
    if compose_changed {
        say("write", COMPOSE);
    }
    let config_changed =
        system::write_if_changed(CONFIG, &configuration(domain, one_factor), 0o644)?;
    if config_changed {
        say("write", CONFIG);
    }

    let running = system::run(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", "authelia"],
    )
    .is_ok_and(|s| s == "true");
    if compose_changed || !running {
        docker::pull(image)?;
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
    say("check", &format!("authelia healthy ({image})"));
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
        let core = crate::catalog::load_core(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog"),
            "authelia",
        )
        .unwrap();
        let image = core.image("authelia").unwrap();
        let redis = core.image("redis").unwrap();
        assert_eq!(
            compose("example.com", image, redis),
            include_str!("../tests/expected/authelia/compose.yml")
                .replace("<authelia>", image)
                .replace("<redis>", redis)
        );
        assert_eq!(
            configuration("example.com", &[]),
            include_str!("../tests/expected/authelia/configuration.yml")
        );
    }

    #[test]
    fn generated_yaml_is_valid() {
        for text in [
            compose(
                "example.com",
                "authelia/authelia:4.39.28",
                "redis:8.12.0-alpine",
            ),
            configuration("example.com", &[]),
            configuration("example.com", &["films.example.com".into()]),
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
    fn password_hash_is_argon2id_for_authelia() {
        use argon2::{PasswordHash, PasswordVerifier};
        let h = hash_password("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=65536,t=3,p=4$"), "{h}");
        let parsed = PasswordHash::new(&h).unwrap();
        assert!(
            argon2::Argon2::default()
                .verify_password(b"correct horse battery", &parsed)
                .is_ok()
        );
        assert!(
            argon2::Argon2::default()
                .verify_password(b"wrong", &parsed)
                .is_err()
        );
        assert_ne!(h, hash_password("correct horse battery").unwrap(), "salt");
    }

    #[test]
    fn password_rules() {
        assert!(password_ok("").is_ok());
        assert!(password_ok("123456789012").is_ok());
        assert!(password_ok("short").is_err());
        assert!(password_ok("ééééééééééé").is_err());
    }

    #[test]
    fn one_factor_rules_before_the_rule_of_all_apps() {
        let text = configuration(
            "example.com",
            &["films.example.com".into(), "series.example.com".into()],
        );
        let rule = |host: &str| text.find(&format!("- domain: '{host}'")).unwrap();
        assert!(rule("traefik.example.com") < rule("films.example.com"));
        assert!(rule("films.example.com") < rule("series.example.com"));
        assert!(rule("series.example.com") < rule("*.example.com"));
        assert!(text.contains(
            "- domain: 'films.example.com'\n      subject: 'group:admins'\n      policy: 'one_factor'\n"
        ));
        // sans app en one_factor: le fichier d'avant, plus Redis
        assert!(!configuration("example.com", &[]).contains("one_factor"));
        // dotbox.yml: le sous-domaine choisi, sinon le nom de l'app
        let cfg = Config::parse(
            "domain: example.com\napps:\n  radarr:\n    subdomain: films\n    access: one_factor\n  sonarr:\n    subdomain: ''\n    access: one_factor\n  prowlarr:\n    subdomain: prowlarr\n    access: two_factor\n",
        )
        .unwrap();
        assert_eq!(
            one_factor_hosts(&cfg, "example.com"),
            ["films.example.com", "sonarr.example.com"]
        );
    }

    #[test]
    fn sessions_in_their_own_redis() {
        let c = compose(
            "example.com",
            "authelia/authelia:4.39.28",
            "redis:8.12.0-alpine",
        );
        // Traefik joint Authelia par dotbox, Redis n'est que sur le réseau privé
        assert!(c.contains("traefik.docker.network: dotbox"));
        assert!(c.contains("container_name: authelia-redis"));
        assert!(c.contains("  private:\n    internal: true\n"));
        assert!(c.contains("condition: service_healthy"));
        // le mot de passe n'est jamais dans le fichier: un secret monté
        assert!(c.contains(
            "/opt/dotbox/secrets/authelia_redis_conf:/usr/local/etc/redis/redis.conf:ro"
        ));
        assert!(configuration("example.com", &[]).contains("host: 'authelia-redis'"));
    }

    #[test]
    fn secrets_are_files_never_values() {
        let c = compose(
            "example.com",
            "authelia/authelia:4.39.28",
            "redis:8.12.0-alpine",
        );
        for (name, target, var) in SECRETS {
            assert!(c.contains(&format!("{var}: {target}")));
            assert!(c.contains(&format!("/opt/dotbox/secrets/{name}:{target}:ro")));
        }
    }
}
