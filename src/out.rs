//! Sortie au style de la page CLI: un verbe sur 9 colonnes, puis le détail.
//! Une sortie fermée (par exemple `dotbox init | head`) ne doit jamais interrompre une
//! installation en cours: les erreurs d'écriture sont ignorées au lieu de faire planter le
//! programme (Journal, J68).

use std::io::Write;

/// Une ligne « verbe détail ». Pendant dotbox init, c'est une sous-étape: elle va dans le log et
/// sous la ligne de l'étape en cours (ui.rs).
static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// -q: seulement les erreurs (13.10).
pub fn set_quiet(on: bool) {
    QUIET.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn quiet() -> bool {
    QUIET.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn say(verb: &str, detail: &str) {
    if crate::ui::active() {
        crate::ui::sub(verb, detail);
        return;
    }
    let _ = writeln!(std::io::stdout(), "  {verb:<9}{detail}");
}

pub fn error(detail: &str) {
    let _ = writeln!(std::io::stderr(), "  {:<9}{detail}", "error");
}
