//! `dotbox --help` (proposition #109A de la page CLI, validée le 20 novembre 2026): l'aide
//! rangée comme le terminal de la page CLI, par groupes, avec les arguments de chaque commande,
//! en couleurs, une ligne vide entre les groupes. L'aide d'une commande (`dotbox install --help`)
//! reste celle de clap. Seules les commandes codées y sont.

use console::style;

/// Largeur de la colonne « commande et arguments ».
const WIDTH: usize = 34;

/// (commande, arguments, description).
type Entry = (&'static str, &'static str, &'static str);

/// (groupe, ses commandes), dans l'ordre de la page CLI.
pub const GROUPS: &[(&str, &[Entry])] = &[
    (
        "SERVER",
        &[
            (
                "init",
                "[--domain <d>] [--user <n>]",
                "Set up the server, then the core and the service",
            ),
            (
                "status",
                "",
                "The core and the apps: state, address, last doctor",
            ),
            ("doctor", "", "Check the server and say what to do"),
            ("upgrade", "", "Update Dot Box itself (sudo)"),
            ("version", "", "This command's version and the service's"),
        ],
    ),
    (
        "APPS",
        &[
            ("list", "[search]", "The catalog, with the installed apps"),
            (
                "info",
                "<app>",
                "Image, state, address, access, folders, ports",
            ),
            (
                "install",
                "<app>...",
                "Install apps, behind Traefik and Authelia",
            ),
            (
                "remove",
                "<app> [--purge]",
                "Remove an app, keep its config",
            ),
            (
                "update",
                "[<app>...] [--rollback]",
                "Update apps one at a time",
            ),
            ("start", "<app>", "Start an app"),
            ("stop", "<app>", "Stop an app"),
            ("restart", "<app>", "Restart an app"),
            (
                "logs",
                "<app> [-f] [--tail <n>]",
                "An app's logs, or traefik, authelia, crowdsec",
            ),
            (
                "exec",
                "<app> -- <command>",
                "Run a command in an app's container (sudo)",
            ),
            (
                "edit",
                "<app>",
                "Apply the app's compose.override.yml (sudo)",
            ),
            (
                "add",
                "<compose.yml> --name <app>",
                "Turn a compose file into an app (sudo)",
            ),
        ],
    ),
    (
        "STATE",
        &[(
            "diff",
            "",
            "What differs from what is wanted, changing nothing",
        )],
    ),
    (
        "USERS",
        &[
            (
                "authelia password",
                "[<name>]",
                "An Authelia account's web password (sudo)",
            ),
            (
                "authelia verification",
                "",
                "The last code Authelia sent (sudo)",
            ),
        ],
    ),
    (
        "CONFIGURATION",
        &[(
            "completion",
            "bash|zsh|fish",
            "The shell completion script, on standard output",
        )],
    ),
];

const OPTIONS: &[(&str, &str)] = &[
    ("--local", "Do the work here, as root, without the service"),
    ("--json", "JSON output, for scripts"),
    ("-y, --yes", "Answer yes to confirmations"),
    ("-q, --quiet", "Only print errors"),
    ("--no-color", "No colors (NO_COLOR works too)"),
    (
        "-h, --help",
        "This help, or a command's: dotbox <command> --help",
    ),
    ("-V, --version", "The version, on one line"),
];

const DOCS: &str = "https://mathod95.github.io/dotbox/getting-started/cli/";

/// Les options globales, qui peuvent accompagner `--help` sans changer l'aide.
const GLOBAL: &[&str] = &[
    "--local",
    "--json",
    "-y",
    "--yes",
    "-q",
    "--quiet",
    "--no-color",
];

/// Ce que demandent les arguments: `Some(true)` l'aide demandée (`help`, `-h`, `--help`),
/// `Some(false)` aucune commande, `None` le reste, laissé à clap.
pub fn wanted(args: &[String]) -> Option<bool> {
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !GLOBAL.contains(a))
        .collect();
    match rest.as_slice() {
        [] => Some(false),
        ["help"] | ["-h"] | ["--help"] => Some(true),
        _ => None,
    }
}

