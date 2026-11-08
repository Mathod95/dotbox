# Dot Box

Gestionnaire d'apps auto-hébergées: un binaire Rust qui transforme des manifests YAML en fichiers Docker Compose (Traefik, Authelia, câblage entre apps). Page d'accueil au style dot, reprise de Dot Page (Dot Hub, `github.com/Mathod95/xeneon`).

## Règles de travail (Mathod95)

- **Rien n'est poussé sans validation**: chaque changement est d'abord montré (artefact, fichier ou diff), puis poussé sur `main` seulement après l'accord de Mathod95 (décision du 2 novembre 2026).
- **Le repo `Mathod95/xeneon` est en lecture seule**: on y lit la charte et les gabarits, on n'y modifie jamais rien.
- Une page déjà créée n'est jamais modifiée pour en faire une nouvelle version: la nouvelle version vit à côté (v1, v2...).

## Arborescence

```
docs/design/        gabarits homePage et releasePage, anatomy.css et anatomy.js, copiés de xeneon (référence de la charte) ; la subPage se lit dans xeneon
pages/v1/           pages figées : index.html (v1 de la homepage, accueil + releases, vue #releases), cli.html (v1 de la page CLI en subPage)
tools/site/         v2 : index.html (homePage), releases.html (releasePage), releases.json (données des versions), roadmap.html et roadmap.json (la roadmap)
tools/site/docs/    documentation Getting Started : layout.html (mise en page commune) et une page par fichier (basics.html ... cli.html)
tools/build_site.py sans option : le site dans site/ (ignoré par Git) ; avec --fragment, --releases-fragment, --roadmap-fragment, --docs-fragments : les aperçus en artefacts
.github/workflows/pages.yml  construit le site et le publie sur GitHub Pages à chaque push sur main
```

## Pages

- **Charte**: celle de Dot Hub, gabarits dans `docs/design/` (copie de `xeneon/docs/design/` au commit `84edb70`). La règle complète reste `xeneon/docs/design/README.md`. Écarts propres à Dot Box: marque « ● DOT BOX », pas de bouton Companion, slogan « Self-hosted · Rust · Docker Compose », lien de pied de page `Mathod95/dotbox`.
- **homePage v2** (`tools/site/index.html`): les six zones du gabarit dans l'ordre (Top, Hero, Showcase, About, List, Footer). Showcase = maquette interactive de l'interface. List = « Les apps », une carte par app générée depuis le tableau `CATALOG` (pictogramme 7 × 7, nom, rôle en accent, description, sous-domaine et logo GitHub ou « Docs ↗ » en pied). Tous les pictogrammes ont la classe `.pic`, taille unique `--pic: 70px`.
- **releasePage v2** (`tools/site/releases.html`): gabarit releasePage, données dans `tools/site/releases.json`.
- **Roadmap** (`tools/site/roadmap.html`): la todo list des features (décision du 8 novembre 2026), gabarit de la releasePage, données dans `tools/site/roadmap.json`. **Une liste simple à cocher, sans version** (les versions vivent dans Releases): compteur « n faites sur N », légende Fait / En cours / À faire, un point par feature, une case par ligne (vide, à moitié, cochée), le domaine en étiquette et un lien « Spec ↗ » vers la page Getting Started qui la décrit. Voir « Roadmap » plus bas.
- **Documentation Getting Started** (`tools/site/docs/`): même architecture que docs.saltbox.dev (décision du 2 novembre 2026). Le bouton « Getting Started » de l'en-tête ouvre la page Basics. Barre de navigation à gauche: la section « Getting Started » et ses pages, avec des sous-groupes (champ `group`, ex. « Backup / Restore »), la page en cours en accent avec son sommaire (titres h2 / h3) dessous. Article à droite: fil « Getting Started », titre de la page en points tous les 14 px (police 5 × 7 complète A à Z et « - » dans `layout.html`), accroche, contenu, puis lien « Suivant / Précédent ». Valeurs de la subPage. La liste des pages et leur ordre sont dans `DOCS` de `build_site.py` ; la barre de gauche est générée à partir de cette liste.
  - Un fichier par page, découpé en `<!-- LEDE -->`, `<!-- CONTENT -->`, `<!-- CSS -->`, `<!-- JS -->`. Texte bilingue: un bloc `data-l="fr"` et un bloc `data-l="en"`, la langue non choisie est masquée ; ou des clés `data-i18n` remplies par le JS de la page, qui s'abonne avec `window.DotBox.onLang()`.
  - Pages, dans l'ordre: Basics, Prerequisites, Installation (regroupe Install Types et Installation de Saltbox), Post-installation, Update, Accessing Apps, Paths, groupe Backup / Restore (Backup regroupe Settings, Standard Backup et Streamed Backup ; Restore & Migration regroupe Restore et Migration), Sandbox, CLI.
  - Ces pages ne recopient pas Saltbox: elles décrivent le fonctionnement voulu de Dot Box et servent de **spécification** pour le code (décision du 2 novembre 2026). Toute décision produit se reporte dans la page concernée et dans « Décisions produit » ci-dessous.
  - Composants de contenu: `.prose` (h2, h3, p, listes), `pre` (`.p` invite, `.c` commentaire, `.ok`, `.w`), `.table-scroll > table`, `.note`, `.flow` (schéma en étapes).
  - **CLI** (`cli.html`): `dotbox --help` dans un terminal (chaque commande cliquable montre un exemple de sortie), tableau de toutes les commandes, premiers pas, options globales. Commandes et options dans `GROUPS` et `FLAGS` : la même source pour l'aide, le tableau et les exemples. C'est aussi la spécification de la CLI à coder.
