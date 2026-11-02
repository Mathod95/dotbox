# Dot Box

Gestionnaire d'apps auto-hébergées: un binaire Rust qui transforme des manifests YAML en fichiers Docker Compose (Traefik, Authelia, câblage entre apps). Page d'accueil au style dot, reprise de Dot Page (Dot Hub, `github.com/Mathod95/xeneon`).

## Règles de travail (Mathod95)

- **Rien n'est poussé sans validation**: chaque changement est d'abord montré (artefact, fichier ou diff), puis poussé sur `main` seulement après l'accord de Mathod95 (décision du 2 novembre 2026).
- **Le repo `Mathod95/xeneon` est en lecture seule**: on y lit la charte et les gabarits, on n'y modifie jamais rien.
- Une page déjà créée n'est jamais modifiée pour en faire une nouvelle version: la nouvelle version vit à côté (v1, v2...).

## Arborescence

```
docs/design/        gabarits homePage et releasePage, anatomy.css et anatomy.js, copiés de xeneon (référence de la charte)
pages/v1/           v1 de la homepage (accueil + releases dans une seule page, vue #releases), figée
tools/site/         v2 : index.html (homePage), releases.html (releasePage), releases.json (données des versions)
tools/build_site.py construit les aperçus en artefacts (--fragment, --releases-fragment) ; le site GitHub Pages viendra ensuite
```

## Pages

- **Charte**: celle de Dot Hub, gabarits dans `docs/design/` (copie de `xeneon/docs/design/` au commit `84edb70`). La règle complète reste `xeneon/docs/design/README.md`. Écarts propres à Dot Box: marque « ● DOT BOX », pas de bouton Companion, slogan « Self-hosted · Rust · Docker Compose », lien de pied de page `Mathod95/dotbox`.
- **homePage v2** (`tools/site/index.html`): les six zones du gabarit dans l'ordre (Top, Hero, Showcase, About, List, Footer). Showcase = maquette interactive de l'interface. List = « Les apps », une carte par app générée depuis le tableau `CATALOG` (pictogramme 7 × 7, nom, rôle en accent, description, sous-domaine et logo GitHub ou « Docs ↗ » en pied). Tous les pictogrammes ont la classe `.pic`, taille unique `--pic: 70px`.
- **releasePage v2** (`tools/site/releases.html`): gabarit releasePage, données dans `tools/site/releases.json`.
- **Langue**: français si le navigateur est en français, anglais sinon ; bouton FR / EN, choix gardé dans le navigateur (`dotbox-lang`), `?lang=fr` ou `?lang=en`.
- **Artefacts** (v2): accueil https://claude.ai/artifact/E5ByDHCb49shftFLzDTPbE, releases https://claude.ai/artifact/XpnuaqWpy7684cLAWuNEAd. Leurs adresses sont dans `build_site.py` pour que leurs boutons se répondent. Une page qui change: `python3 tools/build_site.py --fragment <f> --releases-fragment <f>` puis republier.

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

- `version`: le tag, ou `"next"` pour ce qui est sur `main` sans être tagué (affiché « Sur main · pas encore publiée », badge « Prochaine version »).
- `date`: `AAAA-MM-JJ`, date du tag. Absente pour `next`.
- `tagged`: `true` seulement quand le tag est poussé sur GitHub. Il affiche les liens « Changements » et « Télécharger » ; absent pour `next`.
- Catégories, dans cet ordre: `planned` (Prévu, **uniquement dans `next`**, pour ce qui n'est pas encore codé), `new` (Nouveau), `changed` (Changé), `fixed` (Corrigé), `removed` (Retiré). Une catégorie vide n'est pas affichée.
- Chaque ligne existe **en français et en anglais**. Format: `Composant: description`, sans espace avant les deux-points, noms de fichiers et commandes entre backticks (rendus en `<code>`).
- Composants: `Moteur`, `CLI`, `Interface web`, `Manifest`, `Accueil`, `Releases`, ou le nom d'une app du catalogue (`Radarr`, `Jellyfin`...).
- La version taguée la plus récente reçoit automatiquement le badge « Dernière version ».

### À chaque commit et à chaque tag

- **Commit qui change le produit** (moteur, CLI, interface web, catalogue, homepage): ajouter la ligne dans `next`, dans la bonne catégorie. Une ligne `planned` qui vient d'être codée passe dans `new`.
- **Tag**: `next` devient la version taguée (on lui donne `version` et `date`, les lignes `planned` restantes repartent dans un nouveau `next`), puis on crée le nouveau `next`.
- Les liens « Changements » (comparaison entre deux tags) et « Télécharger » (release GitHub) en bas de chaque version n'apparaissent que pour les versions `"tagged": true` ; passer le champ à `true` une fois le tag poussé.

### Outil (à confirmer)

- Proposé: **release-plz**, fait pour les crates Rust. Il lit les Conventional Commits, met à jour `Cargo.toml` et `CHANGELOG.md`, et ouvre une PR de release. Il garde la convention Cargo en `0.x`.
- semantic-release n'est pas retenu: sa première release est `v1.0.0` et il ne prend pas en charge un démarrage en `0.x`.
- Si release-plz est adopté, `CHANGELOG.md` et `tools/site/releases.json` doivent rester d'accord: soit la page lit le changelog, soit un script génère l'un à partir de l'autre. À trancher au moment de la mise en place.
