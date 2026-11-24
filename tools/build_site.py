#!/usr/bin/env python3
"""Construit les pages de Dot Box à partir des gabarits de tools/site/.

- tools/site/index.html : la homePage (accueil) ;
- tools/site/releases.html : la releasePage, données dans tools/site/releases.json ;
- tools/site/roadmap.html : la roadmap (gabarit de la releasePage), données dans tools/site/roadmap.json ;
- tools/site/docs/ : la documentation :
  layout.html (en-tête, barre de navigation à gauche, article, pied de page)
  et une page par fichier (basics.html, cli.html), découpée en sections
  <!-- LEDE -->, <!-- CONTENT -->, <!-- CSS --> et <!-- JS -->.

Sans option : le site GitHub Pages dans site/ (pages complètes, liens relatifs) :
  site/index.html, site/releases/, site/roadmap/, site/getting-started/<page>/
Publié par .github/workflows/pages.yml à chaque push sur main.

Aperçus en artefacts claude.ai (pages sans en-tête HTML, liens vers les autres artefacts) :
  --fragment FICHIER           l'accueil
  --releases-fragment FICHIER  les releases
  --roadmap-fragment FICHIER   la roadmap
  --docs-fragments DOSSIER     une page par page de documentation (basics.html, cli.html)
"""
import argparse
import json
import pathlib
import re
import shutil

ROOT = pathlib.Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
SITE_SRC = ROOT / "tools" / "site"
HOME_TEMPLATE = SITE_SRC / "index.html"
RELEASES_TEMPLATE = SITE_SRC / "releases.html"
RELEASES_DATA = SITE_SRC / "releases.json"
ROADMAP_TEMPLATE = SITE_SRC / "roadmap.html"
ROADMAP_DATA = SITE_SRC / "roadmap.json"
DOCS_SRC = SITE_SRC / "docs"
DOCS_LAYOUT = DOCS_SRC / "layout.html"

# la documentation (D127, 23 novembre 2026): des sections, comme Getting Started, chacune titrée
# dans la barre de gauche, ses pages dans l'ordre. « Travail »: les pages de travail en français,
# seulement dans le site local (D125). « group »: un groupe repliable dans sa section, ouvert
# seulement sur une de ses pages (D128: « Dot Box apps »).
DOCS = [
    {"slug": "basics", "name": "Basics", "dots": "BASICS", "section": "Getting Started"},
    {"slug": "prerequisites", "name": "Prerequisites", "dots": "PREREQUISITES", "section": "Getting Started"},
    {"slug": "installation", "name": "Installation", "dots": "INSTALL", "section": "Getting Started"},
    {"slug": "post-installation", "name": "Post-installation", "dots": "POST-INSTALL", "section": "Getting Started"},
    {"slug": "accessing-apps", "name": "Accessing Apps", "dots": "ACCESS", "section": "Getting Started"},
    {"slug": "update", "name": "Update", "dots": "UPDATE", "section": "Getting Started"},
    {"slug": "backup", "name": "Backup", "dots": "BACKUP", "section": "Backup / Restore"},
    {"slug": "restore", "name": "Restore & Migration", "dots": "RESTORE", "section": "Backup / Restore"},
    {"slug": "apps", "name": "Overview", "dots": "APPS", "section": "Apps"},
    {"slug": "your-own-apps", "name": "Your own apps", "dots": "YOUR-APPS", "section": "Apps"},
    {"slug": "prowlarr", "name": "Prowlarr", "dots": "PROWLARR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "radarr", "name": "Radarr", "dots": "RADARR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "qbittorrent", "name": "qBittorrent", "dots": "QBITTORRENT", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "qui", "name": "qui", "dots": "QUI", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "sonarr", "name": "Sonarr", "dots": "SONARR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "uptime-kuma", "name": "Uptime Kuma", "dots": "UPTIME-KUMA", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "code-server", "name": "code-server", "dots": "CODE-SERVER", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "jellyfin", "name": "Jellyfin", "dots": "JELLYFIN", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "plex", "name": "Plex", "dots": "PLEX", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "tautulli", "name": "Tautulli", "dots": "TAUTULLI", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "autobrr", "name": "autobrr", "dots": "AUTOBRR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "maintainerr", "name": "Maintainerr", "dots": "MAINTAINERR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "wg-easy", "name": "wg-easy", "dots": "WG-EASY", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "seerr", "name": "Seerr", "dots": "SEERR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "wizarr", "name": "Wizarr", "dots": "WIZARR", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "vaultwarden", "name": "Vaultwarden", "dots": "VAULTWARDEN", "section": "Apps", "group": "Dot Box apps"},
    {"slug": "cli", "name": "CLI", "dots": "CLI", "section": "References"},
    {"slug": "api", "name": "API", "dots": "API", "section": "References"},
    {"slug": "web-ui", "name": "Web UI", "dots": "WEB-UI", "section": "References"},
    {"slug": "configuration", "name": "Configuration", "dots": "CONFIGURATION", "section": "References"},
    {"slug": "motd", "name": "MOTD", "dots": "MOTD", "section": "References"},
    {"slug": "paths", "name": "Paths", "dots": "PATHS", "section": "References"},
    {"slug": "security", "name": "Security", "dots": "SECURITY", "section": "References"},
    {"slug": "renovate", "name": "Renovate", "dots": "RENOVATE", "section": "References"},
    {"slug": "sandbox", "name": "Sandbox", "dots": "SANDBOX", "section": "References"},
    {"slug": "journal", "name": "Journal", "dots": "JOURNAL", "section": "Travail"},
    {"slug": "cli-output", "name": "CLI Output", "dots": "CLI-OUTPUT", "section": "Travail"},
    {"slug": "en-vrac", "name": "En vrac", "dots": "EN-VRAC", "section": "Travail"},
]

