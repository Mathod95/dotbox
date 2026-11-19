# Dot Box

Gestionnaire d'apps auto-hébergées: un binaire Rust qui transforme des manifests YAML en fichiers Docker Compose (Traefik, Authelia, câblage entre apps). Page d'accueil au style dot, reprise de Dot Page (Dot Hub, `github.com/Mathod95/xeneon`).

## Règles de travail (Mathod95)

- **Rien n'est poussé sans validation**: chaque changement est d'abord montré (page servie en local sur `localhost`, fichier ou diff), puis poussé sur `main` seulement après l'accord de Mathod95 (décision du 2 novembre 2026).
- **Le repo `Mathod95/xeneon` est en lecture seule**: on y lit la charte et les gabarits, on n'y modifie jamais rien.
- Une page déjà créée n'est jamais modifiée pour en faire une nouvelle version: la nouvelle version vit à côté (v1, v2...).
- **Demander avant de modifier**: aucun fichier, aucune page ni aucun document n'est modifié sans l'accord de Mathod95. Quand il pose une question, on répond sans rien changer.
- **Propositions visibles dans la doc** (décision du 15 novembre 2026): avant toute modification d'une page du site (pas du Journal), le contenu à remplacer est mis dans un encadré bleu « #n · Sera retiré à la validation » (classe `prop-old`) et le nouveau contenu dans un encadré vert « #n · Sera ajouté à la validation » (classe `prop-new`), façon blue/green, le même numéro `data-prop` pour les deux encadrés d'une paire. **Numérotation unique sur tout le site**, jamais réutilisée, même après validation: une nouvelle proposition prend le numéro suivant (dernier numéro attribué: 49). Mathod95 valide numéro par numéro. **Exception**: une modification que Mathod95 dicte lui-même (« retire cette phrase », « mets un lien ») s'applique directement, sans encadrés ; les encadrés servent aux propositions de Claude. Une proposition se montre toujours dans la page, jamais seulement dans le chat ; s'il y a plusieurs variantes, un encadré vert par variante (`18A`, `18B`) sous le même encadré bleu ; styles dans `layout.html`. Après validation de Mathod95: on supprime les `prop-old` et on retire l'encadré des `prop-new` pour n'en garder que le contenu.
- **Rédaction**: en français ; termes Docker et Kubernetes gardés en anglais (container, service, network...) ; pas de tiret cadratin ; deux-points sans espace avant ; **jamais de point-virgule dans le texte des pages** (décision du 15 novembre 2026), une virgule, un point ou une nouvelle phrase à la place ; les points d'une liste commencent par une majuscule et finissent par un point ; les états (***Running***, ***Exited***) en gras italique.

## État du projet (16 novembre 2026)