- **Site**: https://mathod95.github.io/dotbox/ (accueil), `/releases/`, `/roadmap/`, `/getting-started/<page>/`. Construit par `python3 tools/build_site.py` (pages complètes, liens relatifs) et publié par `pages.yml`. Côté GitHub, Settings → Pages → Source doit être « GitHub Actions ».
- **En-tête** de toutes les pages: marque, FR / EN, Getting Started, Roadmap, Releases, GitHub ↗ ; le bouton de la page en cours est allumé (Getting Started sur toutes les pages de documentation).
- **Langue**: français si le navigateur est en français, anglais sinon ; bouton FR / EN, choix gardé dans le navigateur (`dotbox-lang`), `?lang=fr` ou `?lang=en`.
- **Artefacts** (v2): accueil https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE, releases https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd, roadmap https://claude.ai/artifact/UxWsooQhdW7DtxpN4KpRw7, pages de documentation dans `ARTIFACT_DOCS` de `build_site.py` (Basics https://claude.ai/artifact/WPfzXWk2jWXWKbg2EFEMH9, CLI https://claude.ai/artifact/GZz8HJqNBqMgY65VygEe9x...) (la v1 de la page CLI reste sur https://claude.ai/artifact/A3JeW8jtr2kBZ328FCxnvC). Leurs adresses sont dans `build_site.py` pour que leurs liens se répondent. Une page qui change: `python3 tools/build_site.py --fragment <f> --releases-fragment <f> --roadmap-fragment <f> --docs-fragments <dossier>` puis republier **en passant l'adresse de l'artefact** (sinon un nouvel artefact est créé).

## Décisions produit (spécifiées dans Getting Started)