# artefacts claude.ai : les pages se répondent par leurs boutons
ARTIFACT_PAGE = "https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE"
ARTIFACT_RELEASE = "https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd"
ARTIFACT_ROADMAP = "https://claude.ai/artifact/UxWsooQhdW7DtxpN4KpRw7"
ARTIFACT_DOCS = {
    "basics": "https://claude.ai/artifact/WPfzXWk2jWXWKbg2EFEMH9",
    "prerequisites": "https://claude.ai/artifact/VzSMUX2mPzRYozJpHxJBPo",
    "installation": "https://claude.ai/artifact/PYWicYHgqAF58DrKgsT79w",
    "post-installation": "https://claude.ai/artifact/7RCQm3P8QtJgXiezubksQ5",
    "update": "https://claude.ai/artifact/5YGzukkp3vmpCXCa3TWBQC",
    "accessing-apps": "https://claude.ai/artifact/XCLMhKPuuw8dN53WgeDbSY",
    "paths": "https://claude.ai/artifact/1hsUD8q5XEpjBEoqdxbtfh",
    "backup": "https://claude.ai/artifact/GMGiMWy3pVvZuLnz6g6mkM",
    "restore": "https://claude.ai/artifact/PZj8om1qDxRW4zcacLkQcS",
    "sandbox": "https://claude.ai/artifact/KxbLBXFF5kiVWdmpx3rb34",
    "cli": "https://claude.ai/artifact/GZz8HJqNBqMgY65VygEe9x",
    "journal": "https://claude.ai/artifact/WE96WdBsdMnxaUejtN6BUk",
    "cli-output": "https://claude.ai/artifact/KxU39np5ruPo9QJ6ciEXTz",
}
BLANK = ' target="_blank" rel="noopener"'
# Les pages de travail, en français (D123, D125): dans le repo, mais seulement dans le site local
# (--private). Sur GitHub Pages, ni ces pages, ni les liens qui y mènent.
PRIVATE = {"journal", "cli-output", "en-vrac"}
REMOVED = set()


def links(page, urls, target):
    """Liens entre les pages : adresses des artefacts, ou liens relatifs sur le site."""
    for slug in REMOVED:
        # un lien vers une page de travail absente: on garde son texte, sans le lien
        k = "DOC_" + ukey(slug)
        page = re.sub(r'<a href="__' + k + r'__[^"]*"(?:__' + k + r'_TARGET__)?>(.*?)</a>', r"\1", page, flags=re.S)
    for k, u in urls.items():
        page = page.replace("__" + k + "_TARGET__", target).replace("__" + k + "__", u)
    return page


