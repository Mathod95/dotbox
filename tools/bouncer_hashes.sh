#!/usr/bin/env bash
# Le bouncer pare-feu de CrowdSec dans catalog/core/crowdsec.yml (Journal, 7.14).
# Renovate ne change que sa version: ce script en déduit l'adresse de l'archive et le chemin du
# binaire, télécharge l'archive, la compare à l'empreinte que GitHub publie pour ce fichier de la
# release, puis calcule l'empreinte du binaire qu'elle contient.
#   tools/bouncer_hashes.sh          réécrit url, sha256, binary et binary_sha256
#   tools/bouncer_hashes.sh --check  vérifie seulement, échoue si une ligne diffère
set -euo pipefail

manifest="catalog/core/crowdsec.yml"
repo="crowdsecurity/cs-firewall-bouncer"
asset="crowdsec-firewall-bouncer-linux-amd64.tgz"

field() { sed -n "s/^    $1: *//p" "$manifest"; }

version=$(field version)
if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version du bouncer illisible dans $manifest: '$version'" >&2
  exit 1
fi
url="https://github.com/$repo/releases/download/$version/$asset"
binary="crowdsec-firewall-bouncer-$version/crowdsec-firewall-bouncer"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

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

if [ "${1:-}" = "--check" ]; then
  status=0
  for pair in "url=$url" "sha256=$sha256" "binary=$binary" "binary_sha256=$binary_sha256"; do
    key=${pair%%=*}
    want=${pair#*=}
    got=$(field "$key")
    if [ "$got" != "$want" ]; then
      echo "$manifest: $key vaut '$got', attendu '$want'" >&2
      status=1
    fi
  done
  [ "$status" -eq 0 ] && echo "bouncer $version: adresse, chemin et empreintes conformes"
  exit "$status"
fi

sed -i \
  -e "s|^    url: .*|    url: $url|" \
  -e "s|^    sha256: .*|    sha256: $sha256|" \
  -e "s|^    binary: .*|    binary: $binary|" \
  -e "s|^    binary_sha256: .*|    binary_sha256: $binary_sha256|" \
  "$manifest"
echo "bouncer $version: $manifest mis à jour"
