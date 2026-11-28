//! `dotbox serve` (Journal, 11.2 et 11.3): le service de Dot Box, sous systemd, en root. La CLI
//! lui parle par un socket Unix, /run/dotbox/dotbox.sock, réservé à root et au groupe dotbox
//! (D79). Il vérifie qui appelle par le noyau (SO_PEERCRED), sans mot de passe ni jeton.
//!
//! Les routes (page API): /api/v1/version, /status, /doctor, /apps, /diff, /sync, /config et
//! /jobs. Une installation
//! ou un retrait est une tâche: un processus `dotbox install … --local` lancé avec
//! DOTBOX_EVENTS=1, dont les événements (ui.rs) sont gardés et passés en direct à la CLI, en
//! Server-Sent Events. Une tâche à la fois, comme apt.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use axum::extract::connect_info::{ConnectInfo, Connected};
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::serve::IncomingStream;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::net::UnixListener;
use tokio::sync::watch;

use crate::{config_cmd, doctor, manage, status, sync, system};

pub const SOCKET: &str = "/run/dotbox/dotbox.sock";
pub const GROUP: &str = "dotbox";
/// La porte web (11.11, D106): le service écoute aussi sur la passerelle du réseau dotbox, pour
/// Traefik seulement (jamais sur une adresse publique).
pub const WEB_PORT: u16 = 8800;
/// L'en-tête que Traefik ajoute, et le secret qu'il porte (/opt/dotbox/secrets/web_gate).
pub const GATE_HEADER_NAME: &str = "X-Dotbox-Gate";
pub const GATE_SECRET: &str = "web_gate";
/// Le groupe d'Authelia qui a droit à l'interface web (Q69, 4).
pub const WEB_GROUP: &str = "admins";
/// Les tâches finies gardées en mémoire, pour qui les relit.
const KEEP_JOBS: usize = 20;

/// Qui appelle: lu par le noyau sur le socket.
#[derive(Clone, Debug)]
struct Peer {
    uid: u32,
    gid: u32,
}

impl Connected<IncomingStream<'_, UnixListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, UnixListener>) -> Self {
        match stream.io().peer_cred() {
            Ok(c) => Peer {
                uid: c.uid(),
                gid: c.gid(),
            },
            // inconnu: refusé plus bas
            Err(_) => Peer {
                uid: u32::MAX,
                gid: u32::MAX,
            },
        }
    }
}

/// Le gid du groupe dotbox et ses membres, dans le texte de /etc/group.
pub fn group_entry(etc_group: &str, group: &str) -> Option<(u32, Vec<String>)> {
    etc_group.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() < 4 || f[0] != group {
            return None;
        }
        let gid = f[2].parse().ok()?;
        let members = f[3]
            .split(',')
            .filter(|m| !m.is_empty())
            .map(String::from)
            .collect();
        Some((gid, members))
    })
}

/// Le nom d'un uid, dans le texte de /etc/passwd.
pub fn user_name(etc_passwd: &str, uid: u32) -> Option<String> {
    etc_passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2].parse() == Ok(uid)).then(|| f[0].to_string())
    })
}

/// Root, ou un membre du groupe dotbox (en groupe principal ou secondaire). /etc/group est relu
/// à chaque appel: un utilisateur retiré du groupe perd l'accès tout de suite.
pub fn allowed(uid: u32, gid: u32, etc_group: &str, etc_passwd: &str) -> bool {
    if uid == 0 {
        return true;
    }
    let Some((group_gid, members)) = group_entry(etc_group, GROUP) else {
        return false;
    };
    gid == group_gid || user_name(etc_passwd, uid).is_some_and(|n| members.contains(&n))
}

async fn check_peer(ConnectInfo(peer): ConnectInfo<Peer>, req: Request, next: Next) -> Response {
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    if allowed(peer.uid, peer.gid, &group, &passwd) {
        next.run(req).await
    } else {
        problem(
            StatusCode::FORBIDDEN,
            "only root and the dotbox group may use Dot Box: sudo usermod -aG dotbox $USER, then log in again",
        )
    }
}