def home(urls, target, title):
    page = HOME_TEMPLATE.read_text(encoding="utf-8").replace("__TITLE__", title)
    return links(page, urls, target)


def releases(urls, target, title):
    data = json.dumps(json.loads(RELEASES_DATA.read_text(encoding="utf-8")), ensure_ascii=False).replace("</", "<\\/")
    page = RELEASES_TEMPLATE.read_text(encoding="utf-8").replace("__DATA__", data).replace("__TITLE__", title)
    return links(page, urls, target)


def roadmap(urls, target, title):
    data = json.dumps(json.loads(ROADMAP_DATA.read_text(encoding="utf-8")), ensure_ascii=False).replace("</", "<\\/")
    docs = json.dumps({d["slug"]: urls["DOC_" + ukey(d["slug"])] for d in DOCS})
    page = (ROADMAP_TEMPLATE.read_text(encoding="utf-8").replace("__DATA__", data).replace("__DOCURLS__", docs)
            .replace("__DOCBLANK__", "true" if target else "false").replace("__TITLE__", title))
    return links(page, urls, target)


def ukey(slug):
    """Clé de lien d'une page : post-installation -> POST_INSTALLATION."""
    return slug.upper().replace("-", "_")


def sections(text):
    """Découpe un fichier de page en {LEDE, CONTENT, CSS, JS}."""
    parts = re.split(r"<!-- (LEDE|CONTENT|CSS|JS) -->\n", text)
    return {parts[i]: parts[i + 1].strip("\n") for i in range(1, len(parts), 2)}


def doc(slug, urls, target, title):
    """Une page de documentation : la mise en page commune, la barre de gauche et le contenu de la page."""
    i = [d["slug"] for d in DOCS].index(slug)
    page_def = DOCS[i]
    part = sections((DOCS_SRC / (slug + ".html")).read_text(encoding="utf-8"))
    nav = []
    section = None
    group = None
    for d in DOCS:
        key = "__DOC_" + ukey(d["slug"]) + "__"
        if d.get("group") != group or d["section"] != section:
            if group:
                nav.append("        </ul></details></li>")
            group = None
        if d["section"] != section:
            if section is not None:
                nav.append("      </ul>")
            section = d["section"]
            nav.append('      <h2>' + section.replace("&", "&amp;") + '</h2>')
            nav.append('      <ul class="nav">')
        if d.get("group") and d.get("group") != group:
            # un groupe repliable: ouvert seulement quand la page en cours en fait partie
            group = d["group"]
            is_open = " open" if page_def.get("group") == group else ""
            nav.append('        <li class="fold"><details' + is_open + '><summary class="grp">' + group.replace("&", "&amp;")
                       + '</summary><ul class="nav-sub">')
        li = '<li class="sub">' if group else "<li>"
        name = d["name"].replace("&", "&amp;")
        if d["slug"] == slug:
            nav.append("        " + li + '<a href="' + key + '" aria-current="page">' + name + '</a><ul class="toc" id="pageToc"></ul></li>')
        else:
            nav.append("        " + li + '<a href="' + key + '"__DOC_' + ukey(d["slug"]) + "_TARGET__>" + name + "</a></li>")
    if group:
        nav.append("        </ul></details></li>")
    nav.append("      </ul>")
    pn = []
    if i > 0:
        p = DOCS[i - 1]
        pn.append('<a class="prev" href="__DOC_' + ukey(p["slug"]) + '__"__DOC_' + ukey(p["slug"]) + '_TARGET__><small><span data-l="fr">← Précédent</span><span data-l="en">← Previous</span></small><b>' + p["name"] .replace("&", "&amp;") + "</b></a>")
    if i < len(DOCS) - 1:
        n = DOCS[i + 1]
        pn.append('<a class="next" href="__DOC_' + ukey(n["slug"]) + '__"__DOC_' + ukey(n["slug"]) + '_TARGET__><small><span data-l="fr">Suivant →</span><span data-l="en">Next →</span></small><b>' + n["name"] .replace("&", "&amp;") + "</b></a>")
    page = (DOCS_LAYOUT.read_text(encoding="utf-8")
            .replace("__TITLE__", title.replace("&", "&amp;"))
            .replace("__H1__", page_def["name"].replace("&", "&amp;"))
            .replace("__SECTION__", page_def["section"].replace("&", "&amp;"))
            .replace("__DOTS__", page_def["dots"])
            .replace("__DOTS_ALL__", "|".join(d["dots"] for d in DOCS))
            .replace("__NAV__", "\n".join(nav))
            .replace("__LEDE__", part.get("LEDE", ""))
            .replace("__CONTENT__", part.get("CONTENT", ""))
            .replace("__PN__", '      <nav class="pn" aria-label="Pages">' + "".join(pn) + "</nav>" if pn else "")
            .replace("__PAGE_CSS__", part.get("CSS", ""))
            .replace("__PAGE_JS__", part.get("JS", "")))
    return links(page, urls, target)


