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

say() { printf '  %-9s%s\n' "$1" "$2"; }
die() { printf '  %-9s%s\n' "error" "$1" >&2; exit 1; }
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

  # 1. root
  if [ "$(id -u)" -ne 0 ]; then
    die "run this script as root: su -, then run the same command again"
  fi
  say "check" "root"

  # 2. Linux amd64: sinon le binaire ne démarrerait même pas («Exec format error»)
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  if [ "$os" != "Linux" ] || [ "$arch" != "x86_64" ]; then
    die "Dot Box runs on Linux amd64 only (this machine: ${os} ${arch})"
  fi
  say "check" "Linux amd64"

  # wget: présent par défaut sur Debian et Ubuntu, contrairement à curl
  for cmd in wget sha256sum install mktemp; do
    command -v "$cmd" >/dev/null 2>&1 || die "missing command: ${cmd}"
  done

  # 3. téléchargement et vérification du checksum
  TMP_DIR="$(mktemp -d)"
  trap cleanup EXIT

  say "download" "${ASSET} (latest release)"
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
  say "verify" "sha256 ok"

  install -m 0755 "${TMP_DIR}/${ASSET}" "$BIN"
  say "install" "$BIN"
  cleanup
  TMP_DIR=""

  # 4. dotbox init, sauf --no-init
  if [ "$no_init" -eq 1 ]; then
    say "done" "dotbox installed, init skipped (--no-init)"
    return 0
  fi

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