- **Fonctions principales**: récupérer automatiquement films et séries, et les regarder de n'importe où, sur n'importe quel appareil. Dot Box a sa propre interface web (`dotbox.<domaine>`) ; la CLI passe par la même API.
- **Pas de stockage distant** (décision du 8 novembre 2026): les médias restent sur les disques du serveur. Pas d'équivalent du unionFS de Saltbox (rclone, mergerfs, Cloudplow).
- **Core unique**: pas de type d'installation. `dotbox init` installe Docker, le réseau `dotbox`, Traefik, Authelia et Dot Box (interface web et API sur `dotbox.<domaine>`), crée l'utilisateur `dotbox` (UID et GID 1000) et programme une sauvegarde quotidienne locale. Relançable sans risque. Pas de redémarrage du serveur.
- **Plusieurs instances d'une même app**: `dotbox install radarr --as radarr-4k`, avec son dossier `/opt/radarr-4k`, son sous-domaine et son câblage.
- **Certificat**: wildcard en DNS-01, Let's Encrypt par défaut ou ZeroSSL (`ca:` dans `dotbox.yml`).
- **Pas de packs** (décision du 8 novembre 2026): `dotbox init` n'installe que le core ; ensuite, l'utilisateur installe les apps qu'il veut, une par une ou plusieurs d'un coup (`dotbox install <app>...`).
- **Serveur média au choix** (décision du 8 novembre 2026): Dot Box est un outil de gestion de serveur de contenus, pas un installeur de Jellyfin. Jellyfin et Plex sont au même niveau, aucun n'est mis en avant dans le site ni dans les exemples ; pas d'Emby dans le catalogue.
- **Vocabulaire**: « core » pour ce qu'installe `dotbox init` (Docker, Traefik, Authelia, Dot Box), jamais « socle ».
- **Exemples génériques** (décision du 8 novembre 2026): Dot Box est communautaire, les pages n'utilisent jamais le domaine ni le nom de Mathod95. Domaine `example.com` (et `example.org` pour un changement de domaine), utilisateur `admin`, adresse `admin@example.com`, IP `203.0.113.10`. Seuls les liens vers les repos (`Mathod95/dotbox`, `Mathod95/dotbox-sandbox`) gardent le nom.
- **Configuration**: un seul fichier `/opt/dotbox/dotbox.yml` (domaine, e-mail, DNS, admin, chemins, sauvegarde, mises à jour, réglages par app) ; secrets dans `/opt/dotbox/secrets/` (root seulement), référencés par `secret:<nom>`.
- **Prérequis**: Debian 12/13 ou Ubuntu 24.04, amd64 ou arm64, **uniquement sur un serveur dédié chez un hébergeur**, pas d'installation à la maison (décision du 8 novembre 2026). Domaine obligatoire ; certificat wildcard en DNS-01 (Cloudflare, OVH, Scaleway, Gandi...) ; ports 80 et 443.
- **Accès**: quatre niveaux par app, `two_factor`, `one_factor`, `app` (connexion propre, pour Jellyfin, Plex, Vaultwarden à cause de leurs apps TV et mobiles), `public`. Sous-domaine et niveau modifiables dans `apps:` de `dotbox.yml`, appliqués par `dotbox sync`.
- **Chemins**: `/opt/<app>` (compose.yml généré, compose.override.yml à l'utilisateur, config/) et `/data` (`torrents/` et `media/` sur le même système de fichiers, hardlinks) ; mêmes chemins dans les containers ; tout en minuscules.
- **SSH**: Dot Box ne modifie jamais la config SSH ; `dotbox doctor` signale ce qui manque.
- **Mises à jour**: `dotbox upgrade` (Dot Box seul) et `dotbox update` (apps) séparés ; versions d'image fixées, jamais `latest` ; une app à la fois, attend ***Running*** ; `--rollback` ; automatique seulement si `update.schedule`, avec sauvegarde avant ; pas de Watchtower.
- **Sauvegarde**: `/opt` seulement (pas `/data`), app par app (arrêt court ou instantané btrfs/ZFS), zstd puis chiffrement age (clé dans `secrets/backup.key`), destinations local, rclone, SFTP, rétention `keep`, timer systemd, `--app`, `--to`, `--stream`.
- **Restauration**: `dotbox restore <archive|latest> [--app] [--key]` ; sur un serveur neuf, restore refait l'init à partir de la config sauvegardée ; changement de domaine par `dotbox config set domain` puis `dotbox sync`.
- **Sources d'apps**: catalogue officiel, Sandbox communautaire (`Mathod95/dotbox-sandbox`, `dotbox install sandbox/<app>`), apps locales (`dotbox add` → `/opt/dotbox/apps/`), même format de manifest pour les trois.

## Releases

### Numérotation

- **Semantic Versioning strict**: `vMAJEUR.MINEUR.CORRECTIF`, tags Git préfixés par `v` (`v0.1.0`). Le `v` ne fait partie que du tag, pas de la version dans `Cargo.toml`.
- Les versions sont déduites des **Conventional Commits**:

| Commit | Effet |
|---|---|
| `fix:` | correctif (`0.1.0` → `0.1.1`) |
| `feat:` | mineur (`0.1.1` → `0.2.0`) |
| `feat!:` ou `BREAKING CHANGE:` | majeur ; tant qu'on est en `0.x`, mineur (convention Cargo) |
| `docs:`, `chore:`, `ci:`, `refactor:`, `test:` | pas de nouvelle version |

- Passage en `1.0.0`: décision manuelle de Mathod95, jamais automatique.

### Avant le premier push: une seule `v0.0.1`

**Rien n'est encore poussé ni tagué** (décision du 28 octobre 2026). Tant que c'est le cas:

- Tout ce qui existe est décrit dans **une seule version, `v0.0.1`**. On l'enrichit, on n'en crée pas d'autre.
- Les allers-retours sur quelque chose qui n'a jamais été publié ne sont **pas** des entrées « Changé » ou « Retiré »: on corrige directement la ligne « Nouveau » concernée.
- `v0.0.1` devient une vraie version au premier tag poussé. Ensuite seulement, les règles de numérotation ci-dessus s'appliquent.

### Où vivent les releases

- Page **Releases** (`tools/site/releases.html`, bouton « Releases » de l'en-tête): versions au centre, de la plus récente à la plus ancienne, liste des versions à droite qui suit le défilement. Même gabarit que Dot Release (Dot Hub).
- Source des données: `tools/site/releases.json`, intégré dans la page par `build_site.py`.

### Format d'une entrée

```json
{
  "version": "v0.1.0",
  "date": "2026-11-04",
  "tagged": true,
  "title": { "fr": "Serveur de test", "en": "Test server" },
  "new": [
    { "fr": "Moteur: génère `compose.yml` à partir d'un manifest", "en": "Engine: generates `compose.yml` from a manifest" }
  ],
  "changed": [],
  "fixed": [],
  "removed": []
}
```

- `version`: le tag, ou `"next"` pour ce qui est réellement livré sur `main` sans être tagué (affiché « Sur main · pas encore publiée », badge « Prochaine version »). **Pas de `next` vide ni de `next` qui ne contient que du prévu** (décision du 8 novembre 2026).
- `date`: `AAAA-MM-JJ`, date du tag. Absente pour `next`.
- `tagged`: `true` seulement quand le tag est poussé sur GitHub. Il affiche les liens « Changements » et « Télécharger » ; absent pour `next`.
- Catégories, dans cet ordre: `new` (Nouveau), `changed` (Changé), `fixed` (Corrigé), `removed` (Retiré). Une catégorie vide n'est pas affichée.
- Chaque ligne existe **en français et en anglais**. Format: `Composant: description`, sans espace avant les deux-points, noms de fichiers et commandes entre backticks (rendus en `<code>`).
- Composants: `Moteur`, `CLI`, `Interface web`, `Manifest`, `Accueil`, `Getting Started`, `Releases`, `Roadmap`, ou le nom d'une app du catalogue (`Radarr`, `Jellyfin`...).
- La version taguée la plus récente reçoit automatiquement le badge « Dernière version ».

### À chaque commit et à chaque tag

- **On n'invente rien**: Releases ne contient que ce qui est réellement livré. Ce qui est prévu est dans la Roadmap, jamais dans Releases (plus de catégorie `planned`).
- **Commit qui change le produit** (moteur, CLI, interface web, catalogue, homepage): ajouter la ligne dans `next` (créé à ce moment-là s'il n'existe pas), dans la bonne catégorie, et cocher la feature dans la Roadmap.
- **Tag**: `next` devient la version taguée (on lui donne `version` et `date`). Le `next` suivant n'est créé qu'avec la prochaine ligne réellement livrée.
- Les liens « Changements » (comparaison entre deux tags) et « Télécharger » (release GitHub) en bas de chaque version n'apparaissent que pour les versions `"tagged": true` ; passer le champ à `true` une fois le tag poussé.

### Roadmap

- `tools/site/roadmap.json`: `domains` (Site, Core, Moteur, Interface web, Accès, Sauvegarde, Mises à jour, Catalogue, CLI) et `features`, dans l'ordre de la liste. **Pas de version cible** (décision du 8 novembre 2026): on coche au fur et à mesure, les versions se lisent dans Releases.
- Une feature: `domain`, `status` (`planned` à faire, `progress` en cours, `done` fait), `fr` et `en` au même format que releases.json (`Composant: description`), `doc` (slug de la page Getting Started qui la spécifie, facultatif).
- Une feature commencée passe en `progress` ; une feature livrée passe en `done` et sa ligne va dans `new` de `next` (ou de `v0.0.1` tant que rien n'est tagué). Seulement ce qui est réellement fait.
- Une décision produit qui ajoute une feature: une ligne dans roadmap.json, en plus de la page Getting Started et de « Décisions produit ».

### Outil (à confirmer)

- Proposé: **release-plz**, fait pour les crates Rust. Il lit les Conventional Commits, met à jour `Cargo.toml` et `CHANGELOG.md`, et ouvre une PR de release. Il garde la convention Cargo en `0.x`.
- semantic-release n'est pas retenu: sa première release est `v1.0.0` et il ne prend pas en charge un démarrage en `0.x`.
- Si release-plz est adopté, `CHANGELOG.md` et `tools/site/releases.json` doivent rester d'accord: soit la page lit le changelog, soit un script génère l'un à partir de l'autre. À trancher au moment de la mise en place.
