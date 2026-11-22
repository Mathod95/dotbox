//! La CLI cliente (Journal, 11.5): `dotbox status`, `doctor`, `install` et `remove` demandent le
//! travail au service par son socket (D79), puis affichent la réponse. Une installation suit ses
//! étapes en direct (Server-Sent Events) avec le même affichage qu'en local (ui::replay).

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::UnixStream;

use crate::serve::SOCKET;
use crate::{apps, doctor, prompt, status, ui};

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// Le service ne répond pas: pourquoi, et quoi faire.
fn unreachable(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "the Dot Box service is not installed ({SOCKET} is missing): run sudo dotbox init, or add --local as root"
        ),
        std::io::ErrorKind::PermissionDenied => format!(
            "{SOCKET} is for root and the dotbox group: sudo usermod -aG dotbox $USER, then log in again"
        ),
        _ => format!(
            "the Dot Box service does not answer ({e}): sudo systemctl restart dotbox, or add --local as root"
        ),
    }
}

type Sender = hyper::client::conn::http1::SendRequest<Full<Bytes>>;

async fn connect() -> Result<Sender, String> {
    let stream = UnixStream::connect(SOCKET)
        .await
        .map_err(|e| unreachable(&e))?;
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("the Dot Box service: {e}"))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

fn request(
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Request<Full<Bytes>>, String> {
    let bytes = body.map(|b| b.to_string()).unwrap_or_default();
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", "dotbox")
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(bytes)))
        .map_err(|e| e.to_string())
}

/// Une requête, sa réponse en JSON. Une erreur du service (4xx, 5xx) rend son message.
async fn call(method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let mut sender = connect().await?;
    let res = sender
        .send_request(request(method, path, body)?)
        .await
        .map_err(|e| format!("the Dot Box service: {e}"))?;
    let code = res.status();
    let bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("the Dot Box service: {e}"))?
        .to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if code.is_success() {
        Ok(v)
    } else {
        Err(v
            .get("error")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("the Dot Box service answered {code}")))
    }
}

pub fn get(path: &str) -> Result<Value, String> {
    runtime()?.block_on(call(Method::GET, path, None))
}

