//! Questions posées dans le terminal. Sans terminal, aucune question: l'appelant décide quoi faire.

use std::io::{IsTerminal, Write};

pub fn is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Pose une question, rend la réponse sans espaces autour. None si l'entrée est fermée.
pub fn ask(verb: &str, question: &str) -> Result<Option<String>, String> {
    print!("  {verb:<9}{question} ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    let n = std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok((n > 0).then(|| line.trim().to_string()))
}

/// Oui ou non, non par défaut. Sans terminal: non.
pub fn confirm(question: &str) -> Result<bool, String> {
    if !is_terminal() {
        return Ok(false);
    }
    let answer = ask("confirm", question)?.unwrap_or_default();
    Ok(matches!(answer.to_lowercase().as_str(), "y" | "yes"))
}