/// L'adresse du serveur dans le réseau dotbox (sa passerelle), là où Traefik le joint.
pub fn gateway() -> Result<String, String> {
    let gw = system::run(
        "docker",
        &[
            "network",
            "inspect",
            "dotbox",
            "-f",
            "{{range .IPAM.Config}}{{.Gateway}}{{end}}",
        ],
    )?;
    let gw = gw.trim().to_string();
    gw.parse::<std::net::Ipv4Addr>()
        .map_err(|_| format!("the dotbox network has no IPv4 gateway ({gw:?})"))?;
    Ok(gw)
}

/// Deux textes égaux, comparés sans s'arrêter au premier écart (le temps ne dit rien du secret).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Une requête web est-elle admise (Q69, 3 à 5)? Le secret de Traefik, le groupe admins posé par
/// Authelia, et pour ce qui change quelque chose, l'origine dotbox.<domaine>.
pub fn web_allowed(
    secret: &str,
    given: Option<&str>,
    groups: Option<&str>,
    changes: bool,
    origin: Option<&str>,
    domain: &str,
) -> Result<(), &'static str> {
    if secret.is_empty() || !given.is_some_and(|g| same(g.as_bytes(), secret.as_bytes())) {
        return Err("only through Traefik, at https://dotbox.<domain>");
    }
    if !groups.is_some_and(|g| g.split(',').any(|x| x.trim() == WEB_GROUP)) {
        return Err("only for the admins group of Authelia");
    }
    if changes && origin != Some(format!("https://dotbox.{domain}").as_str()) {
        return Err("a change must come from the Dot Box page itself");
    }
    Ok(())
}

async fn check_web(req: Request, next: Next) -> Response {
    fn header(req: &Request, name: &str) -> Option<String> {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    }
    let (given, groups, origin) = (
        header(&req, GATE_HEADER_NAME),
        header(&req, "Remote-Groups"),
        header(&req, "Origin"),
    );
    let secret = crate::secrets::read(GATE_SECRET).unwrap_or_default();
    let domain = crate::config::Config::load()
        .ok()
        .and_then(|c| c.get("domain"))
        .unwrap_or_default();
    let changes = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    let verdict = web_allowed(
        secret.trim(),
        given.as_deref(),
        groups.as_deref(),
        changes,
        origin.as_deref(),
        &domain,
    );
    match verdict {
        Ok(()) => next.run(req).await,
        Err(why) => problem(StatusCode::FORBIDDEN, why),
    }
}

/// Le front (11.19, D102): trois fichiers rangés dans le binaire, servis par la porte web.
const INDEX: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");

/// Rien d'autre que dotbox.<domaine>: ni script, ni police, ni style venus d'ailleurs, et pas
/// d'iframe (11.19).
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

