#!/usr/bin/env python3
"""Construit les pages de Dot Box à partir des gabarits de tools/site/.

- tools/site/index.html : la homePage (accueil) ;
- tools/site/releases.html : la releasePage, données dans tools/site/releases.json ;
- tools/site/docs/ : la documentation (architecture de docs.saltbox.dev) :
  layout.html (en-tête, barre de navigation à gauche, article, pied de page)
  et une page par fichier (basics.html, cli.html), découpée en sections
  <!-- LEDE -->, <!-- CONTENT -->, <!-- CSS --> et <!-- JS -->.

Sans option : le site GitHub Pages dans site/ (pages complètes, liens relatifs) :
  site/index.html, site/releases/, site/getting-started/basics/, site/getting-started/cli/
Publié par .github/workflows/pages.yml à chaque push sur main.

Aperçus en artefacts claude.ai (pages sans en-tête HTML, liens vers les autres artefacts) :
  --fragment FICHIER           l'accueil
  --releases-fragment FICHIER  les releases
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
DOCS_SRC = SITE_SRC / "docs"
DOCS_LAYOUT = DOCS_SRC / "layout.html"

# la documentation : une section (Getting Started), ses pages dans l'ordre de la barre de gauche
DOCS_SECTION = "Getting Started"
DOCS = [
    {"slug": "basics", "name": "Basics", "dots": "BASICS"},
    {"slug": "cli", "name": "CLI", "dots": "CLI"},
]

# artefacts claude.ai : les pages se répondent par leurs boutons
ARTIFACT_PAGE = "https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE"
ARTIFACT_RELEASE = "https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd"
ARTIFACT_DOCS = {
    "basics": "https://claude.ai/artifact/WPfzXWk2jWXWKbg2EFEMH9",
    "cli": "https://claude.ai/artifact/GZz8HJqNBqMgY65VygEe9x",
}
BLANK = ' target="_blank" rel="noopener"'


def links(page, urls, target):
    """Liens entre les pages : adresses des artefacts, ou liens relatifs sur le site."""
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


def sections(text):
    """Découpe un fichier de page en {LEDE, CONTENT, CSS, JS}."""
    parts = re.split(r"<!-- (LEDE|CONTENT|CSS|JS) -->\n", text)
    return {parts[i]: parts[i + 1].strip("\n") for i in range(1, len(parts), 2)}


def doc(slug, urls, target, title):
    """Une page de documentation : la mise en page commune, la barre de gauche et le contenu de la page."""
    i = [d["slug"] for d in DOCS].index(slug)
    page_def = DOCS[i]
    part = sections((DOCS_SRC / (slug + ".html")).read_text(encoding="utf-8"))
    nav = ['      <h2>' + DOCS_SECTION + '</h2>', '      <ul class="nav">']
    for d in DOCS:
        key = "__DOC_" + d["slug"].upper() + "__"
        if d["slug"] == slug:
            nav.append('        <li><a href="' + key + '" aria-current="page">' + d["name"] + '</a><ul class="toc" id="pageToc"></ul></li>')
        else:
            nav.append('        <li><a href="' + key + '"' + "__DOC_" + d["slug"].upper() + "_TARGET__" + ">" + d["name"] + "</a></li>")
    nav.append("      </ul>")
    pn = []
    if i > 0:
        p = DOCS[i - 1]
        pn.append('<a class="prev" href="__DOC_' + p["slug"].upper() + '__"__DOC_' + p["slug"].upper() + '_TARGET__><small><span data-l="fr">← Précédent</span><span data-l="en">← Previous</span></small><b>' + p["name"] + "</b></a>")
    if i < len(DOCS) - 1:
        n = DOCS[i + 1]
        pn.append('<a class="next" href="__DOC_' + n["slug"].upper() + '__"__DOC_' + n["slug"].upper() + '_TARGET__><small><span data-l="fr">Suivant →</span><span data-l="en">Next →</span></small><b>' + n["name"] + "</b></a>")
    page = (DOCS_LAYOUT.read_text(encoding="utf-8")
            .replace("__TITLE__", title)
            .replace("__H1__", page_def["name"])
            .replace("__DOTS__", page_def["dots"])
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
    urls = {"HOME": up or "./", "RELEASES": up + "releases/", "GS": up + "getting-started/" + DOCS[0]["slug"] + "/"}
    for d in DOCS:
        urls["DOC_" + d["slug"].upper()] = up + "getting-started/" + d["slug"] + "/"
    return urls


def artifact_urls():
    urls = {"HOME": ARTIFACT_PAGE, "RELEASES": ARTIFACT_RELEASE, "GS": ARTIFACT_DOCS[DOCS[0]["slug"]]}
    for d in DOCS:
        urls["DOC_" + d["slug"].upper()] = ARTIFACT_DOCS[d["slug"]]
    return urls


def site():
    if SITE.exists():
        shutil.rmtree(SITE)
    pages = {
        "index.html": home(site_urls(0), "", "Dot Box"),
        "releases/index.html": releases(site_urls(1), "", "Releases · Dot Box"),
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
    ap.add_argument("--docs-fragments")
    args = ap.parse_args()
    if not (args.fragment or args.releases_fragment or args.docs_fragments):
        site()
        return
    art = artifact_urls()
    if args.fragment:
        pathlib.Path(args.fragment).write_text(home(art, BLANK, "Dot Box"), encoding="utf-8")
    if args.releases_fragment:
        pathlib.Path(args.releases_fragment).write_text(releases(art, BLANK, "Dot Box Releases"), encoding="utf-8")
    if args.docs_fragments:
        out = pathlib.Path(args.docs_fragments)
        out.mkdir(parents=True, exist_ok=True)
        for d in DOCS:
            (out / (d["slug"] + ".html")).write_text(doc(d["slug"], art, BLANK, "Dot Box " + d["name"]), encoding="utf-8")


if __name__ == "__main__":
    main()