- **Fait**: le site (accueil, Getting Started, Roadmap, Releases, Journal), publié sur GitHub Pages. Les pages Getting Started sont la **spécification** de tout le reste.
- **Code**: le binaire `dotbox` en Rust (`Cargo.toml`, `src/`), avec `--help`, `--version` et `dotbox init`, qui fait pour l'instant: vérifier le système, le domaine (`--domain`) et les ports, installer `sudo`, préparer l'utilisateur (`--user`), installer Docker, Traefik (derrière un socket proxy, certificats de staging pendant les tests) et Authelia (un compte au nom de l'utilisateur, code à 6 chiffres ou passkey), puis affiche une carte de fin. Une ligne par étape (indicateur, barre de progression, questions façon cliclack sous l'étape), le détail dans `/opt/dotbox/logs/init-<date>.log` (D37), `--verbose` pour le voir en direct, numérotation `[1/7]` avec `install.sh` et `[1/6]` sans (D38), maquette dans la page CLI Output. `dotbox authelia password` relit le mot de passe Authelia gardé pour root, `dotbox authelia verification` affiche le dernier code de vérification d'identité. Ses réponses sont gardées dans `/opt/dotbox/dotbox.yml`. `install.sh` à la racine (root, amd64, wget, checksum, lance `dotbox init`). Testé sur le serveur de test, où Docker 29.9.3, Traefik v3.7.19 et Authelia 4.39.28 tournent, installés par `dotbox init`. Le mot de passe web d'Authelia est rangé sur le PC dans `~/.config/dotbox-test/authelia-pass`.
- **Première release**: `v0.0.1`, taguée le 16 novembre 2026 (Docker, Traefik et Authelia, sans CrowdSec). `.github/workflows/release.yml` publie à chaque tag `dotbox-linux-amd64`, son `.sha256` et `install.sh`: la commande d'installation des pages marche.
- **Deuxième release**: `v0.0.2`, taguée le 16 novembre 2026 (la sortie de `dotbox init`, étape 6 du Journal).
- **Troisième release**: `v0.0.3`, taguée le 17 novembre 2026 (le moteur, `dotbox install` et `remove`, le catalogue avec le core, lu sur GitHub par `dotbox init`, CrowdSec et son bouncer).
- **Fait et testé sur le serveur** (J134 à J143): le moteur (`dotbox install`, `dotbox remove [--purge]`), le core dans `catalog/core/` (plus aucune version dans le binaire), `src/github.rs` qui lit `catalog/` sur `main` à un commit et vérifie chaque fichier, l'étape « Catalog » de `dotbox init` (7 étapes, `[1/8]` avec `install.sh`), CrowdSec et son bouncer. Réinstallation complète du serveur par `install.sh` v0.0.3, en vrais certificats (7.15). Pas encore publié: les droits fixes du dossier du catalogue (`src/github.rs`, J143), pour une v0.0.4.
- **Renovate** (7.14, D65, 19 novembre 2026): app GitHub hébergée par Mend, configurée par `renovate.json` (catalogue, crates Rust et actions GitHub), CI du catalogue `.github/workflows/catalog.yml`, empreintes du bouncer par `catalog-hashes.yml` et `tools/bouncer_hashes.sh`. Expliqué dans la page Renovate de la doc.
- **Pas encore fait**: boucler Renovate (7.14: fusion de la pull request #1, première pull request fusionnée seule, test de `catalog-hashes.yml`, mise à jour du core sur le serveur par `dotbox init`), puis AppSec (10.7), qBittorrent (8.4), Jellyfin (8.6) et Seerr (8.7), le câblage (étape 9), Dot Box (interface web) dans `dotbox init`.
- Le document de travail « Saltbox vs Dot Box : installation » (Claude Docs) a servi à trancher les décisions du 8 et du 9 novembre ; elles sont toutes reportées dans « Décisions produit » ci-dessous.

## Phase en cours: le serveur de test

Mathod95 a un serveur de test, réservé aux essais. Ce n'est jamais son serveur de production (un Saltbox).

- **Accès**: Claude Code tourne sur le PC de Mathod95 (WSL), dans un clone de ce repo, et agit sur le serveur en SSH (`ssh dotbox-test`, clé `~/.ssh/dotbox-test`, alias dans `~/.ssh/config`). Rien n'est installé sur le serveur pour Claude Code. Pour passer root, Claude utilise `sudo` avec le mot de passe de l'utilisateur (Journal, D11), rangé sur le PC dans `~/.config/dotbox-test/sudo-pass` (lisible par Mathod95 seul, jamais dans le repo) et envoyé par l'entrée standard: `python3 ~/.config/dotbox-test/sudo_run.py '<commande>'` (sortie avec le mot de passe et l'IP masqués). Le binaire testé est copié dans `/usr/local/bin/dotbox` (comme le fera `install.sh`), puis lancé avec `sudo dotbox init`.
- **GitHub**: Claude pousse par SSH avec une clé de déploiement limitée à ce repo (`~/.ssh/github-dotbox`, `Host github.com` dans `~/.ssh/config`, push sur `git@github.com:Mathod95/dotbox.git`).
- **Tout est journalisé**: chaque commande lancée sur le serveur (lectures comprises) est ajoutée à la page Journal (`tools/site/docs/journal.html`) avec l'heure et son résultat ; une étape qui écrit sur le serveur y est d'abord proposée, puis lancée seulement après l'accord de Mathod95. **Rien n'est jamais retiré du Journal**: une étape faite reste à sa place, repliée, avec son renvoi vers l'entrée J ; une question tranchée reste repliée avec son renvoi vers la décision D ; les numéros ne bougent jamais. **Étapes numérotées 0, 1, 2..., sous-étapes 1.1, 2.1...** (D46 à D49, 17 novembre 2026: plus de lettres ni de « phases », l'ancienne correspondance est dans D46 et D47). Les étapes se suivent sans groupes, chacune repliée, avec dedans tout ce qui la concerne, replié: ses sous-étapes, ses anciens brouillons, ses décisions D, ses questions Q, son contexte technique N, puis ses entrées J (D48, D51 ; rangement dans `HOME`, `DHOME`, `QHOME` et `NHOME` du script de la page ; une entrée oubliée apparaît dans « non rangées »). Une D, Q ou N sans étape va dans le bloc « Sans étape ». Toute nouvelle entrée J, D, Q ou N doit être ajoutée à ces listes. L'état se lit à la bande de couleur du bord gauche, plus d'étiquettes (D50). L'**étape 0** regroupe notre environnement et notre façon de travailler (clé SSH, accès root de Claude, méthode, doc, organisation du Journal), qui ne sont pas Dot Box. L'étape 8, Apps, a une sous-étape par app (D47). Les explications techniques (sécurité, ce qui est installé, nos choix) vont dans « Contexte technique » et y évoluent.
- **Prudence sur le serveur**: demander avant toute commande destructive (`rm -rf`, `docker system prune`, suppression de volumes, pare-feu) ; ne jamais modifier la config SSH sans accord ; ne pas faire de `apt full-upgrade` ni de redémarrage sans accord.
- **Rien de personnel dans le repo**: le repo est public. Le vrai domaine, l'IP, les e-mails et les secrets restent sur le serveur (fichier `.env` ou `/opt/dotbox/secrets/`, jamais commités). Les fichiers du repo utilisent `example.com`, `admin`, `203.0.113.10`.

