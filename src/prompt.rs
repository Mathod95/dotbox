//! Questions posées dans le terminal, façon cliclack (mockup final de CLI Output): ◆ la question en
//! cours, ◇ une question répondue, ▲ une réponse refusée, avec sa raison en dessous. Elles
//! s'affichent sous la ligne de l'étape en cours (ui::ask) et disparaissent quand l'étape est
//! validée. Sans terminal, aucune question: l'appelant décide quoi faire.

use cliclack::{StringCursor, Theme, ThemeState};
use std::io::IsTerminal;

/// Le thème par défaut de cliclack, dont on reprend le rendu.
struct Base;
impl Theme for Base {}

/// Le thème de Dot Box: le rendu de cliclack, décalé de deux colonnes comme les étapes, avec une
/// indication sous la question en cours (└  ...).
struct Dot {
    hint: String,
}

fn indent(text: String) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

impl Theme for Dot {
    fn format_header(&self, state: &ThemeState, prompt: &str) -> String {
        indent(Base.format_header(state, prompt))
    }
    fn format_input(&self, state: &ThemeState, cursor: &StringCursor) -> String {
        indent(Base.format_input(state, cursor))
    }
    fn format_placeholder(&self, state: &ThemeState, cursor: &StringCursor) -> String {
        indent(Base.format_placeholder(state, cursor))
    }
    fn format_confirm(&self, state: &ThemeState, confirm: bool) -> String {
        indent(Base.format_confirm(state, confirm))
    }
    fn format_footer_with_message(&self, state: &ThemeState, message: &str) -> String {
        let message = match (state, message) {
            (ThemeState::Active, "") => self.hint.as_str(),
            _ => message,
        };
        indent(Base.format_footer_with_message(state, message))
    }
    fn password_mask(&self) -> char {
        '▪'
    }
}

/// Lignes qu'une question laisse une fois répondue: la question, la réponse, puis « │ ».
pub fn lines_left(question: &str) -> usize {
    question.lines().count().max(1) + 2
}

pub fn is_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

fn hint(text: &str) {
    cliclack::set_theme(Dot {
        hint: text.to_string(),
    });
}

fn cancelled(e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::Interrupted => "cancelled, nothing else changed".to_string(),
        _ => format!("cannot ask in this terminal: {e}"),
    }
}

/// Une réponse texte. `default` est déjà écrit dans le champ, à garder avec Entrée.
pub fn input(
    question: &str,
    hint_text: &str,
    default: Option<&str>,
    check: impl Fn(&str) -> Result<(), String> + 'static,
) -> Result<String, String> {
    hint(hint_text);
    let mut q = cliclack::input(question).validate(move |s: &String| check(s.trim()));
    if let Some(d) = default {
        q = q.default_input(d);
    }
    let answer: String = q.interact().map_err(cancelled)?;
    crate::ui::prompted(lines_left(question));
    Ok(answer.trim().to_string())
}

/// Un mot de passe, masqué pendant la saisie. Vide accepté seulement si `check` l'accepte.
pub fn password(
    question: &str,
    hint_text: &str,
    check: impl Fn(&str) -> Result<(), String> + 'static,
) -> Result<String, String> {
    hint(hint_text);
    let answer = cliclack::password(question)
        .allow_empty()
        .validate(move |s: &String| check(s))
        .interact()
        .map_err(cancelled)?;
    crate::ui::prompted(lines_left(question));
    Ok(answer)
}

/// Oui ou non, non par défaut. Sans terminal: non.
pub fn confirm(question: &str, hint_text: &str) -> Result<bool, String> {
    if !is_terminal() {
        return Ok(false);
    }
    hint(hint_text);
    let answer = cliclack::confirm(question)
        .initial_value(false)
        .interact()
        .map_err(cancelled)?;
    crate::ui::prompted(lines_left(question));
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_left_after_an_answer() {
        assert_eq!(lines_left("Domain of this server"), 3);
        assert_eq!(lines_left("Two\nlines"), 4);
    }

    #[test]
    fn theme_indents_and_shows_the_hint() {
        console::set_colors_enabled_stderr(false);
        console::set_colors_enabled(false);
        let t = Dot {
            hint: "Enter alone for a random one".into(),
        };
        assert_eq!(
            t.format_header(&ThemeState::Active, "Authelia password"),
            "  ◆  Authelia password\n"
        );
        assert_eq!(
            t.format_footer(&ThemeState::Active),
            "  └  Enter alone for a random one\n"
        );
        assert_eq!(
            t.format_footer_with_message(&ThemeState::Error("too short".into()), ""),
            "  └  too short\n"
        );
        assert_eq!(t.format_footer(&ThemeState::Submit), "  │\n");
    }
}