/// L'aide, avec ses couleurs si le terminal les accepte (console suit NO_COLOR et --no-color).
pub fn text() -> String {
    let mut s = String::new();
    s += &format!(
        "{}{}\n\n",
        style(format!("Dot Box {}", env!("CARGO_PKG_VERSION"))).bold(),
        style(" · self-hosted apps on Docker Compose, behind Traefik and Authelia").dim()
    );
    s += &format!(
        "{}{}\n",
        style("Usage").green(),
        style("  dotbox <command> [args] [options]").bold()
    );
    for (group, cmds) in GROUPS {
        s += &format!("\n{}\n", style(group).green());
        for (name, args, what) in *cmds {
            let args = if args.is_empty() {
                String::new()
            } else {
                format!(" {args}")
            };
            let pad = WIDTH.saturating_sub(name.len() + args.len());
            s += &format!(
                "  {}{}{}{what}\n",
                style(name).bold(),
                style(args).dim(),
                " ".repeat(pad)
            );
        }
    }
    s += &format!("\n{}\n", style("OPTIONS").green());
    for (flag, what) in OPTIONS {
        s += &format!("  {}{what}\n", style(format!("{flag:WIDTH$}")).bold());
    }
    s += &format!("\n{}{DOCS}\n", style("Docs  ").dim());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn help_asked() {
        assert_eq!(wanted(&args(&[])), Some(false));
        assert_eq!(wanted(&args(&["--no-color"])), Some(false));
        assert_eq!(wanted(&args(&["help"])), Some(true));
        assert_eq!(wanted(&args(&["-h"])), Some(true));
        assert_eq!(wanted(&args(&["--no-color", "--help"])), Some(true));
        assert_eq!(wanted(&args(&["install", "--help"])), None);
        assert_eq!(wanted(&args(&["help", "install"])), None);
        assert_eq!(wanted(&args(&["--version"])), None);
        assert_eq!(wanted(&args(&["status"])), None);
    }

    #[test]
    fn every_command_listed() {
        use clap::CommandFactory;
        let listed: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, c)| c.iter().map(|(n, _, _)| n.split(' ').next().unwrap_or(n)))
            .collect();
        for c in crate::Cli::command().get_subcommands() {
            if c.is_hide_set() {
                continue;
            }
            assert!(
                listed.contains(&c.get_name()),
                "{} is not in the help",
                c.get_name()
            );
        }
        for n in &listed {
            assert!(
                crate::Cli::command().find_subcommand(n).is_some(),
                "{n} is in the help but not in the binary"
            );
        }
    }

    /// La page CLI montre les mêmes lignes (le champ « h » de GROUPS et FLAGS, tools/site/docs/cli.html).
    #[test]
    fn same_as_the_cli_page() {
        let page = include_str!("../tools/site/docs/cli.html");
        let q = |t: &str| serde_json::to_string(t).unwrap_or_default();
        for (_, cmds) in GROUPS {
            for (name, args, what) in *cmds {
                let h = format!("h: [{}, {}, {}]", q(name), q(args), q(what));
                assert!(page.contains(&h), "the CLI page has no {h}");
            }
        }
        for (flag, what) in OPTIONS {
            let h = format!("{{ f: {}, h: {},", q(flag), q(what));
            assert!(page.contains(&h), "the CLI page has no {h}");
        }
        for (group, _) in GROUPS {
            assert!(
                page.to_uppercase()
                    .contains(&format!("EN: \"{group}\" }}, CMDS")),
                "the CLI page has no group {group}"
            );
        }
    }

    #[test]
    fn lines_fit() {
        console::set_colors_enabled(false);
        let t = text();
        for l in t.lines() {
            assert!(l.chars().count() <= 90, "{l}");
        }
        assert!(
            t.contains("\n  remove <app> [--purge]            Remove an app, keep its config\n")
        );
        assert!(t.contains("\n\nAPPS\n  list [search]"));
    }
}