### Méthode (décision D13 du 15 novembre 2026)

Pas de fichiers écrits à la main dans `lab/`: chaque morceau du core est codé directement dans `dotbox init`, en suivant la doc officielle de l'outil et ses bonnes pratiques, puis testé avec le binaire sur le serveur. Pour chaque morceau: plan détaillé dans le Journal (une étape, des sous-étapes numérotées), accord de Mathod95, code et tests sur le PC (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, binaire statique `cargo build --release --target x86_64-unknown-linux-musl`), puis tests sur le serveur, chacun noté dans le Journal. Seul Debian 13 est testé en vrai, le chemin Ubuntu est couvert par les tests sur le PC (D14). Le domaine de test est un sous-domaine du domaine personnel de Mathod95 (`*.dotbox` et `dotbox` vers le serveur de test, D16), jamais écrit dans le repo ni le Journal (`<domaine>`). **Pendant les tests, les certificats viennent du staging de Let's Encrypt** (`ca: letsencrypt-staging`, D17): le domaine personnel est partagé avec la production et ses limites de certificats aussi. Passage aux vrais certificats seulement sur décision de Mathod95.

Ordre et critères de réussite:

1. **Système, sudo, utilisateur**: fait (étape 2 du Journal).
2. **Docker** depuis le dépôt officiel, paquets en conflit retirés avec accord, `daemon.json` (logs `local`, `live-restore`), réseau `dotbox`, utilisateur dans le groupe `docker`: fait (étape 3).
3. **Traefik** (étape 4 du Journal, fait): certificats Let's Encrypt en HTTP-01, un par sous-domaine, redirection 80 vers 443 ; ports des containers jamais publiés, tout passe par le réseau `dotbox`. Dernière version stable, fixée (D15). Sans e-mail ni ZeroSSL (D18, D19). À trancher: Q14 (socket Docker).
4. **Authelia** (étape 5 du Journal, fait, sauf le test dans le navigateur): un compte au nom de l'utilisateur, double authentification ; les quatre niveaux d'accès (`two_factor`, `one_factor`, `app`, `public`) testés (Q4).
5. **Apps, avant CrowdSec** (D40 du 16 novembre 2026): d'abord le moteur et les premières apps, toutes au niveau `two_factor` derrière Authelia: le moteur (étape 7), puis les apps, une étape par app (étape 8: 8.1 Prowlarr fait, 8.2 Radarr, 8.3 Sonarr, 8.4 qBittorrent), le câblage et le test du hardlink (étape 9), puis le catalogue lu sur GitHub dès `dotbox init`, avec le core dedans (7.6, 7.12 à 7.15, D64, avant AppSec et avant la réinstallation complète du serveur).
6. **CrowdSec**, avant le serveur média: surveille SSH (journald sur Debian 13) et Traefik, bloque au pare-feu (bouncer iptables, chaînes INPUT et DOCKER-USER) et dans Traefik ; vérifier qu'une IP de test est bien bannie puis débannie (Q3, Q11).
7. **Serveur média** (Jellyfin ou Plex, niveau `app`, sans Authelia devant). Toutes les apps: chacune dans `/opt/<app>` avec `compose.yml` et `config/`, l'utilisateur et ses `PUID`/`PGID`, `/data/torrents` et `/data/media` sur le même système de fichiers, câblées par leurs API.
8. **Critères de réussite**: chaque sous-domaine en HTTPS avec un certificat valide ; Authelia applique le bon niveau ; un film va de Radarr à qBittorrent puis dans `/data/media` en **hardlink** (même inode) ; les apps repartent seules après un redémarrage de Docker.

La CI de release (binaire `dotbox-linux-amd64`, son `.sha256` et `install.sh` publiés à chaque tag) s'écrit au premier tag (Journal, 2.8).

## Arborescence

