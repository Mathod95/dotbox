//! L'affichage de `dotbox init` (mockup final de CLI Output, Journal D37 et D38).
//!
//! Une ligne par étape. L'étape en cours a son indicateur ⠼ et, en dessous, ce qu'elle fait et
//! sa barre de progression quand on la connaît. Une fois finie, il ne reste que sa ligne
//! `✓ [n/N] Nom  résumé  durée`. Les questions s'affichent sous la ligne de l'étape et
//! disparaissent avec le reste. Le détail va dans le log (log.rs), et à l'écran avec --verbose.
//!
//! Lancé par install.sh (DOTBOX_INSTALLER=1), install.sh a déjà affiché [1/7] Dot Box: les étapes
//! vont de [2/7] à [7/7]. Seul, de [1/6] à [6/6].

use console::{Term, style};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::log;

/// Largeur des lignes d'étape: celle d'un terminal SSH par défaut.
pub const WIDTH: usize = 80;
const NAME_W: usize = 13;
/// « ✓ [n/N] » et le nom: 2 + 1 + 1 + 5 + 1 + 13 + 1 = 24 colonnes avant le résumé.
const LEFT: usize = 24;
const TIME_W: usize = 6;
pub const SUMMARY_W: usize = WIDTH - LEFT - 1 - TIME_W;
const SPINNER: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ";
const STEPS: usize = 6;

struct Current {
    n: usize,
    name: String,
    /// ce que fait l'étape, repris sur la ligne ✗ si elle échoue
    doing: String,
    started: Instant,
    bar: ProgressBar,
    /// lignes écrites au-dessus de la barre pour les questions (ligne de l'étape comprise)
    frozen: usize,
    /// une ligne vide termine les questions: la question suivante de la même étape la reprend
    blank: bool,
    with_bar: bool,
}

struct State {
    first: usize,
    total: usize,
    count: usize,
    verbose: bool,
    tty: bool,
    start: Instant,
    cur: Option<Current>,
}

static UI: Mutex<Option<State>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<State>> {
    UI.lock().unwrap_or_else(|e| e.into_inner())
}

fn term() -> Term {
    Term::stderr()
}

fn print(line: &str) {
    let _ = term().write_line(line);
}

/// Premier numéro et nombre d'étapes (D38).
pub fn numbering(installer: bool) -> (usize, usize) {
    if installer {
        (2, STEPS + 1)
    } else {
        (1, STEPS)
    }
}

pub fn start(verbose: bool) {
    let installer = std::env::var("DOTBOX_INSTALLER").is_ok_and(|v| v == "1");
    let (first, total) = numbering(installer);
    *lock() = Some(State {
        first,
        total,
        count: 0,
        verbose,
        tty: term().is_term(),
        start: Instant::now(),
        cur: None,
    });
}

pub fn active() -> bool {
    lock().is_some()
}

/// Temps depuis le début de init.
pub fn elapsed() -> Duration {
    lock()
        .as_ref()
        .map(|s| s.start.elapsed())
        .unwrap_or_default()
}

