#!/usr/bin/env sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: conformance.sh <https-origin> <management-token>" >&2
  exit 2
fi

origin=${1%/}
token=$2
case "$origin" in https://*) ;; *) echo "public origin must use HTTPS" >&2; exit 2 ;; esac

get() {
  curl --fail --silent --show-error --proto '=https' --tlsv1.2 "$@"
}

health=$(get "$origin/health")
capabilities=$(get -H "Authorization: Bearer $token" "$origin/capabilities")
state=$(get -H "Authorization: Bearer $token" "$origin/state")

python3 - "$health" "$capabilities" "$state" <<'PY'
import json, sys
health, capabilities, state = map(json.loads, sys.argv[1:])
assert health.get("status") == "ok"
assert isinstance(health.get("serverId"), str) and health["serverId"]
assert capabilities.get("serverId") == health["serverId"]
assert isinstance(capabilities.get("features"), list)
assert capabilities.get("protocolMin", 0) <= capabilities.get("protocolMax", -1)
assert isinstance(state, dict) and "gateway" in state
serialized = json.dumps([health, capabilities, state]).lower()
for forbidden in ("access_token", "refresh_token", "api_key", "authorization"):
    assert forbidden not in serialized, f"secret-shaped field exposed: {forbidden}"
PY

unauthorized=$(curl --silent --output /dev/null --write-out '%{http_code}' --proto '=https' --tlsv1.2 "$origin/state")
[ "$unauthorized" = 401 ] || { echo "management endpoint accepted no token: $unauthorized" >&2; exit 1; }
echo "compatible: $origin"
