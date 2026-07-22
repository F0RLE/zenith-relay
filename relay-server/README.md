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

Stop the service, then run:

```bash
zenith-relay-server --backup /secure/path/relay-backup
zenith-relay-server --restore /secure/path/relay-backup
```

The server holds an exclusive data-directory lock, so maintenance commands fail
instead of racing a running process. Backup is built and validated in a temporary
directory before it becomes visible. Restore validates the manifest, SQLite
store, encrypted vault, secret references, and original `ZENITH_RELAY_VAULT_KEY`
before replacing live files. Store the backup and vault key separately.

## Data Retention

Raw request and error rows are retained for 90 days and capped at 100,000
rows. Before pruning, Relay atomically records lifetime and UTC-day rollups by
client key and model; request, token, timing, and API-equivalent totals therefore
survive without retaining request details. Daily rollups and request-id
deduplication markers use a 400-day window. Wake history is bounded in its state
machine, and unfinished import payloads expire after 30 minutes. Explicitly
clearing usage removes both raw rows and their rollups.

## Database Migrations

Migrations in `migrations/` are append-only and run in numeric order. Schema
version 2 records the applied filename/version ledger in `schema_migrations`.
Before upgrading an existing database, Relay creates a consistent
`relay.sqlite.pre-migration` snapshot and a durable in-progress marker. If the
process stops during migration, the next start validates and restores that
snapshot before retrying. The snapshot remains available after a successful
migration; a database created by a newer Relay version is rejected without
rewriting it.
