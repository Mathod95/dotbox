//! Le cadre façon cliclack de `dotbox status` et `dotbox doctor` (page CLI, propositions #92 et
//! #93): `┌` le titre, une ligne par élément avec son symbole, le détail sous la ligne après `│`,
//! puis `└` le résumé. ◇ en vert: tout va bien. ▲ en jaune: à regarder. ■ en rouge: une erreur.

use console::{StyledObject, style};
use std::io::Write;

/// Une ligne sur stdout. Une sortie fermée (dotbox status | head) n'est pas une erreur: la
/// commande s'arrête sans paniquer.
pub fn put(line: &str) {
    if crate::out::quiet() {
        return;
    }
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{line}").is_err() {
        std::process::exit(0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    Ok,
    Warn,
    Error,
}

impl Mark {
    pub fn symbol(self) -> StyledObject<&'static str> {
        match self {
            Mark::Ok => style("◇").green(),
            Mark::Warn => style("▲").yellow(),
            Mark::Error => style("■").red(),
        }
    }

    pub fn paint(self, text: &str) -> String {
        match self {
            Mark::Ok => style(text).green().to_string(),
            Mark::Warn => style(text).yellow().to_string(),
            Mark::Error => style(text).red().to_string(),
        }
    }
}

fn bar_char(c: &str) -> StyledObject<&str> {
    style(c).dim()
}

pub fn open(title: &str) {
    crate::clack::put(&format!("{}  {}", bar_char("┌"), style(title).bold()));
    bar();
}

pub fn bar() {
    crate::clack::put(&format!("{}", bar_char("│")));
}

/// Une ligne: le symbole, le nom sur `width` colonnes, le texte.
pub fn item(mark: Mark, name: &str, width: usize, text: &str) {
    crate::clack::put(&format!("{}  {name:<width$}{text}", mark.symbol()));
}

/// Sous la ligne: un détail, en discret.
pub fn sub(text: &str) {
    crate::clack::put(&format!("{}    {}", bar_char("│"), style(text).dim()));
}

/// Sous la ligne: un problème, dans sa couleur.
pub fn sub_mark(mark: Mark, text: &str) {
    crate::clack::put(&format!("{}    {}", bar_char("│"), mark.paint(text)));
}

pub fn close(text: &str) {
    bar();
    crate::clack::put(&format!("{}  {text}", bar_char("└")));
}

/// Le cadre seulement dans un terminal: un script ou une redirection reçoit du texte simple.
pub fn fancy() -> bool {
    console::Term::stdout().is_term()
}

/// Un indicateur pendant une attente (doctor prend quelques secondes), effacé à la fin.
pub fn waiting<T>(message: &str, f: impl FnOnce() -> T) -> T {
    if !fancy() {
        return f();
    }
    let spinner = cliclack::spinner();
    spinner.start(message);
    let out = f();
    spinner.clear();
    out
}

/// Une ligne d'un élément éteint (une app du catalogue pas installée): ○ en discret.
pub fn item_off(name: &str, width: usize, text: &str) {
    put(&format!(
        "{}  {name:<width$}{}",
        style("○").dim(),
        style(text).dim()
    ));
}

/// Un champ sous le titre: « │  Image      lscr.io/... ».
pub fn field(name: &str, value: &str) {
    put(&format!(
        "{}  {:<11}{value}",
        bar_char("│"),
        style(name).dim()
    ));
}