pub fn html_escape(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// La page, avec l'utilisateur vu par Authelia et la version.
pub fn index_page(user: &str) -> String {
    INDEX
        .replace("{user}", &html_escape(user))
        .replace("{version}", env!("CARGO_PKG_VERSION"))
}

fn web_file(body: String, kind: &str, cache: &str) -> Response {
    Response::builder()
        .header("content-type", kind)
        .header("cache-control", cache)
        .header("content-security-policy", CSP)
        .header("x-content-type-options", "nosniff")
        .header("referrer-policy", "no-referrer")
        .header("x-frame-options", "DENY")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|e| problem(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

async fn web_index(headers: axum::http::HeaderMap) -> Response {
    let user = headers
        .get("Remote-User")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("?");
    // la page porte le nom de l'utilisateur: jamais gardée en cache
    web_file(index_page(user), "text/html; charset=utf-8", "no-store")
}

async fn web_css() -> Response {
    web_file(APP_CSS.into(), "text/css; charset=utf-8", "no-cache")
}

async fn web_js() -> Response {
    web_file(APP_JS.into(), "text/javascript; charset=utf-8", "no-cache")
}

/// L'écoute web: sur la passerelle du réseau dotbox, réessayée tant que Docker n'est pas prêt.
async fn web_listen(app: Router) {
    loop {
        let gw = tokio::task::spawn_blocking(gateway)
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let bound = match gw {
            Ok(gw) => tokio::net::TcpListener::bind((gw.as_str(), WEB_PORT))
                .await
                .map(|l| (gw, l))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        match bound {
            Ok((gw, listener)) => {
                eprintln!("dotbox web gate listening on {gw}:{WEB_PORT}");
                if let Err(e) = axum::serve(listener, app.clone()).await {
                    eprintln!("the web gate stopped: {e}");
                }
            }
            Err(e) => eprintln!("the web gate is not listening yet: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

fn problem(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

struct Job {
    what: String,
    events: Vec<String>,
    code: Option<i32>,
    tick: watch::Sender<usize>,
}

#[derive(Default)]
struct Jobs {
    next: u64,
    running: Option<u64>,
    all: HashMap<u64, Arc<Mutex<Job>>>,
}

type Shared = Arc<Mutex<Jobs>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn run() -> Result<(), String> {
    if system::effective_uid()? != 0 {
        return Err("dotbox serve runs as root, under systemd (dotbox.service)".into());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the service: {e}"))?;
    rt.block_on(serve())
}

async fn serve() -> Result<(), String> {
    let dir = std::path::Path::new(SOCKET)
        .parent()
        .unwrap_or(std::path::Path::new("/run"));
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let _ = std::fs::remove_file(SOCKET);
    let listener =
        UnixListener::bind(SOCKET).map_err(|e| format!("cannot listen on {SOCKET}: {e}"))?;
    // 0660 root:dotbox: le noyau ferme déjà la porte aux autres, le service vérifie encore
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let gid = group_entry(&group, GROUP).map(|(g, _)| g);
    std::os::unix::fs::chown(SOCKET, Some(0), gid)
        .map_err(|e| format!("cannot give {SOCKET} to the dotbox group: {e}"))?;
    std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("cannot set the rights of {SOCKET}: {e}"))?;

    let jobs: Shared = Arc::default();
    let api: Router<Shared> = Router::new()
        .route("/api/v1/version", get(version))
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/doctor", get(get_doctor).post(post_doctor))
        .route("/api/v1/motd", get(get_motd))
        .route("/api/v1/motd/refresh", post(post_motd_refresh))
        .route("/api/v1/apps", post(install))
        .route("/api/v1/apps/{app}", get(info).delete(remove))
        .route("/api/v1/apps/{app}/logs", get(logs))
        .route("/api/v1/apps/{app}/{action}", post(lifecycle))
        .route("/api/v1/catalog", get(catalog_list))
        .route("/api/v1/update", post(update))
        .route("/api/v1/diff", get(get_diff))
        .route("/api/v1/sync", post(post_sync))
        .route("/api/v1/backup", post(post_backup))
        .route("/api/v1/config", get(config_all))
        .route("/api/v1/config/{key}", get(config_one).put(config_set))
        .route("/api/v1/jobs/{id}/events", get(events));
    // la porte web: les mêmes routes, plus le front, derrière check_web (11.11, 11.19)
    let web = api
        .clone()
        .route("/", get(web_index))
        .route("/app.css", get(web_css))
        .route("/app.js", get(web_js))
        .layer(middleware::from_fn(check_web))
        .with_state(jobs.clone());
    tokio::spawn(web_listen(web));
    tokio::spawn(count_bans());
    let app = api.layer(middleware::from_fn(check_peer)).with_state(jobs);
    eprintln!("dotbox {} listening on {SOCKET}", env!("CARGO_PKG_VERSION"));
    axum::serve(listener, app.into_make_service_with_connect_info::<Peer>())
        .await
        .map_err(|e| format!("the service stopped: {e}"))
}

/// La version, et l'empreinte du binaire qui tourne: dotbox init redémarre le service quand le
/// binaire de /usr/local/bin a changé (11.7).
async fn version() -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "binary": running_binary(),
    }))
}

pub fn running_binary() -> String {
    static SUM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    // /proc/<pid>/exe et non /proc/self/exe: sha256sum lirait son propre binaire
    SUM.get_or_init(|| {
        crate::crowdsec::sha256(&format!("/proc/{}/exe", std::process::id())).unwrap_or_default()
    })
    .clone()
}

async fn blocking<T: serde::Serialize + Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => problem(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(e) => problem(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn get_status() -> Response {
    blocking(status::collect).await
}

async fn get_doctor() -> Response {
    match doctor::last() {
        Some(r) => Json(r).into_response(),
        None => problem(StatusCode::NOT_FOUND, "doctor has never run: dotbox doctor"),
    }
}

async fn post_doctor() -> Response {
    blocking(doctor::run).await
}

async fn get_motd() -> Response {
    blocking(crate::motd::collect).await
}

/// Rafraîchit ce qui est en cache (doctor, le catalogue, CrowdSec), puis rend le message. Un
/// morceau qui échoue n'empêche pas le reste: il garde son ancienne valeur et son âge.
async fn post_motd_refresh() -> Response {
    blocking(|| {
        let _ = crate::motd::refresh();
        crate::motd::collect()
    })
    .await
}

/// Le compte des bannissements de CrowdSec pour le MOTD (D89): toutes les dix minutes.
async fn count_bans() {
    loop {
        let _ = tokio::task::spawn_blocking(crate::motd::refresh_bans).await;
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    }
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    search: String,
}

async fn catalog_list(Query(q): Query<Search>) -> Response {
    blocking(move || manage::list(&q.search)).await
}

async fn info(Path(app): Path<String>) -> Response {
    if !crate::catalog::valid_name(&app) {
        return problem(StatusCode::BAD_REQUEST, "not an app name");
    }
    blocking(move || manage::info(&app)).await
}

async fn get_diff() -> Response {
    blocking(sync::plan).await
}

async fn config_all() -> Response {
    blocking(|| config_cmd::get(None)).await
}

/// Une clé mal formée ou refusée: 400, avec le motif.
async fn config_answer<T: serde::Serialize + Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => problem(StatusCode::BAD_REQUEST, &e),
        Err(e) => problem(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn config_one(Path(key): Path<String>) -> Response {
    config_answer(move || config_cmd::get(Some(&key))).await
}

#[derive(Deserialize)]
struct ConfigBody {
    value: String,
}

/// dotbox config set: écrit dotbox.yml, n'applique rien (D93). Pas une tâche: un fichier.
async fn config_set(Path(key): Path<String>, Json(body): Json<ConfigBody>) -> Response {
    config_answer(move || config_cmd::set(&key, &body.value)).await
}

#[derive(Deserialize)]
struct SyncBody {
    /// le plan que la CLI a montré, et que l'utilisateur a accepté (D93)
    plan: Vec<sync::Gap>,
}

/// dotbox sync: le plan est recalculé ici, et doit être celui qui a été montré. Sinon, rien
/// n'est fait (comme un plan Terraform périmé): on n'applique jamais autre chose.
async fn post_sync(State(jobs): State<Shared>, Json(body): Json<SyncBody>) -> Response {
    let now = match tokio::task::spawn_blocking(sync::plan).await {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return problem(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(e) => return problem(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if now != body.plan {
        return problem(
            StatusCode::CONFLICT,
            "the plan changed since it was shown: run dotbox sync again to see the new one",
        );
    }
    if sync::targets(&now).is_empty() {
        return problem(StatusCode::CONFLICT, "nothing to do");
    }
    // la CLI a déjà demandé la confirmation
    start_job(
        &jobs,
        "dotbox sync".into(),
        vec!["sync".into(), "--yes".into()],
    )
}

async fn lifecycle(
    State(jobs): State<Shared>,
    Path((app, action)): Path<(String, String)>,
) -> Response {
    if !matches!(action.as_str(), "start" | "stop" | "restart") {
        return problem(StatusCode::NOT_FOUND, "start, stop or restart");
    }
    if !crate::catalog::valid_name(&app) {
        return problem(StatusCode::BAD_REQUEST, "not an app name");
    }
    start_job(&jobs, format!("dotbox {action} {app}"), vec![action, app])
}

#[derive(Deserialize)]
struct UpdateBody {
    #[serde(default)]
    apps: Vec<String>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    rollback: bool,
}

async fn update(State(jobs): State<Shared>, Json(body): Json<UpdateBody>) -> Response {
    if !body.apps.iter().all(|a| crate::catalog::valid_name(a)) {
        return problem(StatusCode::BAD_REQUEST, "not an app name");
    }
    let mut args = vec!["update".to_string()];
    args.extend(body.apps.iter().cloned());
    if body.all {
        args.push("--all".into());
    }
    if body.rollback {
        args.push("--rollback".into());
    }
    let what = format!("dotbox {}", args.join(" "));
    start_job(&jobs, what, args)
}

#[derive(Deserialize, Default)]
struct BackupBody {
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    stream: bool,
}

/// dotbox backup, en tâche (D97): une à la fois, comme le reste.
async fn post_backup(State(jobs): State<Shared>, body: Option<Json<BackupBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let mut args = vec!["backup".to_string()];
    if let Some(app) = body.app {
        if app != "dotbox" && !crate::catalog::valid_name(&app) {
            return problem(StatusCode::BAD_REQUEST, "not an app name");
        }
        args.push("--app".into());
        args.push(app);
    }
    if let Some(to) = body.to {
        if let Err(e) = crate::backup::Dest::parse(&to) {
            return problem(StatusCode::BAD_REQUEST, &e);
        }
        args.push("--to".into());
        args.push(to);
    }
    if body.stream {
        args.push("--stream".into());
    }
    let what = format!("dotbox {}", args.join(" "));
    start_job(&jobs, what, args)
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: u32,
    #[serde(default)]
    follow: bool,
}

fn default_tail() -> u32 {
    100
}

/// Les logs d'une app, en flux: stdout et stderr du container mêlés, comme à l'écran. Quand la
/// CLI s'en va (Ctrl+C), le flux tombe et docker logs est arrêté avec lui.
async fn logs(Path(app): Path<String>, Query(q): Query<LogsQuery>) -> Response {
    let check = app.clone();
    match tokio::task::spawn_blocking(move || manage::logs_allowed(&check)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return problem(StatusCode::BAD_REQUEST, &e),
        Err(e) => return problem(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
    let child = tokio::process::Command::new("docker")
        .args(manage::logs_args(&app, q.tail, q.follow))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("docker logs: {e}"),
            );
        }
    };
    let out = child.stdout.take();
    let err = child.stderr.take();
    fn chunks<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
        r: Option<R>,
        keep: Option<tokio::process::Child>,
    ) -> impl futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send {
        futures_util::stream::unfold((r, keep), |(r, keep)| async move {
            use tokio::io::AsyncReadExt;
            let mut r = r?;
            let mut buf = vec![0u8; 8192];
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok(axum::body::Bytes::from(buf)), (Some(r), keep)))
                }
            }
        })
    }
    let stream = futures_util::stream::select(chunks(out, Some(child)), chunks(err, None));
    Response::builder()
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|e| problem(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

#[derive(Deserialize)]
struct InstallBody {
    apps: Vec<String>,
    /// le niveau d'accès à la place de celui du manifest (D178)
    #[serde(default)]
    access: Option<String>,
}

async fn install(State(jobs): State<Shared>, Json(body): Json<InstallBody>) -> Response {
    if body.apps.is_empty() || !body.apps.iter().all(|a| crate::catalog::valid_name(a)) {
        return problem(StatusCode::BAD_REQUEST, "give one or more app names");
    }
    let mut args = vec!["install".to_string()];
    args.extend(body.apps.iter().cloned());
    let mut what = format!("dotbox install {}", body.apps.join(" "));
    if let Some(a) = &body.access {
        if crate::catalog::Access::parse(a).is_none() {
            return problem(
                StatusCode::BAD_REQUEST,
                "access: two_factor, one_factor, app or public",
            );
        }
        args.push("--access".into());
        args.push(a.clone());
        what.push_str(&format!(" --access {a}"));
    }
    start_job(&jobs, what, args)
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    purge: bool,
}

async fn remove(
    State(jobs): State<Shared>,
    Path(app): Path<String>,
    Query(q): Query<RemoveQuery>,
) -> Response {
    if !crate::catalog::valid_name(&app) {
        return problem(StatusCode::BAD_REQUEST, "not an app name");
    }
    let mut args = vec!["remove".to_string(), app.clone()];
    // la CLI a déjà demandé la confirmation (D42)
    if q.purge {
        args.push("--purge".into());
        args.push("--yes".into());
    }
    let what = format!(
        "dotbox remove {app}{}",
        if q.purge { " --purge" } else { "" }
    );
    start_job(&jobs, what, args)
}

fn start_job(jobs: &Shared, what: String, mut args: Vec<String>) -> Response {
    let (id, job) = {
        let mut j = lock(jobs);
        if let Some(r) = j.running {
            let busy = j
                .all
                .get(&r)
                .map(|x| lock(x).what.clone())
                .unwrap_or_default();
            return problem(
                StatusCode::CONFLICT,
                &format!("another job is running ({busy}): wait for it to end"),
            );
        }
        j.next += 1;
        let id = j.next;
        let (tick, _) = watch::channel(0);
        let job = Arc::new(Mutex::new(Job {
            what: what.clone(),
            events: Vec::new(),
            code: None,
            tick,
        }));
        j.all.insert(id, job.clone());
        j.running = Some(id);
        // les plus anciennes tâches finies sortent de la mémoire
        while j.all.len() > KEEP_JOBS {
            let Some(old) = j
                .all
                .keys()
                .copied()
                .filter(|k| Some(*k) != j.running)
                .min()
            else {
                break;
            };
            j.all.remove(&old);
        }
        (id, job)
    };
    args.push("--local".into());
    let child = tokio::process::Command::new("/proc/self/exe")
        .args(&args)
        .env("DOTBOX_EVENTS", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(false)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            lock(jobs).running = None;
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("cannot start {what}: {e}"),
            );
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let shared = jobs.clone();
    tokio::spawn(async move {
        let push = |line: String| {
            let j = lock(&job);
            let mut j = j;
            j.events.push(line);
            let n = j.events.len();
            let _ = j.tick.send(n);
        };
        // les erreurs écrites avant l'affichage (une app inconnue...) arrivent sur stderr
        let err_task = stderr.map(|e| {
            let job = job.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(e).lines();
                let mut text = Vec::new();
                while let Ok(Some(l)) = lines.next_line().await {
                    text.push(l);
                }
                drop(job);
                text
            })
        });
        if let Some(out) = stdout {
            let mut lines = tokio::io::BufReader::new(out).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                push(l);
            }
        }
        let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(1);
        if let Some(t) = err_task {
            for l in t
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|l| !l.trim().is_empty())
            {
                let msg = l.trim().trim_start_matches("error").trim().to_string();
                push(json!({"t": "error", "text": msg}).to_string());
            }
        }
        push(json!({"t": "end", "code": code}).to_string());
        {
            let mut j = lock(&job);
            j.code = Some(code);
            let n = j.events.len();
            let _ = j.tick.send(n);
        }
        lock(&shared).running = None;
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "job": id, "what": what })),
    )
        .into_response()
}

/// Les événements d'une tâche, depuis le début, puis en direct jusqu'à sa fin.
async fn events(State(jobs): State<Shared>, Path(id): Path<u64>) -> Response {
    let Some(job) = lock(&jobs).all.get(&id).cloned() else {
        return problem(StatusCode::NOT_FOUND, "no such job");
    };
    let rx = lock(&job).tick.subscribe();
    let stream = futures_util::stream::unfold(
        (job, rx, 0usize, false),
        |(job, mut rx, sent, ended)| async move {
            if ended {
                return None;
            }
            loop {
                let (next, finished) = {
                    let j = lock(&job);
                    (
                        j.events.get(sent).cloned(),
                        j.code.is_some() && sent >= j.events.len(),
                    )
                };
                if let Some(e) = next {
                    let last = e.contains("\"t\":\"end\"");
                    let ev: Result<Event, std::convert::Infallible> = Ok(Event::default().data(e));
                    return Some((ev, (job, rx, sent + 1, last)));
                }
                if finished || rx.changed().await.is_err() {
                    return None;
                }
            }
        },
    );
    Sse::new(stream).into_response()
}

pub const UNIT_PATH: &str = "/etc/systemd/system/dotbox.service";
const BIN: &str = "/usr/local/bin/dotbox";

/// L'unité systemd du service (11.7).
pub fn unit() -> String {
    format!(
        "# Écrit par dotbox init: le service de Dot Box, que la CLI appelle par {SOCKET} (D79).
[Unit]
Description=Dot Box service (the API behind the dotbox command)
After=docker.service network-online.target
Wants=docker.service network-online.target

[Service]
ExecStart={BIN} serve
Restart=on-failure
RestartSec=2
RuntimeDirectory=dotbox
RuntimeDirectoryMode=0755

[Install]
WantedBy=multi-user.target
"
    )
}

/// Installe le service, le (re)démarre si l'unité ou le binaire ont changé, puis vérifie qu'il
/// répond sur son socket. Rend le résumé de l'étape.
pub fn setup() -> Result<String, String> {
    use crate::out::say;
    let changed = system::write_if_changed(UNIT_PATH, &unit(), 0o644)?;
    if changed {
        say("write", UNIT_PATH);
        system::run("systemctl", &["daemon-reload"])?;
    }
    system::run("systemctl", &["enable", "dotbox"])?;
    let active = system::run("systemctl", &["is-active", "dotbox"]).as_deref() == Ok("active");
    let installed = crate::crowdsec::sha256(BIN).unwrap_or_default();
    let running = crate::client::get("/api/v1/version")
        .ok()
        .and_then(|v| v.get("binary").and_then(Value::as_str).map(String::from))
        .unwrap_or_default();
    if !active {
        system::run("systemctl", &["start", "dotbox"])?;
        say("start", "dotbox.service");
    } else if changed || running != installed {
        system::run("systemctl", &["restart", "dotbox"])?;
        say("restart", "dotbox.service (new binary or unit)");
    } else {
        say("check", "dotbox.service running");
    }
    // il répond, avec ce binaire
    let start = std::time::Instant::now();
    loop {
        if let Ok(v) = crate::client::get("/api/v1/version") {
            let sum = v.get("binary").and_then(Value::as_str).unwrap_or_default();
            if sum == installed {
                say("check", &format!("{SOCKET} answers"));
                return Ok(format!("service running, {SOCKET}"));
            }
        }
        if start.elapsed() > std::time::Duration::from_secs(15) {
            let logs = system::run("journalctl", &["-u", "dotbox", "-n", "20", "--no-pager"])
                .unwrap_or_default();
            return Err(format!(
                "the dotbox service does not answer on {SOCKET}:\n{logs}"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La porte web (11.11, Q69): le secret, le groupe admins, l'origine pour un changement.
    #[test]
    fn web_gate() {
        let ok = |given, groups, changes, origin| {
            web_allowed("s3cret", given, groups, changes, origin, "example.com")
        };
        assert!(ok(Some("s3cret"), Some("admins"), false, None).is_ok());
        assert!(ok(Some("s3cret"), Some("family, admins"), false, None).is_ok());
        assert!(ok(None, Some("admins"), false, None).is_err());
        assert!(ok(Some("s3cre"), Some("admins"), false, None).is_err());
        assert!(ok(Some("s3cretX"), Some("admins"), false, None).is_err());
        assert!(ok(Some("s3cret"), Some("family"), false, None).is_err());
        assert!(ok(Some("s3cret"), None, false, None).is_err());
        assert!(ok(Some("s3cret"), Some("admins"), true, None).is_err());
        assert!(
            ok(
                Some("s3cret"),
                Some("admins"),
                true,
                Some("https://evil.example.org")
            )
            .is_err()
        );
        assert!(
            ok(
                Some("s3cret"),
                Some("admins"),
                true,
                Some("https://dotbox.example.com")
            )
            .is_ok()
        );
        // sans secret sur le serveur, rien ne passe
        assert!(web_allowed("", Some(""), Some("admins"), false, None, "example.com").is_err());
        assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"abc", b"ab"));
    }

    /// Le front (11.19): l'utilisateur échappé, la version, les fichiers que la page appelle.
    #[test]
    fn the_front() {
        let page = index_page("<b>\"x\"</b>");
        assert!(page.contains("data-user=\"&lt;b&gt;&quot;x&quot;&lt;/b&gt;\""));
        assert!(!page.contains("{user}") && !page.contains("{version}"));
        assert!(page.contains(env!("CARGO_PKG_VERSION")));
        assert!(page.contains("app.css?v=") && page.contains("app.js?v="));
        // rien de chargé ailleurs, rien en ligne (la CSP le refuserait)
        for f in [INDEX, APP_CSS, APP_JS] {
            assert!(!f.contains("http://"), "no plain http");
        }
        assert!(!INDEX.contains("<style") && !INDEX.contains("style=\""));
        assert!(!INDEX.contains("<script>"));
        assert!(CSP.contains("frame-ancestors 'none'") && CSP.contains("script-src 'self'"));
        // chaque route que le front appelle existe dans l'API
        for route in [
            "\"/status\"",
            "\"/doctor\"",
            "\"/diff\"",
            "\"/catalog\"",
            "\"/config\"",
            "\"/update\"",
            "\"/backup\"",
            "\"/sync\"",
            "\"/apps\"",
        ] {
            assert!(APP_JS.contains(route), "{route}");
        }
    }

    const GROUP_FILE: &str = "root:x:0:\nsudo:x:27:admin\ndotbox:x:990:admin,lea\nadmin:x:1000:\n";
    const PASSWD: &str = "root:x:0:0::/root:/bin/bash\nadmin:x:1000:1000::/home/admin:/bin/bash\nbob:x:1001:1001::/home/bob:/bin/bash\n";

    #[test]
    fn the_unit() {
        let u = unit();
        assert!(u.contains("ExecStart=/usr/local/bin/dotbox serve"));
        assert!(u.contains("RuntimeDirectory=dotbox"));
        assert!(u.contains("After=docker.service"));
    }

    #[test]
    fn who_may_call() {
        assert!(allowed(0, 0, GROUP_FILE, PASSWD));
        assert!(allowed(1000, 1000, GROUP_FILE, PASSWD));
        assert!(!allowed(1001, 1001, GROUP_FILE, PASSWD));
        // dotbox en groupe principal
        assert!(allowed(1001, 990, GROUP_FILE, PASSWD));
        // pas de groupe dotbox: seul root
        assert!(!allowed(1000, 1000, "sudo:x:27:admin\n", PASSWD));
    }

    #[test]
    fn group_and_user_lookup() {
        assert_eq!(
            group_entry(GROUP_FILE, "dotbox"),
            Some((990, vec!["admin".to_string(), "lea".to_string()]))
        );
        assert_eq!(group_entry(GROUP_FILE, "docker"), None);
        assert_eq!(user_name(PASSWD, 1001).as_deref(), Some("bob"));
        assert_eq!(user_name(PASSWD, 42), None);
    }
}
