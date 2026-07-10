#!/usr/bin/env sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: install.sh <binary-url> <sha256>" >&2
  exit 2
fi

url=$1
expected=$2
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
curl --fail --location --proto '=https' --tlsv1.2 "$url" --output "$tmp"
actual=$(sha256sum "$tmp" | awk '{print $1}')
if [ "$actual" != "$expected" ]; then
  echo "checksum mismatch" >&2
  exit 1
fi
install -m 0755 "$tmp" /usr/local/bin/zenith-relay-server
install -d -m 0750 -o relay -g relay /var/lib/zenith-relay
echo "binary installed; configure /etc/zenith-relay/environment and enable the systemd unit"
