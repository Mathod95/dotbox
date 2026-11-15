//! Sortie au style de la page CLI: un verbe sur 9 colonnes, puis le détail.

pub fn say(verb: &str, detail: &str) {
    println!("  {verb:<9}{detail}");
}

pub fn error(detail: &str) {
    eprintln!("  {:<9}{detail}", "error");
}
