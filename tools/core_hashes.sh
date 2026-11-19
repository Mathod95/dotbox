#!/usr/bin/env bash
# Les fichiers du core téléchargés par dotbox init (Journal, 7.14 et 10.7). Renovate ne change que
# leur version: ce script en déduit le reste, télécharge chaque archive et calcule ses empreintes.
#   Le bouncer pare-feu (catalog/core/crowdsec.yml): l'adresse de l'archive, comparée à
#   l'empreinte que GitHub publie pour ce fichier de la release, et le binaire qu'elle contient.
#   Le plugin de Traefik (catalog/core/crowdsec-plugin.yml): l'archive du tag et son dossier.
#   tools/core_hashes.sh          réécrit les lignes qui suivent la version
#   tools/core_hashes.sh --check  vérifie seulement, échoue si une ligne diffère
set -euo pipefail

mode="${1:-}"
status=0

field() { sed -n "s/^    $2: *//p" "$1"; }

# compare (--check) ou réécrit les lignes « clé=valeur » données, dans le manifest $1
apply() {
  local manifest=$1 what=$2
  shift 2
  if [ "$mode" = "--check" ]; then
    local ok=1
    for pair in "$@"; do
      local key=${pair%%=*} want=${pair#*=} got
      got=$(field "$manifest" "$key")
      if [ "$got" != "$want" ]; then
        echo "$manifest: $key vaut '$got', attendu '$want'" >&2
        ok=0
        status=1
      fi
    done
    [ "$ok" -eq 1 ] && echo "$what: conforme"
  else
    for pair in "$@"; do
      local key=${pair%%=*} want=${pair#*=}
      sed -i "s|^    $key: .*|    $key: $want|" "$manifest"
    done
    echo "$what: $manifest mis à jour"
  fi
  return 0
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# --- le bouncer pare-feu ---
manifest="catalog/core/crowdsec.yml"
repo="crowdsecurity/cs-firewall-bouncer"
asset="crowdsec-firewall-bouncer-linux-amd64.tgz"

version=$(field "$manifest" version)
if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version du bouncer illisible dans $manifest: '$version'" >&2
  exit 1
fi
url="https://github.com/$repo/releases/download/$version/$asset"
binary="crowdsec-firewall-bouncer-$version/crowdsec-firewall-bouncer"

curl -fsSL -o "$tmp/$asset" "$url"
sha256=$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)

published=$(curl -fsSL -H "Accept: application/vnd.github+json" \
  ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} \
  "https://api.github.com/repos/$repo/releases/tags/$version" |
  python3 -c 'import json, sys
for a in json.load(sys.stdin)["assets"]:
    if a["name"] == sys.argv[1]:
        print((a.get("digest") or "").removeprefix("sha256:"))' "$asset")
if [ "$published" != "$sha256" ]; then
  echo "l'archive $asset $version ne correspond pas à l'empreinte publiée par GitHub ('$published')" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp" "$binary"
binary_sha256=$(sha256sum "$tmp/$binary" | cut -d ' ' -f 1)

apply "$manifest" "bouncer $version" "url=$url" "sha256=$sha256" "binary=$binary" "binary_sha256=$binary_sha256"

# --- le plugin CrowdSec de Traefik ---
manifest="catalog/core/crowdsec-plugin.yml"
prepo="maxlerebourg/crowdsec-bouncer-traefik-plugin"
version=$(field "$manifest" version)
if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version du plugin illisible dans $manifest: '$version'" >&2
  exit 1
fi
url="https://github.com/$prepo/archive/refs/tags/$version.tar.gz"
dir="crowdsec-bouncer-traefik-plugin-${version#v}"
curl -fsSL -o "$tmp/plugin.tgz" "$url"
# la liste dans un fichier: avec pipefail, grep -q qui s'arrête tôt ferait échouer tar (SIGPIPE)
tar -tzf "$tmp/plugin.tgz" >"$tmp/plugin.list"
if ! grep -qx "$dir/go.mod" "$tmp/plugin.list"; then
  echo "l'archive du plugin $version n'a pas $dir/go.mod" >&2
  exit 1
fi
sha256=$(sha256sum "$tmp/plugin.tgz" | cut -d ' ' -f 1)
apply "$manifest" "plugin $version" "url=$url" "sha256=$sha256" "dir=$dir"

exit "$status"
