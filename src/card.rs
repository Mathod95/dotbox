//! La carte de fin de `dotbox init` (mockup final de CLI Output): le compte et son mot de passe
//! Authelia, les commandes pour le relire, le log, les composants installés, puis la
//! post-installation avec la page Getting Started de chaque étape. Sur 80 colonnes, plus large
//! seulement si le domaine est long.

use console::{measure_text_width, style};

/// Les pages de la doc (tools/site/docs/<page>.html, publiées par GitHub Pages).
pub const DOCS: &str = "https://mathod95.github.io/dotbox/getting-started/";

pub struct Component {
    pub name: String,
    pub version: String,
    /// « running », « healthy »... vide: pas encore disponible
    pub state: String,
    pub address: String,
}

pub struct Info {
    pub user: String,
    /// Some: le mot de passe vient d'être créé (affiché une fois). None: déjà gardé ou absent.
    pub password: Option<String>,
    /// le mot de passe est gardé pour root (sudo dotbox authelia password)
    pub password_kept: bool,
    pub log: String,
    pub components: Vec<Component>,
    pub elapsed: String,
}

/// Les étapes de post-installation: ce qu'il faut faire, et la page qui l'explique.
pub fn next_steps(user: &str) -> Vec<(String, &'static str)> {
    vec![
        (format!("exit, then: exec su - {user}"), "installation"),
        (
            "open the Authelia address, add a 6-digit code app".into(),
            "post-installation",
        ),
        (
            "check the server: doctor, SSH, backups".into(),
            "post-installation",
        ),
    ]
}

const COLS: [usize; 3] = [12, 11, 13];
const MIN_INNER: usize = 76;

fn g(t: &str) -> String {
    style(t).green().to_string()
}
fn d(t: &str) -> String {
    style(t).dim().to_string()
}

/// Une ligne de la carte: « │ » + contenu complété d'espaces + « │ ».
fn row(inner: usize, content: &str) -> String {
    let w = measure_text_width(content);
    format!(
        "{}{content}{}{}",
        g("  │"),
        " ".repeat(inner.saturating_sub(w)),
        g("│")
    )
}

fn labeled(inner: usize, label: &str, value: &str) -> String {
    row(inner, &format!("  {label:<19}{value}"))
}

/// Une ligne de séparation, avec des jonctions aux colonnes données (├──┬──┤).
fn grid(inner: usize, left: char, right: char, joints: &[(usize, char)]) -> String {
    let mut line: Vec<char> = vec!['─'; inner];
    for (pos, c) in joints {
        if *pos < inner {
            line[*pos] = *c;
        }
    }
    g(&format!(
        "  {left}{}{right}",
        line.into_iter().collect::<String>()
    ))
}

/// Positions des séparateurs d'un tableau dont les colonnes ont ces largeurs.
fn joints(widths: &[usize]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut pos = 0;
    for w in &widths[..widths.len() - 1] {
        pos += w;
        out.push(pos);
        pos += 1;
    }
    out
}

fn cells(widths: &[usize], values: &[String]) -> String {
    let mut out = String::new();
    for (i, (v, w)) in values.iter().zip(widths).enumerate() {
        let text = format!(" {v}");
        let pad = w.saturating_sub(measure_text_width(&text));
        out.push_str(&text);
        out.push_str(&" ".repeat(pad));
        if i < widths.len() - 1 {
            out.push_str(&d("│"));
        }
    }
    out
}

fn state(s: &str) -> String {
    match s {
        "" => d("○ later"),
        "healthy" | "running" => format!("{} {s}", style("●").green()),
        other => format!("{} {other}", style("●").red()),
    }
}