/// « 0.4 s », « 48 s », « 1 min 13 s ».
pub fn duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 60.0 {
        format!("{} s", s.round() as u64)
    } else {
        let s = s.round() as u64;
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// Coupe un texte à `width` colonnes, avec « … ».
pub fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let mut t: String = text.chars().take(width.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn counter(n: usize, total: usize) -> String {
    style(format!("[{n}/{total}]")).dim().to_string()
}

/// La ligne d'une étape: marque, compteur, nom, résumé, durée (sur 80 colonnes).
pub fn step_line(
    mark: &str,
    n: usize,
    total: usize,
    name: &str,
    summary: &str,
    time: &str,
) -> String {
    let left = format!("  {mark} {} {name:<NAME_W$} ", counter(n, total));
    if time.is_empty() {
        format!("{left}{}", fit(summary, SUMMARY_W + 1 + TIME_W))
    } else {
        let summary = fit(summary, SUMMARY_W);
        format!(
            "{left}{summary:<SUMMARY_W$} {}",
            style(format!("{time:>TIME_W$}")).dim()
        )
    }
}

fn pad() -> String {
    " ".repeat(LEFT)
}

fn spinner_style(with_head: bool) -> ProgressStyle {
    let sub = format!("{}{{msg:.dim}}", pad());
    let template = if with_head {
        format!("  {{spinner:.yellow}} {{prefix}}\n{sub}")
    } else {
        sub
    };
    ProgressStyle::with_template(&template)
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_chars(SPINNER)
}

fn bar_style(with_head: bool) -> ProgressStyle {
    let sub = format!(
        "{p}{{msg:.dim}}\n{p}{{bar:48.green/black.bright}}  {{pos:>3}} %   {{elapsed:.dim}}",
        p = pad()
    );
    let template = if with_head {
        format!("  {{spinner:.yellow}} {{prefix}}\n{sub}")
    } else {
        sub
    };
    ProgressStyle::with_template(&template)
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .tick_chars(SPINNER)
        .progress_chars("━━")
}

fn new_bar(tty: bool, verbose: bool, prefix: String, with_head: bool) -> ProgressBar {
    if !tty || verbose {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
    bar.set_style(spinner_style(with_head));
    bar.set_prefix(prefix);
    bar.enable_steady_tick(Duration::from_millis(100));
    bar
}

fn head(n: usize, total: usize, name: &str, doing: &str) -> String {
    format!(
        "{} {name:<NAME_W$} {}",
        counter(n, total),
        fit(doing, SUMMARY_W + 1 + TIME_W)
    )
}

/// Commence une étape.
pub fn step(name: &str, doing: &str) {
    let mut g = lock();
    let Some(s) = g.as_mut() else { return };
    let n = s.first + s.count;
    s.count += 1;
    log::write(&format!("== [{n}/{}] {name}: {doing}", s.total));
    if s.verbose && s.tty {
        print(&step_line(
            &style("·").dim().to_string(),
            n,
            s.total,
            name,
            doing,
            "",
        ));
    }
    let bar = new_bar(s.tty, s.verbose, head(n, s.total, name, doing), true);
    s.cur = Some(Current {
        n,
        name: name.to_string(),
        doing: doing.to_string(),
        started: Instant::now(),
        bar,
        frozen: 0,
        blank: false,
        with_bar: false,
    });
}

/// Une sous-étape: dans le log, et sous la ligne de l'étape (ou en entier avec --verbose).
pub fn sub(verb: &str, detail: &str) {
    log::write(&format!("{verb:<9}{detail}"));
    let mut g = lock();
    let Some(s) = g.as_mut() else { return };
    let verbose = s.verbose;
    let Some(c) = s.cur.as_mut() else { return };
    if verbose {
        print(&format!(
            "{}{}",
            " ".repeat(10),
            style(format!("{verb:<9}{detail}")).dim()
        ));
        return;
    }
    if c.with_bar {
        c.bar.set_style(spinner_style(c.frozen == 0));
        c.with_bar = false;
    }
    c.bar
        .set_message(fit(&format!("{verb} {detail}"), WIDTH - LEFT));
}

/// La progression de la sous-étape en cours, de 0 à 100.
pub fn progress(pct: f64) {
    let mut g = lock();
    let Some(c) = g.as_mut().and_then(|s| s.cur.as_mut()) else {
        return;
    };
    if !c.with_bar {
        c.bar.set_style(bar_style(c.frozen == 0));
        c.bar.set_length(100);
        c.bar.reset_elapsed();
        c.with_bar = true;
    }
    c.bar.set_position(pct.clamp(0.0, 100.0) as u64);
}

/// Pose des questions sous la ligne de l'étape en cours. La ligne de l'étape reste en place
/// (sans tourner) pendant les questions, puis l'étape continue en dessous.
pub fn ask<T>(questions: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    {
        let mut g = lock();
        if let Some(c) = g.as_mut().and_then(|s| s.cur.as_mut()) {
            if c.frozen == 0 {
                c.bar.finish_and_clear();
                let line = format!("  {} {}", style("⠼").yellow(), c.bar.prefix());
                print(&line);
                c.frozen = 1;
            } else {
                c.bar.finish_and_clear();
                if c.blank {
                    let _ = term().clear_last_lines(1);
                    c.frozen -= 1;
                    c.blank = false;
                }
            }
        }
    }
    let result = questions();
    let mut g = lock();
    if let Some(s) = g.as_mut() {
        let (tty, verbose) = (s.tty, s.verbose);
        if let Some(c) = s.cur.as_mut() {
            print("");
            c.frozen += 1;
            c.blank = true;
            c.with_bar = false;
            c.bar = new_bar(tty, verbose, c.bar.prefix(), false);
        }
    }
    result
}

/// Une question vient d'être répondue: ses lignes restent à l'écran jusqu'à la fin de l'étape.
pub fn prompted(lines: usize) {
    let mut g = lock();
    if let Some(c) = g.as_mut().and_then(|s| s.cur.as_mut()) {
        c.frozen += lines;
    }
}

fn close(c: &Current) {
    c.bar.finish_and_clear();
    if c.frozen > 0 {
        let _ = term().clear_last_lines(c.frozen);
    }
}

/// L'étape en cours est finie: il ne reste que sa ligne.
pub fn done(summary: &str) {
    let mut g = lock();
    let Some(s) = g.as_mut() else { return };
    let Some(c) = s.cur.take() else { return };
    close(&c);
    let time = duration(c.started.elapsed());
    log::write(&format!(
        "ok [{}/{}] {}: {summary} ({time})",
        c.n, s.total, c.name
    ));
    print(&step_line(
        &style("✓").green().to_string(),
        c.n,
        s.total,
        &c.name,
        summary,
        &time,
    ));
}

/// Une étape qui n'existe pas encore dans cette version (CrowdSec).
pub fn later(name: &str, text: &str) {
    let mut g = lock();
    let Some(s) = g.as_mut() else { return };
    let n = s.first + s.count;
    s.count += 1;
    log::write(&format!("-- [{n}/{}] {name}: {text}", s.total));
    print(&format!(
        "  {} {} {}",
        style("-").dim(),
        counter(n, s.total),
        style(format!("{name:<NAME_W$} {text}")).dim()
    ));
}

/// L'étape en cours a échoué: ✗, puis les dernières lignes du log et son chemin (D37).
/// Rend true si l'erreur a été affichée (sinon l'appelant l'affiche).
pub fn fail(error: &str) -> bool {
    log::write(&format!("error    {error}"));
    let mut g = lock();
    let Some(s) = g.as_mut() else { return false };
    let Some(c) = s.cur.take() else { return false };
    close(&c);
    print(&step_line(
        &style("✗").red().to_string(),
        c.n,
        s.total,
        &c.name,
        &c.doing,
        &duration(c.started.elapsed()),
    ));
    print("");
    for (i, line) in error_lines(error).iter().enumerate() {
        let label = if i == 0 { "error" } else { "" };
        print(&format!("  {} {line}", style(format!("{label:<5}")).red()));
    }
    let tail = log::tail(10);
    if !tail.is_empty() {
        print("");
        print(&format!("  {}", style("last lines of the log").dim()));
        for l in tail {
            print(&format!("  {}", style(fit(&l, WIDTH - 2)).dim()));
        }
    }
    if let Some(p) = log::path() {
        print("");
        print(&format!("  {:<9}{p}", "log"));
    }
    true
}

/// Les lignes d'une erreur à afficher, passées à la ligne à 80 colonnes (« error » occupe les 8
/// premières). Au plus 6 lignes: le reste est dans le log.
fn error_lines(error: &str) -> Vec<String> {
    let mut out: Vec<String> = error.lines().flat_map(|l| wrap(l, WIDTH - 8)).collect();
    if out.len() > 6 {
        out.truncate(6);
        out[5] = fit(&format!("{} …", out[5]), WIDTH - 8);
    }
    out
}

/// Passe un texte à la ligne entre les mots, sans dépasser `width` colonnes (un mot plus long
/// que la ligne est coupé).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(head);
        }
        let len = line.chars().count();
        if len > 0 && len + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_with_and_without_install_sh() {
        assert_eq!(numbering(true), (2, 7));
        assert_eq!(numbering(false), (1, 6));
    }

    #[test]
    fn errors_wrap_instead_of_being_cut() {
        let e = "*.example.com does not resolve: point an A record for *.example.com to this server, then run dotbox init again";
        let lines = error_lines(e);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.chars().count() <= WIDTH - 8));
        assert_eq!(lines.join(" "), e);
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("abcdefghijkl xy", 5), ["abcde", "fghij", "kl xy"]);
        let many = "word ".repeat(200);
        let lines = error_lines(&many);
        assert_eq!(lines.len(), 6);
        assert!(lines[5].ends_with('…') || lines[5].ends_with(" …"));
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_millis(400)), "0.4 s");
        assert_eq!(duration(Duration::from_secs(48)), "48 s");
        assert_eq!(duration(Duration::from_secs(73)), "1 min 13 s");
    }

    #[test]
    fn step_lines_fit_in_80_columns() {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
        let l = step_line(
            "✓",
            4,
            7,
            "Docker",
            "29.9.3, local logs, live-restore",
            "48 s",
        );
        assert_eq!(
            l,
            "  ✓ [4/7] Docker        29.9.3, local logs, live-restore                    48 s"
        );
        assert_eq!(l.chars().count(), WIDTH);
        let long = step_line("✓", 2, 7, "System", &"x".repeat(100), "0.4 s");
        assert_eq!(long.chars().count(), WIDTH);
        assert!(long.contains('…'));
    }
}