pub fn status(json_out: bool) -> Result<(), String> {
    let v = get("/api/v1/status")?;
    if json_out {
        crate::clack::put(&serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    let st: status::Status = serde_json::from_value(v).map_err(|e| e.to_string())?;
    status::print(&st);
    Ok(())
}

pub fn doctor(json_out: bool) -> Result<(), String> {
    let v = crate::clack::waiting("checking the server", || {
        runtime()?.block_on(call(Method::POST, "/api/v1/doctor", None))
    })?;
    let r: doctor::Report = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    if json_out {
        crate::clack::put(&serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        doctor::print(&r);
    }
    if r.problems() > 0 {
        Err(String::new())
    } else {
        Ok(())
    }
}

/// dotbox motd sans sudo: le service rassemble les lignes, la commande les dessine.
pub fn motd(refresh: bool, style: Option<&str>, json_out: bool) -> Result<(), String> {
    if let Some(s) = style {
        crate::motd::check_style(s)?;
    }
    let v = if refresh {
        crate::clack::waiting("refreshing doctor, the catalog and CrowdSec", || {
            runtime()?.block_on(call(Method::POST, "/api/v1/motd/refresh", None))
        })?
    } else {
        get("/api/v1/motd")?
    };
    let m: crate::motd::Motd = serde_json::from_value(v).map_err(|e| e.to_string())?;
    crate::motd::show(&m, style, json_out)
}

/// Une tâche: la demander, puis suivre ses événements jusqu'à la fin.
async fn job(
    method: Method,
    path: &str,
    body: Option<Value>,
    verbose: bool,
) -> Result<Option<Value>, String> {
    let started = call(method, path, body).await?;
    let id = started
        .get("job")
        .and_then(Value::as_u64)
        .ok_or("the Dot Box service gave no job")?;
    let mut sender = connect().await?;
    let mut res = sender
        .send_request(request(
            Method::GET,
            &format!("/api/v1/jobs/{id}/events"),
            None,
        )?)
        .await
        .map_err(|e| format!("the Dot Box service: {e}"))?;
    if res.status() != StatusCode::OK {
        return Err(format!("the Dot Box service answered {}", res.status()));
    }
    let body = res.body_mut();
    let mut buf = String::new();
    let mut result = None;
    let mut failed = false;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| format!("the Dot Box service: {e}"))?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        buf.push_str(&String::from_utf8_lossy(data));
        while let Some(end) = buf.find("\n\n") {
            let block: String = buf.drain(..end + 2).collect();
            for data in block.lines().filter_map(|l| l.strip_prefix("data:")) {
                let data = data.trim_start();
                match serde_json::from_str::<Value>(data) {
                    Ok(e) => match ui::replay(&e, verbose) {
                        ui::Replayed::Shown => {}
                        ui::Replayed::Failed => failed = true,
                        ui::Replayed::Result(v) => result = Some(v),
                        ui::Replayed::End(code) => {
                            return if code == 0 && !failed {
                                Ok(result)
                            } else {
                                Err(String::new())
                            };
                        }
                    },
                    // une ligne qui n'est pas un événement: telle quelle
                    Err(_) => eprintln!("{data}"),
                }
            }
        }
    }
    Err("the Dot Box service closed the job before its end".into())
}

pub fn install(args: &apps::InstallArgs) -> Result<(), String> {
    if args.catalog.is_some() {
        return Err("--catalog works with --local only (as root)".into());
    }
    let body = json!({ "apps": args.apps });
    let result =
        runtime()?.block_on(job(Method::POST, "/api/v1/apps", Some(body), args.verbose))?;
    if let Some(v) = result {
        let ready: Vec<apps::Ready> = serde_json::from_value(v).unwrap_or_default();
        apps::print_ready(&ready);
    }
    Ok(())
}

pub fn remove(args: &apps::RemoveArgs) -> Result<(), String> {
    // la confirmation avant d'envoyer quoi que ce soit (D42)
    if args.purge && !args.yes {
        if !prompt::is_terminal() {
            return Err(
                "--purge asks for a confirmation: run it in a terminal, or add --yes".into(),
            );
        }
        let q = format!("Delete /opt/{} and everything in it?", args.app);
        if !prompt::confirm(&q, "←/→ to choose, Enter to confirm")? {
            eprintln!("  cancelled, nothing changed");
            return Err(String::new());
        }
    }
    let path = format!(
        "/api/v1/apps/{}{}",
        args.app,
        if args.purge { "?purge=true" } else { "" }
    );
    runtime()?.block_on(job(Method::DELETE, &path, None, args.verbose))?;
    Ok(())
}

/// Une valeur dans une adresse: lettres, chiffres et « -._~ » tels quels, le reste en %XX.
pub fn encode(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn print_json(v: &Value) {
    crate::clack::put(&serde_json::to_string_pretty(v).unwrap_or_default());
}

pub fn list(search: &str, json_out: bool) -> Result<(), String> {
    let v = get(&format!("/api/v1/catalog?search={}", encode(search)))?;
    if json_out {
        print_json(&v);
        return Ok(());
    }
    let l: Vec<crate::manage::CatalogApp> = serde_json::from_value(v).map_err(|e| e.to_string())?;
    crate::manage::print_list(&l);
    Ok(())
}

pub fn info(app: &str, json_out: bool) -> Result<(), String> {
    let v = get(&format!("/api/v1/apps/{}", encode(app)))?;
    if json_out {
        print_json(&v);
        return Ok(());
    }
    let i: crate::manage::AppInfo = serde_json::from_value(v).map_err(|e| e.to_string())?;
    crate::manage::print_info(&i);
    Ok(())
}

pub fn diff(json_out: bool) -> Result<(), String> {
    let v = get("/api/v1/diff")?;
    if json_out {
        print_json(&v);
    }
    let g: Vec<crate::sync::Gap> = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if !json_out {
        crate::sync::print_diff(&g);
    }
    if crate::sync::differences(&g) == 0 {
        Ok(())
    } else {
        Err(String::new())
    }
}

/// dotbox sync (D93): le plan, la confirmation, puis la tâche, avec le plan montré: le service
/// refuse s'il a changé entre-temps.
pub fn sync(yes: bool, verbose: bool) -> Result<(), String> {
    let v = get("/api/v1/diff")?;
    let plan: Vec<crate::sync::Gap> =
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    crate::sync::print_plan(&plan);
    if crate::sync::targets(&plan).is_empty() {
        return Ok(());
    }
    crate::sync::confirm(yes)?;
    runtime()?.block_on(job(
        Method::POST,
        "/api/v1/sync",
        Some(json!({ "plan": v })),
        verbose,
    ))?;
    Ok(())
}

pub fn config_get(key: Option<&str>, json_out: bool) -> Result<(), String> {
    let path = match key {
        Some(k) => format!("/api/v1/config/{}", encode(k)),
        None => "/api/v1/config".into(),
    };
    let v = get(&path)?;
    let list: Vec<crate::config_cmd::Entry> =
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    if json_out {
        print_json(&v);
    } else {
        crate::config_cmd::print_get(&list, key.is_some());
    }
    Ok(())
}

pub fn config_set(key: &str, value: &str, json_out: bool) -> Result<(), String> {
    let v = runtime()?.block_on(call(
        Method::PUT,
        &format!("/api/v1/config/{}", encode(key)),
        Some(json!({ "value": value })),
    ))?;
    if json_out {
        print_json(&v);
    } else {
        let c: crate::config_cmd::Change = serde_json::from_value(v).map_err(|e| e.to_string())?;
        crate::config_cmd::print_set(&c);
    }
    Ok(())
}

pub fn lifecycle(action: &str, app: &str, verbose: bool) -> Result<(), String> {
    let path = format!("/api/v1/apps/{}/{action}", encode(app));
    runtime()?.block_on(job(Method::POST, &path, None, verbose))?;
    Ok(())
}

pub fn update(
    apps: &[String],
    all: bool,
    rollback: bool,
    verbose: bool,
    json_out: bool,
) -> Result<(), String> {
    let body = json!({ "apps": apps, "all": all, "rollback": rollback });
    let result = runtime()?.block_on(job(Method::POST, "/api/v1/update", Some(body), verbose))?;
    if let Some(v) = result {
        if json_out {
            print_json(&v);
        } else {
            let r: Vec<crate::manage::Version> = serde_json::from_value(v).unwrap_or_default();
            crate::manage::print_versions(&r);
        }
    }
    Ok(())
}

pub fn backup(
    app: Option<&str>,
    to: Option<&str>,
    verbose: bool,
    json_out: bool,
) -> Result<(), String> {
    let body = json!({ "app": app, "to": to });
    let result = runtime()?.block_on(job(Method::POST, "/api/v1/backup", Some(body), verbose))?;
    if let Some(v) = result {
        if json_out {
            print_json(&v);
        } else {
            let r: crate::backup::Report = serde_json::from_value(v).unwrap_or_default();
            crate::backup::print_report(&r);
        }
    }
    Ok(())
}

/// Les logs en flux: chaque morceau reçu est écrit tout de suite, jusqu'à la fin ou Ctrl+C.
pub fn logs(app: &str, tail: u32, follow: bool) -> Result<(), String> {
    use std::io::Write;
    let path = format!(
        "/api/v1/apps/{}/logs?tail={tail}&follow={follow}",
        encode(app)
    );
    runtime()?.block_on(async move {
        let mut sender = connect().await?;
        let mut res = sender
            .send_request(request(Method::GET, &path, None)?)
            .await
            .map_err(|e| format!("the Dot Box service: {e}"))?;
        if !res.status().is_success() {
            let bytes = res
                .into_body()
                .collect()
                .await
                .map_err(|e| e.to_string())?
                .to_bytes();
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the Dot Box service refused")
                .to_string());
        }
        let body = res.body_mut();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| format!("the Dot Box service: {e}"))?;
            if let Some(data) = frame.data_ref() {
                let mut out = std::io::stdout().lock();
                if out.write_all(data).and_then(|_| out.flush()).is_err() {
                    return Ok(());
                }
            }
        }
        Ok(())
    })
}

