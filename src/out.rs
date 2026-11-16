//! Sortie au style de la page CLI: un verbe sur 9 colonnes, puis le détail.
//! Une sortie fermée (par exemple `dotbox init | head`) ne doit jamais interrompre une
//! installation en cours: les erreurs d'écriture sont ignorées au lieu de faire planter le
//! programme (Journal, J68).

use std::io::Write;

pub fn say(verb: &str, detail: &str) {
    let _ = writeln!(std::io::stdout(), "  {verb:<9}{detail}");
}

pub fn blank() {
    let _ = writeln!(std::io::stdout());
}

pub fn error(detail: &str) {
    let _ = writeln!(std::io::stderr(), "  {:<9}{detail}", "error");
}