```
Cargo.toml, src/    le binaire dotbox: main.rs (CLI, clap), init.rs (dotbox init), authelia_cmd.rs (dotbox authelia), config.rs (dotbox.yml), docker.rs, traefik.rs, authelia.rs, crowdsec.rs, github.rs (lire le catalogue sur GitHub), secrets.rs, system.rs, prompt.rs (questions, cliclack), ui.rs (une ligne par étape), log.rs (les logs de init, install, remove), card.rs (la carte de fin), catalog.rs (lire et vérifier un manifest), engine.rs (manifest vers compose.yml), apps.rs (dotbox install et remove), out.rs
catalog/            le catalogue: un manifest par app (catalog/prowlarr.yml), et le core dans catalog/core/ (D41, D64)
tests/expected/     les fichiers que dotbox génère, tels qu'attendus (comparés par les tests)
assets/docker.asc   clé officielle du dépôt Docker, embarquée dans le binaire
install.sh          l'installeur (modèle de rustup): root, amd64, télécharge dotbox, vérifie son SHA-256, lance dotbox init
docs/design/        gabarits homePage et releasePage, anatomy.css et anatomy.js, copiés de xeneon (référence de la charte) ; la subPage se lit dans xeneon
pages/v1/           pages figées: index.html (v1 de la homepage, accueil + releases, vue #releases), cli.html (v1 de la page CLI en subPage)
tools/site/         v2: index.html (homePage), releases.html (releasePage), releases.json (données des versions), roadmap.html et roadmap.json (la roadmap)
tools/site/docs/    documentation Getting Started: layout.html (mise en page commune) et une page par fichier (basics.html ... cli.html)
tools/build_site.py sans option: le site dans site/ (ignoré par Git) ; avec --fragment, --releases-fragment, --roadmap-fragment, --docs-fragments: les aperçus en artefacts
.github/workflows/pages.yml    construit le site et le publie sur GitHub Pages à chaque push sur main
.github/workflows/release.yml  à chaque tag v*: vérifications, binaire statique, release GitHub (dotbox-linux-amd64, .sha256, install.sh)
```

## Pages