/// `dotbox version` (13): la version de la commande, et celle du service qui tourne. Si le
/// service tourne un autre binaire que /usr/local/bin/dotbox (un upgrade sans redémarrage), le
/// dire, avec la commande pour le relancer.
pub fn version(json_out: bool) -> Result<(), String> {
    use crate::clack::Mark;
    let cli = env!("CARGO_PKG_VERSION");
    let installed = crate::crowdsec::sha256("/usr/local/bin/dotbox").unwrap_or_default();
    let service = get("/api/v1/version");
    let (sv, same) = match &service {
        Ok(v) => (
            v.get("version")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string(),
            v.get("binary").and_then(Value::as_str) == Some(installed.as_str()),
        ),
        Err(_) => (String::new(), false),
    };
    if json_out {
        print_json(&json!({
            "cli": cli,
            "service": if sv.is_empty() { Value::Null } else { json!(sv) },
            "same_binary": same,
        }));
        return Ok(());
    }
    crate::clack::put(&format!("  {:<9}{cli}", "cli"));
    match service {
        Ok(_) if same => crate::clack::put(&format!("  {:<9}{sv}, {SOCKET}", "service")),
        Ok(_) => {
            crate::clack::put(&format!(
                "  {:<9}{sv}, {}",
                "service",
                Mark::Warn.paint("runs another binary than /usr/local/bin/dotbox")
            ));
            crate::clack::put("           sudo systemctl restart dotbox");
        }
        Err(e) => crate::clack::put(&format!("  {:<9}{}", "service", Mark::Error.paint(&e))),
    }
    Ok(())
}