def document(full):
    """Page complète à partir d'un fragment (le titre, les polices et le style vont dans <head>)."""
    cut = full.index('<div class="wrap">')
    return (
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n"
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">\n"
        + full[:cut] + "</head>\n<body>\n" + full[cut:] + "\n</body>\n</html>\n"
    )


def site_urls(depth):
    """Liens relatifs depuis une page située à `depth` dossiers de la racine du site."""
    up = "../" * depth
    urls = {"HOME": up or "./", "RELEASES": up + "releases/", "ROADMAP": up + "roadmap/", "GS": up + "getting-started/" + DOCS[0]["slug"] + "/"}
    for d in DOCS:
        urls["DOC_" + ukey(d["slug"])] = up + "getting-started/" + d["slug"] + "/"
    return urls


def artifact_urls():
    urls = {"HOME": ARTIFACT_PAGE, "RELEASES": ARTIFACT_RELEASE, "ROADMAP": ARTIFACT_ROADMAP, "GS": ARTIFACT_DOCS[DOCS[0]["slug"]]}
    for d in DOCS:
        urls["DOC_" + ukey(d["slug"])] = ARTIFACT_DOCS.get(d["slug"], "#")
    return urls


def site():
    if SITE.exists():
        shutil.rmtree(SITE)
    pages = {
        "index.html": home(site_urls(0), "", "Dot Box"),
        "releases/index.html": releases(site_urls(1), "", "Releases · Dot Box"),
        "roadmap/index.html": roadmap(site_urls(1), "", "Roadmap · Dot Box"),
    }
    for d in DOCS:
        pages["getting-started/" + d["slug"] + "/index.html"] = doc(d["slug"], site_urls(2), "", d["name"] + " · Dot Box")
    for path, html in pages.items():
        out = SITE / path
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(document(html), encoding="utf-8")
    print(f"{len(pages)} pages -> {SITE}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fragment")
    ap.add_argument("--releases-fragment")
    ap.add_argument("--roadmap-fragment")
    ap.add_argument("--docs-fragments")
    ap.add_argument("--private", action="store_true", help="avec le Journal, CLI Output et En vrac (le site local)")
    args = ap.parse_args()
    if not args.private:
        REMOVED.update(PRIVATE)
        DOCS[:] = [d for d in DOCS if d["slug"] not in PRIVATE]
    if not (args.fragment or args.releases_fragment or args.roadmap_fragment or args.docs_fragments):
        site()
        return
    art = artifact_urls()
    if args.fragment:
        pathlib.Path(args.fragment).write_text(home(art, BLANK, "Dot Box"), encoding="utf-8")
    if args.releases_fragment:
        pathlib.Path(args.releases_fragment).write_text(releases(art, BLANK, "Dot Box Releases"), encoding="utf-8")
    if args.roadmap_fragment:
        pathlib.Path(args.roadmap_fragment).write_text(roadmap(art, BLANK, "Dot Box Roadmap"), encoding="utf-8")
    if args.docs_fragments:
        out = pathlib.Path(args.docs_fragments)
        out.mkdir(parents=True, exist_ok=True)
        for d in DOCS:
            (out / (d["slug"] + ".html")).write_text(doc(d["slug"], art, BLANK, "Dot Box " + d["name"]), encoding="utf-8")


if __name__ == "__main__":
    main()
