# Zenith Relay Server

Standalone personal-pool runtime for a server controlled by the user. It uses
the public Zenith Relay management protocol and does not contain Zenith billing,
provider-economy, inventory, or private account-pool logic.

## Configuration

Generate a random management token and a 32-byte vault key:

```bash
openssl rand -base64 32
openssl rand -base64 32
```

Set the public HTTPS origin, management token, and vault key through a protected
shell or server secret manager. The repository and generated deployment bundle
intentionally contain no secret-bearing `.env` file.

The public listener defaults to `127.0.0.1:14999`. For Internet access, place a
TLS reverse proxy in front of the server. Zenith Relay rejects insecure remote
HTTP unless the user explicitly enables it.

## Run

```bash
cargo run --manifest-path relay-server/Cargo.toml --release
```

or:

```bash
ZENITH_RELAY_PUBLIC_BASE_URL=https://relay.example.com \
ZENITH_RELAY_MANAGEMENT_TOKEN="$MANAGEMENT_TOKEN" \
ZENITH_RELAY_VAULT_KEY="$VAULT_KEY" \
docker compose -f relay-server/compose.yaml up -d
```

Connect the desktop app with the HTTPS origin and management token. Create a
separate pool request key for `/v1/*`; the management token is never accepted by
the model gateway.

## Backup And Restore

Stop writes or stop the service, then run:

```bash
zenith-relay-server --backup /secure/path/relay-backup
zenith-relay-server --restore /secure/path/relay-backup
```

The backup contains SQLite metadata and the encrypted vault. The same
`ZENITH_RELAY_VAULT_KEY` is required after restore. Store the backup and vault
key separately.
