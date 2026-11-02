#!/usr/bin/env python3
"""Construit les pages de Dot Box à partir des gabarits de tools/site/.

- tools/site/index.html : la homePage (accueil) ;
- tools/site/releases.html : la releasePage, données dans tools/site/releases.json.

Pour l'instant, seulement les aperçus en artefacts claude.ai :
  --fragment FICHIER           l'accueil, sans en-tête HTML, boutons vers l'artefact Releases
  --releases-fragment FICHIER  les releases, sans en-tête HTML, marque vers l'artefact Accueil

Le site GitHub Pages (liens relatifs, pages complètes) viendra avec sa mise en place.
"""
import argparse
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SITE_SRC = ROOT / "tools" / "site"
HOME_TEMPLATE = SITE_SRC / "index.html"
RELEASES_TEMPLATE = SITE_SRC / "releases.html"
RELEASES_DATA = SITE_SRC / "releases.json"
# artefacts claude.ai (v2) : l'accueil et les releases se répondent par leurs boutons
ARTIFACT_PAGE = "https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE"
ARTIFACT_RELEASE = "https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd"
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fragment")
    ap.add_argument("--releases-fragment")
    args = ap.parse_args()
    if not (args.fragment or args.releases_fragment):
        ap.error("le site GitHub Pages n'est pas encore en place : utiliser --fragment ou --releases-fragment")
    art = {"HOME": ARTIFACT_PAGE, "RELEASES": ARTIFACT_RELEASE}
    if args.fragment:
        pathlib.Path(args.fragment).write_text(home(art, BLANK, "Dot Box"), encoding="utf-8")
    if args.releases_fragment:
        pathlib.Path(args.releases_fragment).write_text(releases(art, BLANK, "Dot Box Releases"), encoding="utf-8")


if __name__ == "__main__":
    main()