pub fn render(info: &Info) -> Vec<String> {
    let addr_w = info
        .components
        .iter()
        .map(|c| c.address.chars().count() + 3)
        .max()
        .unwrap_or(0);
    let fixed: usize = COLS.iter().sum::<usize>() + COLS.len();
    let inner = MIN_INNER.max(fixed + addr_w);
    let widths = [COLS[0], COLS[1], COLS[2], inner - fixed];
    let comp_joints = joints(&widths);
    let next_widths = [4, inner - 5];
    let next_joint = joints(&next_widths)[0];

    let mut out = Vec::new();
    // titre
    let title = format!(
        "{} {}",
        style("✓").green(),
        style("Dot Box is ready").bold()
    );
    let fill =
        inner + 4 - (5 + measure_text_width(&title) + 1 + 1 + info.elapsed.chars().count() + 3);
    out.push(format!(
        "{}{title} {} {}{}",
        g("  ╭─ "),
        g(&"─".repeat(fill)),
        d(&info.elapsed),
        g(" ─╮")
    ));
    out.push(row(inner, ""));
    out.push(labeled(
        inner,
        "User",
        &format!("{}{}", info.user, d("  group admins")),
    ));
    let pw = match (&info.password, info.password_kept) {
        (Some(p), _) => format!("{}{}", style(p).bold(), d("  shown once")),
        (None, true) => d("kept for root, see the command below"),
        (None, false) => d("not kept (account created before Dot Box kept it)"),
    };
    out.push(labeled(inner, "Authelia password", &pw));
    out.push(grid(inner, '├', '┤', &[]));
    let cmd = |c: &str| style(c).blue().to_string();
    out.push(labeled(
        inner,
        "Password",
        &format!(
            "{}{}",
            cmd("sudo dotbox authelia password"),
            d("  read it again")
        ),
    ));
    out.push(labeled(
        inner,
        "Verify",
        &format!(
            "{}{}",
            cmd("sudo dotbox authelia verification"),
            d("  code for a device")
        ),
    ));
    out.push(labeled(
        inner,
        "Log",
        &format!("{}{}", cmd(&info.log), d("  root only")),
    ));

    // les composants
    let j: Vec<(usize, char)> = comp_joints.iter().map(|p| (*p, '┬')).collect();
    out.push(grid(inner, '├', '┤', &j));
    let header = |t: &str| style(t).bold().to_string();
    out.push(row(
        inner,
        &cells(
            &widths,
            &[
                header("Component"),
                header("Version"),
                header("State"),
                header("Address"),
            ],
        ),
    ));
    let j: Vec<(usize, char)> = comp_joints.iter().map(|p| (*p, '┼')).collect();
    out.push(grid(inner, '├', '┤', &j));
    for c in &info.components {
        let later = c.state.is_empty();
        let dim_if = |t: &str| if later { d(t) } else { t.to_string() };
        let address = if c.address.is_empty() {
            d("-")
        } else {
            style(&c.address).underlined().to_string()
        };
        let version = if c.version.is_empty() {
            d("-")
        } else {
            dim_if(&c.version)
        };
        out.push(row(
            inner,
            &cells(
                &widths,
                &[dim_if(&c.name), version, state(&c.state), address],
            ),
        ));
    }

    // la post-installation: les colonnes des composants se ferment, celle du numéro s'ouvre
    let mut j: Vec<(usize, char)> = comp_joints.iter().map(|p| (*p, '┴')).collect();
    j.push((
        next_joint,
        if comp_joints.contains(&next_joint) {
            '┼'
        } else {
            '┬'
        },
    ));
    out.push(grid(inner, '├', '┤', &j));
    out.push(row(
        inner,
        &cells(&next_widths, &[header("#"), header("Post-installation")]),
    ));
    out.push(grid(inner, '├', '┤', &[(next_joint, '┼')]));
    for (i, (text, page)) in next_steps(&info.user).iter().enumerate() {
        out.push(row(
            inner,
            &cells(
                &next_widths,
                &[style(i + 1).yellow().to_string(), text.clone()],
            ),
        ));
        out.push(row(
            inner,
            &cells(
                &next_widths,
                &[
                    String::new(),
                    style(format!("{DOCS}{page}/")).underlined().to_string(),
                ],
            ),
        ));
    }
    out.push(grid(inner, '╰', '╯', &[(next_joint, '┴')]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(domain: &str) -> Info {
        Info {
            user: "admin".into(),
            password: Some("Xq7vR2mLp9sTe4KcWb8nZd3F".into()),
            password_kept: true,
            log: "/opt/dotbox/logs/init-2026-11-16-1402.log".into(),
            components: vec![
                Component {
                    name: "Docker".into(),
                    version: "29.9.3".into(),
                    state: "running".into(),
                    address: String::new(),
                },
                Component {
                    name: "Traefik".into(),
                    version: "v3.7.19".into(),
                    state: "healthy".into(),
                    address: format!("https://traefik.{domain}"),
                },
                Component {
                    name: "Authelia".into(),
                    version: "4.39.28".into(),
                    state: "healthy".into(),
                    address: format!("https://auth.{domain}"),
                },
                Component {
                    name: "CrowdSec".into(),
                    version: String::new(),
                    state: String::new(),
                    address: String::new(),
                },
            ],
            elapsed: "1 min 13 s".into(),
        }
    }

    #[test]
    fn card_is_80_columns_and_aligned() {
        console::set_colors_enabled(false);
        let lines = render(&info("dotbox.example.com"));
        for l in &lines {
            assert_eq!(measure_text_width(l), 80, "{l}");
        }
        let text = lines.join("\n");
        assert!(text.contains(
            "│ Traefik    │ v3.7.19   │ ● healthy   │ https://traefik.dotbox.example.com  │"
        ));
        assert!(text.contains("│ CrowdSec   │ -         │ ○ later     │ -"));
        assert!(text.contains("├────┬───────┴───────────┴─────────────┴"));
        assert!(text.contains("│ 1  │ exit, then: exec su - admin"));
        assert!(
            text.contains("https://mathod95.github.io/dotbox/getting-started/post-installation/")
        );
        assert!(lines.last().unwrap().starts_with("  ╰────┴"));
    }

    #[test]
    fn a_long_domain_widens_the_card() {
        console::set_colors_enabled(false);
        let lines = render(&info("a-much-longer-subdomain.example.com"));
        let w = measure_text_width(&lines[0]);
        assert!(w > 80);
        for l in &lines {
            assert_eq!(measure_text_width(l), w, "{l}");
        }
    }

    #[test]
    fn every_linked_page_exists() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let site = std::fs::read_to_string(root.join("tools/build_site.py")).unwrap();
        for (_, page) in next_steps("admin") {
            assert!(
                root.join(format!("tools/site/docs/{page}.html")).exists(),
                "{page}.html"
            );
            assert!(
                site.contains(&format!("\"slug\": \"{page}\"")),
                "{page} in DOCS of build_site.py"
            );
        }
    }
}