- **Charte**: celle de Dot Hub, gabarits dans `docs/design/` (copie de `xeneon/docs/design/` au commit `84edb70`). La règle complète reste `xeneon/docs/design/README.md`. Écarts propres à Dot Box: marque « ● DOT BOX », pas de bouton Companion, slogan « Self-hosted · Rust · Docker Compose », lien de pied de page `Mathod95/dotbox`.
- **homePage v2** (`tools/site/index.html`): les six zones du gabarit dans l'ordre (Top, Hero, Showcase, About, List, Footer). Showcase = maquette interactive de l'interface. List = « Les apps », une carte par app générée depuis le tableau `CATALOG` (pictogramme 7 × 7, nom, rôle en accent, description, sous-domaine et logo GitHub ou « Docs ↗ » en pied). Tous les pictogrammes ont la classe `.pic`, taille unique `--pic: 70px`.
- **releasePage v2** (`tools/site/releases.html`): gabarit releasePage, données dans `tools/site/releases.json`.
- **Roadmap** (`tools/site/roadmap.html`): la todo list des features (décision du 8 novembre 2026), gabarit de la releasePage, données dans `tools/site/roadmap.json`. **Des features à cocher, sans version** (les versions vivent dans Releases), **rangées par étapes dans l'ordre des dépendances** (décision du 15 novembre 2026): Installation, Core, Apps, Interface web, Sauvegarde et migration, Mises à jour et catalogue, Site. Compteur « n faites sur N », légende Fait / En cours / À faire, un point par feature ; chaque étape a son numéro, une phrase et son avancement ; une case par ligne (vide, à moitié, cochée), le domaine en étiquette, des liens vers les entrées du Journal (`J6-J15`, `D8`, `Q3`, `3.1-3.11`) et un lien « Spec ↗ » vers la page Getting Started qui la décrit. Voir « Roadmap » plus bas.
- **Documentation Getting Started** (`tools/site/docs/`): même architecture que docs.saltbox.dev (décision du 2 novembre 2026). Le bouton « Getting Started » de l'en-tête ouvre la page Basics. Barre de navigation à gauche: la section « Getting Started » et ses pages, avec des sous-groupes (champ `group`, ex. « Backup / Restore »), la page en cours en accent avec son sommaire (titres h2 / h3) dessous. Article à droite: fil « Getting Started », titre de la page en points tous les 14 px (police 5 × 7 complète A à Z et « - », F G M V W Y Z ajoutées le 17 novembre 2026 dans `layout.html`), accroche, contenu, puis lien « Suivant / Précédent ». Valeurs de la subPage. La liste des pages et leur ordre sont dans `DOCS` de `build_site.py` ; la barre de gauche est générée à partir de cette liste.
  - Un fichier par page, découpé en `<!-- LEDE -->`, `<!-- CONTENT -->`, `<!-- CSS -->`, `<!-- JS -->`. Texte bilingue: un bloc `data-l="fr"` et un bloc `data-l="en"`, la langue non choisie est masquée ; ou des clés `data-i18n` remplies par le JS de la page, qui s'abonne avec `window.DotBox.onLang()`.
  - Pages, dans l'ordre: Basics, Prerequisites, Installation (regroupe Install Types et Installation de Saltbox), Post-installation, Update, Renovate (ce que fait Renovate sur le catalogue, 7.14), Accessing Apps, Security (les protections, en place et prévues, D62), Paths, groupe Backup / Restore (Backup regroupe Settings, Standard Backup et Streamed Backup ; Restore & Migration regroupe Restore et Migration), Sandbox, groupe Apps (Overview, puis une page par app du catalogue: Prowlarr, Radarr, qui, Sonarr), CLI, Journal, CLI Output (maquettes de la sortie de la CLI, page de travail en français comme le Journal, venue de l'artefact « Récap dotbox init »), En vrac (blocs d'information à ranger dans la doc, D53).
  - Ces pages ne recopient pas Saltbox: elles décrivent le fonctionnement voulu de Dot Box et servent de **spécification** pour le code (décision du 2 novembre 2026). Toute décision produit se reporte dans la page concernée et dans « Décisions produit » ci-dessous.
  - Composants de contenu: `.prose` (h2, h3, p, listes), `pre` (`.p` invite, `.c` commentaire, `.ok`, `.w`), `.table-scroll > table`, `.note`, `.flow` (schéma à l'étapes).
  - **Journal** (`journal.html`): le journal du serveur de test, dernière entrée de la barre de gauche (décision du 15 novembre 2026). Ce n'est pas une page de spécification. Données dans le JS de la page: `DONE` (commandes lancées, `J1`, `J2`...: heure, où, lecture ou écriture, commande, effet, résultat, annulation), `PLAN` (étapes et sous-étapes, `1.1`, `2.3`, `3.11`... ; une sous-étape faite garde sa place avec `status: "fait"` et `ref`), `DECISIONS` (`D1`...), `NOTES` (« Contexte technique », `N1`...), `QUESTIONS` (`Q1`... au numéro fixe `n` ; une question tranchée garde sa place avec `done: "D…"`). Ancres `#j6`, `#d8`, `#q3`, `#n1`, `#e3-3` (étape 3.3), utilisées par la Roadmap. Outils de Claude hors repo: `su_run.py` et `sudo_run.py` (mot de passe envoyé à su ou sudo, masqué dans la sortie avec l'IP). Texte en français seulement, aucune donnée personnelle: IP, domaine et mots de passe masqués, nom d'utilisateur remplacé par `<user>`.
  - **Apps** (D55 du 17 novembre 2026): `apps.html` (Overview: installer et retirer une app, d'où viennent les images, les apps du catalogue) et une page par app **réellement dans le catalogue** (`prowlarr.html`, `radarr.html`, `qui.html`, `sonarr.html`), en français et en anglais, slug = nom de l'app, groupe « Apps » de `DOCS`. Sections d'une page d'app: liens (code source, site, wiki, image), tableau (adresse, accès, image, commande), ce que fait l'app, ce que fait l'installation, où sont les fichiers, les réglages que Dot Box ajoute et pourquoi, après l'installation. Une app sans page est dans la Roadmap (une feature par app). La sous-étape de l'app dans le Journal renvoie à sa page et garde le plan, les preuves et le backlog.
  - **En vrac** (`en-vrac.html`, D53 du 17 novembre 2026): page de travail en français, après CLI Output. Ce n'est pas une spécification: un bloc par information apprise en chemin, en attente de sa place dans la doc. Chaque bloc a un numéro fixe (V1, V2..., ancre `#v1`), « Ira dans » et ses sources ; rangé dans une page par une proposition, il reste dans En vrac marqué « rangé dans ». Titres des blocs en h2 (barre de gauche), intertitres en h4.
  - **CLI** (`cli.html`): `dotbox --help` dans un terminal (chaque commande cliquable montre un exemple de sortie), tableau de toutes les commandes, premiers pas, options globales. Commandes et options dans `GROUPS` et `FLAGS`: la même source pour l'aide, le tableau et les exemples. C'est aussi la spécification de la CLI à coder. Une proposition s'y écrit dans les données (D28): la commande porte `prop` (numéro) et `was` (anciennes valeurs), le tableau affiche l'ancien texte en bleu et le nouveau en vert, le terminal garde l'ancien texte jusqu'à la validation. À la validation, on retire `prop` et `was`. Une commande nouvelle porte `added: true` (cachée du terminal jusqu'à la validation). Une commande qui demande sudo porte `sudo: true`: le tableau écrit `sudo` en vert devant la commande et son exemple, et le terminal l'ajoute à l'exemple.
- **Site**: https://mathod95.github.io/dotbox/ (accueil), `/releases/`, `/roadmap/`, `/getting-started/<page>/`. Construit par `python3 tools/build_site.py` (pages complètes, liens relatifs) et publié par `pages.yml`. Côté GitHub, Settings → Pages → Source doit être « GitHub Actions ».
- **En-tête** de toutes les pages: marque, FR / EN, Getting Started, Roadmap, Releases, GitHub ↗ ; le bouton de la page en cours est allumé (Getting Started sur toutes les pages de documentation).
- **Langue**: français si le navigateur est en français, anglais sinon ; bouton FR / EN, choix gardé dans le navigateur (`dotbox-lang`), `?lang=fr` ou `?lang=en`.
- **Plus d'artefacts** (décision du 15 novembre 2026): les pages se montrent en local, `python3 tools/build_site.py` puis `python3 -m http.server 8000 --directory site`, ouvertes dans le navigateur par Claude. Les adresses ci-dessous ne sont plus mises à jour.
- **Artefacts** (v2, figés): accueil https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE, releases https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd, roadmap https://claude.ai/artifact/UxWsooQhdW7DtxpN4KpRw7, pages de documentation dans `ARTIFACT_DOCS` de `build_site.py` (Basics https://claude.ai/artifact/WPfzXWk2jWXWKbg2EFEMH9, CLI https://claude.ai/artifact/GZz8HJqNBqMgY65VygEe9x, Journal https://claude.ai/artifact/WE96WdBsdMnxaUejtN6BUk...) (la v1 de la page CLI reste sur https://claude.ai/artifact/A3JeW8jtr2kBZ328FCxnvC). Leurs adresses sont dans `build_site.py` pour que leurs liens se répondent. Une page qui change: `python3 tools/build_site.py --fragment <f> --releases-fragment <f> --roadmap-fragment <f> --docs-fragments <dossier>` puis republier **en passant l'adresse de l'artefact** (sinon un nouvel artefact est créé).

## Décisions produit (spécifiées dans Getting Started)

- **Fonctions principales**: récupérer automatiquement films et séries, et les regarder de n'importe où, sur n'importe quel appareil. Dot Box a sa propre interface web (`dotbox.<domaine>`) ; la CLI passe par la même API.
- **Pas de stockage distant** (décision du 8 novembre 2026): les médias restent sur les disques du serveur. Pas d'équivalent du unionFS de Saltbox (rclone, mergerfs, Cloudplow).
- **Installation** (décisions du 15 novembre 2026, Journal D8 et D9): en root (`su -`), puis `wget -qO- https://github.com/Mathod95/dotbox/releases/latest/download/install.sh | bash`. wget et pas curl: wget est installé par défaut sur Debian et Ubuntu, curl ne l'est pas toujours (absent du serveur de test), et Dot Box n'en a pas besoin ensuite. Modèle de rustup: `install.sh` ne fait que vérifier root et amd64, télécharger la **dernière version** de `dotbox` (`dotbox-linux-amd64` et `dotbox-linux-amd64.sha256`), vérifier son SHA-256, l'installer dans `/usr/local/bin` et lancer `dotbox init` (ses autres options lui sont transmises, `--no-init` installe seulement le binaire, pour une migration). Tout le reste est dans le binaire: `dotbox init` vérifie la version du système, installe `sudo` s'il manque, prépare l'utilisateur. Choisir une version précédente: à décider plus tard, dans la Roadmap.
- **Core unique**: pas de type d'installation. `dotbox init` installe Docker (rotation des logs, `live-restore`), le réseau `dotbox`, Traefik, Authelia, CrowdSec et Dot Box (interface web et API sur `dotbox.<domaine>`) et programme une sauvegarde quotidienne locale. Il demande l'utilisateur qui fait tourner les apps (`--user`, sinon une question qui propose le compte connecté avant `su -`): créé s'il n'existe pas (avec son mot de passe), ajouté aux groupes `sudo` et `docker`, sans toucher au mot de passe, au shell ni aux clés SSH d'un utilisateur existant. Docker vient du dépôt officiel, avec les logs `local` et `live-restore` (Journal, Contexte technique N4). Relançable sans risque: il ne refait que ce qui manque.
- **Pas de mise à jour du système ni de redémarrage** (décision du 9 novembre 2026): `dotbox init` n'installe que ce dont il a besoin, sans `apt full-upgrade`, et ne redémarre jamais le serveur. `dotbox doctor` signale les mises à jour en attente, `unattended-upgrades` désactivé et un redémarrage nécessaire.
- **CrowdSec par défaut** (décision du 9 novembre 2026): installé avec le core, il surveille SSH et Traefik et bloque les attaques au pare-feu et dans Traefik, sans toucher à la config SSH. Pas de fail2ban.
- **`dotbox remove`** (D42): arrête l'app, retire son `compose.yml` et son entrée `apps:`, garde `/opt/<app>/config` et `compose.override.yml` ; une réinstallation reprend la configuration ; `--purge` efface `/opt/<app>` et l'image, après une confirmation.
- **Accès des apps** (7.3): une seule règle Authelia `*.<domaine>`, groupe `admins`, `two_factor`, posée par `dotbox init` ; installer une app ne redémarre pas Authelia ; seules les apps `one_factor` auront une règle à elles.
- **Ports publiés** (D44): aucun port web publié, tout passe par Traefik ; seule exception, les ports qui ne sont pas du web, déclarés dans le manifest (`ports:`), comme le port des pairs de qBittorrent (56881 en TCP et UDP, une instance de plus prend le prochain port libre jusqu'à 56901, comme Saltbox).
- **Câblage à part** (D45): `dotbox install` installe et rien de plus, comme les autres outils. Relier les apps (qBittorrent dans Radarr et Sonarr, Radarr et Sonarr dans Prowlarr, les dossiers racine) est une commande à part, lancée quand on le veut (nom à trancher, Q30), qui montre ce qu'elle va faire et demande une confirmation.
- **Plusieurs instances d'une même app**: `dotbox install radarr --as radarr-4k`, avec son dossier `/opt/radarr-4k`, son sous-domaine et son câblage.
- **Certificat** (décision du 9 novembre 2026): HTTP-01 uniquement, comme Saltbox sans clé Cloudflare. Traefik obtient un certificat Let's Encrypt par sous-domaine. **Pas de ZeroSSL, et aucun e-mail demandé ni enregistré** (décisions du 15 novembre 2026, Journal D18 et D19): Let's Encrypt n'en a plus besoin. `ca` dans `dotbox.yml` ne sert qu'à choisir le staging pour les tests (`letsencrypt-staging`). Aucun jeton DNS, pas de wildcard, Dot Box ne crée ni ne modifie aucun enregistrement DNS: l'utilisateur crée `*.<domaine>` vers le serveur, `dotbox init` vérifie qu'il y pointe.
- **Pas de packs** (décision du 8 novembre 2026): `dotbox init` n'installe que le core ; ensuite, l'utilisateur installe les apps qu'il veut, une par une ou plusieurs d'un coup (`dotbox install <app>...`).
- **Serveur média au choix** (décision du 8 novembre 2026): Dot Box est un outil de gestion de serveur de contenus, pas un installeur de Jellyfin. Jellyfin et Plex sont au même niveau, aucun n'est mis en avant dans le site ni dans les exemples ; pas d'Emby dans le catalogue.
- **Vocabulaire**: « core » pour ce qu'installe `dotbox init` (Docker, Traefik, Authelia, CrowdSec, Dot Box), jamais « socle ».
- **Exemples génériques** (décision du 8 novembre 2026): Dot Box est communautaire, les pages n'utilisent jamais le domaine ni le nom de Mathod95. Domaine `example.com` (et `example.org` pour un changement de domaine), utilisateur `admin`, adresse `admin@example.com`, IP `203.0.113.10`. Seuls les liens vers les repos (`Mathod95/dotbox`, `Mathod95/dotbox-sandbox`) gardent le nom.
- **Configuration**: un seul fichier `/opt/dotbox/dotbox.yml` (domaine, `ca`, utilisateur, admin, chemins, sauvegarde, mises à jour, réglages par app) ; secrets dans `/opt/dotbox/secrets/` (root seulement), référencés par `secret:<nom>`.
- **Prérequis**: versions LTS uniquement, Debian 12 ou 13, Ubuntu 24.04 ou 26.04 (décision du 8 novembre 2026), amd64 uniquement (décision du 8 novembre 2026), **uniquement sur un serveur dédié chez un hébergeur**, pas d'installation à la maison (décision du 8 novembre 2026). Pas de vérification LXC ni « Desktop ». Domaine obligatoire, avec `*.<domaine>` (A, et AAAA si IPv6) vers le serveur ; ports 80 (certificats HTTP-01) et 443. Pas de pilotes NVIDIA.
- **Accès**: quatre niveaux par app, `two_factor`, `one_factor`, `app` (connexion propre, pour Jellyfin, Plex, Vaultwarden à cause de leurs apps TV et mobiles), `public`. Sous-domaine et niveau modifiables dans `apps:` de `dotbox.yml`, appliqués par `dotbox sync`.
- **Chemins**: `/opt/<app>` (compose.yml généré, compose.override.yml à l'utilisateur, config/) et `/data` (`torrents/` et `media/` sur le même système de fichiers, hardlinks) ; mêmes chemins dans les containers ; tout en minuscules.
- **SSH**: Dot Box ne modifie jamais la config SSH de lui-même ; `dotbox doctor` signale ce qui manque. Une option, que l'utilisateur choisit d'appliquer, durcira SSH (décision du 15 novembre 2026, contenu à définir, Journal D12 et Q9).
- **Mises à jour**: `dotbox upgrade` (Dot Box seul) et `dotbox update` (apps) séparés ; versions d'image fixées, jamais `latest` ; une app à la fois, attend ***Running*** ; `--rollback` ; automatique seulement si `update.schedule`, avec sauvegarde avant ; pas de Watchtower.
- **Sauvegarde**: `/opt` seulement (pas `/data`), app par app (arrêt court ou instantané btrfs/ZFS), zstd puis chiffrement age (clé dans `secrets/backup.key`), destinations local, rclone, SFTP, rétention `keep`, timer systemd, `--app`, `--to`, `--stream`.
- **Restauration**: `dotbox restore <archive|latest> [--app] [--key]` ; sur un serveur neuf, l'utilisateur pointe d'abord `*.<domaine>` vers la nouvelle IP, installe le binaire avec `install.sh --no-init`, puis restore refait l'init à partir de la config sauvegardée ; changement de domaine par `dotbox config set domain` puis `dotbox sync`.
- **Catalogue dans ce repo** (D41 du 16 novembre 2026, remplace le repo à part du 8 novembre ; D64 du 17 novembre: le core y entre, et `dotbox init` le lit dès le début): un manifest par app dans `catalog/<app>.yml`, et le core dans `catalog/core/` (Traefik, Authelia, CrowdSec, le bouncer et le plugin, avec les empreintes des binaires) ; le binaire garde comment installer, plus quelles versions, et une mise à jour du core ou d'une app ne demande jamais de nouvelle version de `dotbox` ; lisible sur GitHub avec son historique ; `dotbox` lit `catalog/` sur `main` à un commit précis (API GitHub, empreinte de chaque fichier vérifiée), le range dans `/opt/dotbox/catalog/` avec le commit et sa date ; pas d'archive ni de manifests embarqués dans le binaire ; `--catalog <dossier>` pour les tests ; plus tard, un tag `catalog-<date>` pour regrouper des changements ; Renovate (app GitHub hébergée par Mend) propose chaque nouvelle version d'image en pull request, la CI teste, les versions correctives fusionnent seules ; Dot Box rafraîchit le catalogue à chaque `dotbox diff` / `dotbox update` sans changer de binaire ; chaque manifest déclare la version minimale de Dot Box. Pas de git ni de repo sur le serveur ; `dotbox upgrade` met à jour le binaire depuis les releases GitHub.
- **Images Docker** (D53, En vrac V1): l'image officielle du projet quand il en publie une et la maintient (Traefik, Authelia), sinon celle de linuxserver (socket proxy, Prowlarr, Radarr, Sonarr, qBittorrent: Servarr ne publie pas d'image), toujours à une version précise (D15).
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

### Le premier tag: `v0.0.1`

Le premier code est poussé sur `main` le 15 novembre 2026, et `v0.0.1` est tagué le 16 novembre 2026. Avant ce tag:

- Tout ce qui était livré était décrit dans **une seule entrée, `next`**, devenue `v0.0.1` au tag.
- Les allers-retours sur quelque chose qui n'a jamais été publié ne sont **pas** des entrées « Changé » ou « Retiré »: on corrige directement la ligne « Nouveau » concernée.
- Depuis `v0.0.1`, les règles de numérotation ci-dessus s'appliquent. Un tag se pousse seulement avec l'accord de Mathod95, après que `version` de `Cargo.toml` a été mise au même numéro (la CI vérifie qu'ils correspondent).

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
- Composants: `Moteur`, `CLI`, `Interface web`, `Manifest`, `Catalogue`, ou le nom d'une app du catalogue (`Radarr`, `Jellyfin`...).
- La version taguée la plus récente reçoit automatiquement le badge « Dernière version ».

### À chaque commit et à chaque tag

- **Le site n'entre jamais dans Releases** (décision du 15 novembre 2026): accueil, Getting Started, Journal, Roadmap, Releases. Releases suit l'application, pas le site.
- **On n'invente rien**: Releases ne contient que ce qui est réellement livré. Ce qui est prévu est dans la Roadmap, jamais dans Releases (plus de catégorie `planned`).
- **Commit qui change le produit** (moteur, CLI, interface web, catalogue): ajouter la ligne dans `next` (créé à ce moment-là s'il n'existe pas), dans la bonne catégorie, et cocher la feature dans la Roadmap.
- **Tag**: `next` devient la version taguée (on lui donne `version` et `date`). Le `next` suivant n'est créé qu'avec la prochaine ligne réellement livrée.
- Les liens « Changements » (comparaison entre deux tags) et « Télécharger » (release GitHub) en bas de chaque version n'apparaissent que pour les versions `"tagged": true` ; passer le champ à `true` une fois le tag poussé.

### Roadmap

- `tools/site/roadmap.json`: `stages` (les étapes, dans l'ordre des dépendances, avec `name` et `desc`), `domains` (Site, Core, Moteur, Interface web, Accès, Sauvegarde, Mises à jour, Catalogue, CLI) et `features`, dans l'ordre de la liste. **Pas de version cible** (décision du 8 novembre 2026): on coche au fur et à mesure, les versions se lisent dans Releases.
- Une feature: `stage`, `domain`, `status` (`planned` à faire, `progress` en cours, `done` fait), `fr` et `en` au même format que releases.json (`Composant: description`), `doc` (slug de la page Getting Started qui la spécifie, facultatif), `journal` (entrées du Journal qui la suivent: `J16-J31`, `D8`, `Q3`, `3.1-3.11`, facultatif).
- Une feature commencée passe en `progress` ; une feature livrée passe en `done` et sa ligne va dans `new` de `next`. Seulement ce qui est réellement fait.
- Une décision produit qui ajoute une feature: une ligne dans roadmap.json, en plus de la page Getting Started et de « Décisions produit ».

### Outil (à confirmer)

- Proposé: **release-plz**, fait pour les crates Rust. Il lit les Conventional Commits, met à jour `Cargo.toml` et `CHANGELOG.md`, et ouvre une PR de release. Il garde la convention Cargo en `0.x`.
- semantic-release n'est pas retenu: sa première release est `v1.0.0` et il ne prend pas en charge un démarrage en `0.x`.
- Si release-plz est adopté, `CHANGELOG.md` et `tools/site/releases.json` doivent rester d'accord: soit la page lit le changelog, soit un script génère l'un à partir de l'autre. À trancher au moment de la mise en place.
