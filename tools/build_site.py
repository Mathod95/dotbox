#!/usr/bin/env python3
"""Construit les pages de Dot Box à partir des gabarits de tools/site/.

- tools/site/index.html : la homePage (accueil) ;
- tools/site/releases.html : la releasePage, données dans tools/site/releases.json ;
- tools/site/cli.html : la subPage de la CLI.

Sans option : le site GitHub Pages dans site/ (pages complètes, liens relatifs) :
  site/index.html, site/releases/index.html, site/cli/index.html
Publié par .github/workflows/pages.yml à chaque push sur main.

Aperçus en artefacts claude.ai (pages sans en-tête HTML, liens vers les autres artefacts) :
  --fragment FICHIER           l'accueil
  --releases-fragment FICHIER  les releases
  --cli-fragment FICHIER       la page de la CLI
"""
import argparse
import json
import pathlib
import shutil

ROOT = pathlib.Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
SITE_SRC = ROOT / "tools" / "site"
HOME_TEMPLATE = SITE_SRC / "index.html"
RELEASES_TEMPLATE = SITE_SRC / "releases.html"
RELEASES_DATA = SITE_SRC / "releases.json"
CLI_TEMPLATE = SITE_SRC / "cli.html"
# artefacts claude.ai (v2) : l'accueil et les releases se répondent par leurs boutons
ARTIFACT_PAGE = "https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE"
ARTIFACT_RELEASE = "https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd"
ARTIFACT_CLI = "https://claude.ai/artifact/A3JeW8jtr2kBZ328FCxnvC"
BLANK = ' target="_blank" rel="noopener"'


def links(page, urls, target):
    """Boutons qui relient les pages : adresses des artefacts (ou liens relatifs sur le site)."""
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


def cli(urls, target, title):
    page = CLI_TEMPLATE.read_text(encoding="utf-8").replace("__TITLE__", title)
    return links(page, urls, target)


def document(full):
    """Page complète à partir d'un fragment (le titre, les polices et le style vont dans <head>)."""
    cut = full.index('<div class="wrap">')
    return (
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n"
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">\n"
        + full[:cut] + "</head>\n<body>\n" + full[cut:] + "\n</body>\n</html>\n"
    )


def site():
    if SITE.exists():
        shutil.rmtree(SITE)
    pages = {
        "index.html": home({"RELEASES": "releases/", "CLI": "cli/"}, "", "Dot Box"),
        "releases/index.html": releases({"HOME": "../", "CLI": "../cli/"}, "", "Releases · Dot Box"),
        "cli/index.html": cli({"HOME": "../", "RELEASES": "../releases/"}, "", "CLI · Dot Box"),
    }
    for path, html in pages.items():
        out = SITE / path
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(document(html), encoding="utf-8")
    print(f"{len(pages)} pages -> {SITE}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fragment")
    ap.add_argument("--releases-fragment")
    ap.add_argument("--cli-fragment")
    args = ap.parse_args()
    if not (args.fragment or args.releases_fragment or args.cli_fragment):
        site()
        return
    art = {"HOME": ARTIFACT_PAGE, "RELEASES": ARTIFACT_RELEASE, "CLI": ARTIFACT_CLI}
    if args.fragment:
        pathlib.Path(args.fragment).write_text(home(art, BLANK, "Dot Box"), encoding="utf-8")
    if args.releases_fragment:
        pathlib.Path(args.releases_fragment).write_text(releases(art, BLANK, "Dot Box Releases"), encoding="utf-8")
    if args.cli_fragment:
        pathlib.Path(args.cli_fragment).write_text(cli(art, BLANK, "Dot Box CLI"), encoding="utf-8")


if __name__ == "__main__":
    main()
