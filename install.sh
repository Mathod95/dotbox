#!/usr/bin/env bash
# Installe Dot Box: télécharge le binaire dotbox, vérifie son SHA-256 et lance `dotbox init`.
#
#   su -
#   wget -qO- https://github.com/Mathod95/dotbox/releases/latest/download/install.sh | bash
#
# Options:
#   --no-init   installe seulement le binaire (migration, voir Restore & Migration)
#   -h, --help  affiche l'aide
# Toute autre option est transmise à `dotbox init` (par exemple --user admin).
#
# Le script ne fait que ce qui doit se passer avant que le binaire tourne. Le reste (version du
# système, sudo, utilisateur, Docker, Traefik, Authelia, CrowdSec, Dot Box) est le travail de
# `dotbox init`. Tout le code est dans main(), appelée à la dernière ligne: si le téléchargement
# du script est coupé en cours de route, rien ne s'exécute.

set -euo pipefail

REPO="Mathod95/dotbox"
ASSET="dotbox-linux-amd64"
BASE_URL="https://github.com/${REPO}/releases/latest/download"
BIN="/usr/local/bin/dotbox"
TMP_DIR=""

# Affichage (mockup final de CLI Output, D38): une seule ligne, [1/9] Dot Box, puis dotbox init
# continue de [2/9] à [9/9]. Couleurs seulement dans un terminal.
if [ -t 2 ]; then
  GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; DIM=$'\033[2m'; RESET=$'\033[0m'; CLEAR=$'\r\033[K'
else
  GREEN=""; RED=""; YELLOW=""; DIM=""; RESET=""; CLEAR=""
fi
PENDING=0

# step MARQUE RÉSUMÉ [DURÉE]: la ligne de l'étape, sur 80 colonnes
step() {
  printf '%s  %s %s[1/9]%s %-13s %-49s %s%6s%s\n' "$CLEAR" "$1" "$DIM" "$RESET" "Dot Box" "$2" "$DIM" "${3:-}" "$RESET" >&2
}
say() { printf '  %-9s%s\n' "$1" "$2"; }
die() {
  if [ "$PENDING" -eq 1 ]; then step "${RED}✗${RESET}" ""; fi
  printf '  %s%-6s%s%s\n' "$RED" "error" "$RESET" "$1" >&2
  exit 1
}

# durée depuis START (date +%s%N): « 0.8 s », « 12 s »
elapsed() {
  local now; now="$(date +%s%N)"
  awk -v ms="$(( (now - START) / 1000000 ))" 'BEGIN { s = ms / 1000; if (s < 10) printf "%.1f s", s; else printf "%d s", s + 0.5 }'
}
cleanup() { if [ -n "$TMP_DIR" ]; then rm -rf "$TMP_DIR"; fi; }

usage() {
  cat <<'EOF'
Usage: wget -qO- https://github.com/Mathod95/dotbox/releases/latest/download/install.sh | bash [-s -- OPTIONS]

Installs the dotbox binary in /usr/local/bin, then runs `dotbox init`. Run it as root (su -).

Options:
  --no-init    Install the binary only (migration, see Restore & Migration)
  -h, --help   Show this help
Any other option is passed to `dotbox init`.
EOF
}

main() {
  local no_init=0
  local init_args=()
  while [ $# -gt 0 ]; do
    case "$1" in
      --no-init) no_init=1 ;;
      -h|--help) usage; exit 0 ;;
      *) init_args+=("$1") ;;
    esac
    shift
  done

  # 1. root (vérifications silencieuses: seule une erreur s'affiche, Journal D30)
  if [ "$(id -u)" -ne 0 ]; then
    die "run this script as root: su -, then run the same command again"
  fi

  # 2. Linux amd64: sinon le binaire ne démarrerait même pas («Exec format error»)
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  if [ "$os" != "Linux" ] || [ "$arch" != "x86_64" ]; then
    die "Dot Box runs on Linux amd64 only (this machine: ${os} ${arch})"
  fi

  # wget: présent par défaut sur Debian et Ubuntu, contrairement à curl
  for cmd in wget sha256sum install mktemp; do
    command -v "$cmd" >/dev/null 2>&1 || die "missing command: ${cmd}"
  done

  # 3. téléchargement et vérification du checksum
  TMP_DIR="$(mktemp -d)"
  trap cleanup EXIT

  START="$(date +%s%N)"
  PENDING=1
  if [ -n "$CLEAR" ]; then
    printf '  %s⠼%s %s[1/9]%s %-13s %s' "$YELLOW" "$RESET" "$DIM" "$RESET" "Dot Box" "downloading from GitHub" >&2
  fi
  wget -q --https-only -O "${TMP_DIR}/${ASSET}" "${BASE_URL}/${ASSET}" \
    || die "could not download ${ASSET} from ${BASE_URL}"
  wget -q --https-only -O "${TMP_DIR}/${ASSET}.sha256" "${BASE_URL}/${ASSET}.sha256" \
    || die "could not download ${ASSET}.sha256 from ${BASE_URL}"

  local expected actual
  expected="$(cut -d ' ' -f 1 "${TMP_DIR}/${ASSET}.sha256")"
  actual="$(sha256sum "${TMP_DIR}/${ASSET}" | cut -d ' ' -f 1)"
  if ! [[ "$expected" =~ ^[0-9a-f]{64}$ ]]; then
    die "${ASSET}.sha256 does not contain a valid SHA-256 checksum"
  fi
  if [ "$expected" != "$actual" ]; then
    die "wrong SHA-256 checksum for ${ASSET}, nothing installed"
  fi
  install -m 0755 "${TMP_DIR}/${ASSET}" "$BIN"
  local version
  version="$("$BIN" --version 2>/dev/null | cut -d ' ' -f 2)"
  step "${GREEN}✓${RESET}" "${version}, downloaded from GitHub, sha256 verified" "$(elapsed)"
  PENDING=0
  cleanup
  TMP_DIR=""

  # 4. dotbox init, sauf --no-init
  if [ "$no_init" -eq 1 ]; then
    say "done" "dotbox installed in $BIN, init skipped (--no-init)"
    return 0
  fi

  # dotbox init sait ainsi que la ligne [1/9] est déjà affichée (D38)
  export DOTBOX_INSTALLER=1

  # Avec wget -qO- … | bash, l'entrée du script est le texte envoyé par wget, pas le clavier.
  # dotbox init est donc relié au terminal pour pouvoir poser ses questions.
  if [ -t 0 ]; then
    exec "$BIN" init "${init_args[@]}"
  elif { : </dev/tty; } 2>/dev/null; then
    exec "$BIN" init "${init_args[@]}" </dev/tty
  else
    exec "$BIN" init "${init_args[@]}"
  fi
}

main "$@"
