# Relay Hosted server

Run this Rust binary independently of Desktop. It serves a closed installation:
the production landing and public `/docs` and `/legal` pages use the verified
installation and its configured branding. No sign-in is needed to read them.
The operator binds the first owner locally. The owner then signs in at `/admin`
with the enrolled passkey. An optional OIDC registration lets the owner admit
named testers to their personal wallets, API keys and request history. Public registration,
checkout and public inference remain closed.

## Install locally

1. Use a **new, dedicated PostgreSQL database**. Create three separate login
   roles: `relay_migrator`, `relay_setup`, `relay_runtime`. Grant the migrator
   `CREATE` on this database; grant the other two `CONNECT`, no schema creation,
   role membership, superuser or database ownership. Keep backups separate.
2. Copy [config.example.toml](config.example.toml) to a private operator directory.
   Set `ZENITH_RELAY_HOSTED_CONFIG` to its absolute path. Put each role's PostgreSQL
   URL in its own owner-readable file outside the website. Select the correct
   file for each command using `ZENITH_RELAY_HOSTED_DATABASE_URL_FILE`. Specify
   the role, host and database in the URL; non-loopback hosts require
   `sslmode=verify-full`. The only other supported URL option is `sslrootcert`.
   Never put passwords in TOML, shell arguments or the web bundle.
3. From the repository root, build the binary and optional web artifact:

   ```sh
   cargo build --locked --release --manifest-path hosted/server/Cargo.toml
   cd hosted/web
   bun install --frozen-lockfile
   bun run build
   ```

   On a configured Windows checkout, `bun scripts/build/cargo.mjs` can replace
   `cargo`. Passkey verification uses OpenSSL: install its development headers
   and `pkg-config` on Linux. Windows builds the locked vendored OpenSSL with
   MSVC, NASM and a native Perl (`perl` on PATH, or `OPENSSL_SRC_PERL`).
   Use the built `zenith-relay-hosted` executable for the commands below.
4. With the migrator credential, run `zenith-relay-hosted migrate`. It locks
   migrations, checks their checksums and creates one installation and one
   `setup` fence. Rerunning never changes the identity, origin, owner or mode.
   As the migrator apply these least-privilege grants, then run
   `psql -v ON_ERROR_STOP=1 -v runtime_role=relay_runtime -f hosted/server/runtime-grants.sql`
   on the same database. Apply the versioned grants on upgrade as well.

   ```sql
   GRANT USAGE ON SCHEMA relay_hosted TO relay_runtime, relay_setup;
   GRANT SELECT ON relay_hosted.closed_installation, relay_hosted._sqlx_migrations
     TO relay_runtime, relay_setup;
   GRANT SELECT ON relay_hosted.pending_owner TO relay_setup;
   GRANT EXECUTE ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint)
     TO relay_setup;
   ```

5. To provision the owner, stop the normal listener, select the **setup** credential
   and run `zenith-relay-hosted setup-owner --web-dist PATH_TO_DIST --code-dir PRIVATE_DIR`.
   Use an existing operator-only directory (Unix `0700`, Windows private ACL).
   Open `/setup/owner` on the configured origin. Enter the code from the file
   named in the terminal, create a discoverable passkey, then verify it. The
   private key stays with the authenticator. The verified public credential and
   owner assignment commit together. The command exits without opening service.
6. Select the **runtime** credential. Run `zenith-relay-hosted serve --web-dist PATH_TO_DIST`
   and open the configured origin (example: `http://localhost:1443/`). For an
   API-only process omit `--web-dist`. The normal server does not register setup
   endpoints. Open `/admin` on the **configured origin**, not the Vite preview;
   sign in with the enrolled passkey. An unknown migration, changed checksum/origin or missing service
   state is an error, not an invitation to initialize another installation.

**If setup is interrupted:** the code, registration and proof expire together
after three minutes and are not resumable. A stale code file from a killed
process cannot authorize a new one. Run the local command again: if an owner is
already bound it refuses without issuing a code. Do not drop the database or
delete a credential to retry. Keep the enrolled authenticator available.

### Manage owner passkeys

After migration `0045`, reapply [runtime-grants.sql](runtime-grants.sql).
On the first signed-in Overview, open **Set up a backup passkey**. Confirm with
an existing passkey, create the backup on another device or security key, then
verify the new key. The confirmation flow expires after three minutes and
allows up to eight active keys. Any active key can sign in as the same owner.

Use **Passkeys** on Overview to revoke a key. Confirm with a different active
passkey. The server will not remove the last one and ends only the sessions
that used the revoked key, including the current session if applicable.
The list shows labels, creation time and which key signed in; it does not
return stored verifiers. The database records each committed add or revoke
in `relay_hosted.owner_passkey_changes`.

### Recover owner access

If all owner passkeys are lost, recover against the same installation and RP.
With the migrator credential, record the installation UUID (`SELECT id FROM
relay_hosted.installations WHERE singleton`) and grant a dedicated `relay_recovery`
login just the [recovery grants](owner-recovery-grants.sql):

```sh
psql -v ON_ERROR_STOP=1 -v recovery_role=relay_recovery -f hosted/server/owner-recovery-grants.sql
```

Keep this credential offline when not recovering. Stop the regular web listener
and restrict access to the temporary listener to the operator at the configured
origin. Select the recovery credential and a new operation UUID:

```sh
zenith-relay-hosted recover-owner --installation INSTALLATION_UUID --operation OPERATION_UUID --web-dist PATH_TO_DIST --code-dir PRIVATE_DIR
```

Open `/setup/recovery`, enter the one-use code from the local file and create and
verify the replacement within three minutes. Only a verified replacement changes
the database. The commit keeps the same owner, revokes previous passkeys and
sessions, invalidates paired Desktop grants, and appends a recovery receipt.
It does not change customer authority, wallet history or the service mode.

If the confirmation was lost, repeat the command with the **same** operation UUID
to check whether the recovery committed. A committed operation issues no new
code. The migration owner can inspect `relay_hosted.owner_recoveries` by operation.
After the recovery listener stops, restart `serve`, sign in with the replacement
and pair Desktop again if needed. Normal `serve` has no recovery endpoint.
A [fenced restore](#fence-an-isolated-restore) cannot use this procedure to reopen.

### Check an upgrade

With the **candidate** binary, the current configuration and the runtime database
credential, run `zenith-relay-hosted upgrade-check` before scheduling downtime.
It reads one database snapshot and prints the installed version, target version
and ordered pending migrations. It rejects a different origin, gaps or failed
entries, checksum changes and a database ahead of the binary. It does not run
migrations, reset sessions or change the service mode.

A compatible history is only a preflight result. Use a verified backup, stop and
drain the services before `migrate`, reapply the versioned grants, and verify the
upgrade in the closed installation before reopening access.

### Fence an isolated restore

Keep the replacement database on private networking. Do not start Relay, route
webhooks to the copy, or give it provider/merchant egress. Stop the original
execution authority before considering a cutover; the copy shares its
installation ID and must never run alongside it as a second authority.

Restore compatible database, configuration and encrypted vault copies. Keep the
key separate. On the **replacement** database, run `upgrade-check` and use the
matching binary and schema (including `0043`). With the migration owner's
credential for that database and the expected ID from the recovery inventory,
run:

```sh
zenith-relay-hosted restore-fence --installation INSTALLATION_UUID --operation RECOVERY_UUID
```

The command checks the configured origin and exact schema, then enters
`maintenance`, clears the qualification scope and invalidates pre-restore
sessions, inference keys and paired-device grants. A retry with the same
operation ID does not advance the epochs again. A mismatch or error means keep
the copy isolated.

The fence survives restarts and migrations. Normal Relay startup, new invoice
and refund claims, and automatic payment-credit replay are blocked. Preserve
pending deliveries and the existing journal and holds. Reconcile payments,
refunds and potentially dispatched requests against evidence outside the
backup, and reapply revocations/deletions. Absence from the restored database
is not evidence of a missing external effect. No command currently clears
the fence; this is a containment step, not a production recovery or reopening
procedure.

#### Reapply customer access decisions

After stopping access changes on the authoritative installation, run this with
its migration credential and a new private file outside the database backup:

```sh
(umask 077; zenith-relay-hosted export-customer-access --installation INSTALLATION_UUID > PRIVATE_DIR/customer-access.json)
```

Keep the successful export with the off-host recovery evidence. On a compatible,
isolated restore fenced with `RECOVERY_UUID`, use the replacement database's
migration credential:

```sh
zenith-relay-hosted restore-customer-access --installation INSTALLATION_UUID --operation RECOVERY_UUID --receipts PRIVATE_DIR/customer-access.json
```

Replay applies the original revisions in order and skips exact receipts already
present. A missing workspace, conflicting receipt or wrong fence aborts the
replay. An older snapshot cannot prove that later suspensions or closures did
not happen. Keep ingress closed if evidence is missing or incomplete; reconcile
individual key revocations and financial obligations separately.

### Owner sessions

`serve` never accepts a setup code, inference key or customer cookie as an owner.
Each sign-in uses an origin-bound, single-use challenge (two minutes, at most 64
starts per process per two minutes) and requires user verification. The server
checks the installation, active assignment, credential and authorization epochs.
`/admin/overview` reads the service mode. **Approve 24-hour access** and
**Pause customer access** each require their own passkey confirmation. The
confirmation keeps the current session, covers the chosen transition and review
IDs at the displayed revision, and cannot authorize another change. After a
lost response, retry the same operation; a recorded receipt does not require a
new confirmation. The same pattern applies to payment and supply writes in the
[owner action matrix](../../docs/hosted/HOSTED_RELAY_ARCHITECTURE.md#owner-confirmation-by-action).
After upgrading through `0046`–`0048`, reapply [runtime grants](runtime-grants.sql)
before starting `serve`.

After migrations `0037` and `0038`, reapply [runtime-grants.sql](runtime-grants.sql).
Owner Overview reads `/api/hosted/v1/admin/overview/usage`: rolling 24h, 7d,
30d or All time over retained logical requests, optionally by exact public
model ID. It sums posted inference charges minus posted request credits at
request start time. Top-ups, holds, payment refunds and supplier expense are
not charges. Refresh reads one snapshot, without running providers or extending
the owner session. The runtime role cannot invoke the private aggregate or
read its source tables directly.

Owner Usage reads `/api/hosted/v1/admin/usage/requests`, `/errors` in the same
family, and `/requests/{id}`. Requests and Errors use 32-row pages, a rolling
seven-day default, or exact UTC bounds copied from an Overview interval.
Continue with `next_before` and the same bounds and filters. Bounds stay fixed;
settlement and error membership are read live. Errors distinguishes execution
outcome from pending settlement, missing final usage and unposted corrections.
Stage, origin, category, severity and code filters match the same retained
observation. The list shows the latest match. Detail keeps observations separate
from the dispatch timeline, posted charge and physical supplier valuation. An
HTTP status is not proof of fault, and a permit is not proof of delivery. These
reads require the owner session and do not touch its idle timer.

After migrations `0039`–`0040`, reapply [runtime](runtime-grants.sql) and
[inference](inference-grants.sql) grants before restarting both processes.
Request diagnostics are allowlisted observations, with one record per request
and code and a cap of 262,144 rows per installation. They expire 30 days from
request start. A bounded background writer can drop observations on timeout or
capacity pressure. This never authorizes another send or replaces terminal
evidence and journal entries. Absent observations mean unavailable detail.

The `__Host-relay_admin` and `__Host-relay_admin_attempt` cookies use `Path=/`,
Secure, HttpOnly, SameSite=Strict and no Domain. Sessions expire after 20 idle
minutes or eight hours total; at most 16 are retained for the owner. Logout
requires exact Origin and the current overview's `X-Relay-CSRF` value; it deletes
the session before clearing cookies. The process-local HMAC key deliberately
invalidates attempts and sessions on **every `serve` restart**, including a
restore. Run one web process. Browser refresh does not sign the owner out.

### Owner operations

**Overview → Operations** reads the last saved passes of the projection
cleanup jobs and the `serve-text` request reconciler. A reconciliation pass
becomes stale after 20 seconds; a cleanup pass after 90 seconds. A successful
pass is not proof that all holds are closed or that inference is ready.
The page does not monitor provider refresh, payment processing or backups.

Recent events keep up to 128 failure groups or recovery events per job and
expire 14 days from their first observation. A repeated failed pass extends its
group until a recovery, a known gap or expiry. The page flags known gaps and
never treats empty history as a healthy result. A failed or stale reconciliation
calls for checking the text process, database connectivity and grants using the
safe process error. Fix the underlying failure before restarting the worker;
manual `reconcile-text` does not count as a worker observation.

After migration `0041`, reapply [runtime](runtime-grants.sql) and
[inference](inference-grants.sql) grants. A failed observation write cannot
change the outcome of a cleanup or reconciliation pass.

#### Temporary diagnostics

In **Overview → Operations → Temporary diagnostics**, enter one in-progress
request ID from **Usage** or `X-Request-Id`, choose a reason and set the duration.
Start requires same-origin CSRF and a one-use owner passkey assertion for that
request, reason, duration and current limits. Stop does not require another
passkey. Neither customer accounts nor paired Desktop can read or operate
captures. Repeating the same start operation returns the existing capture
without extending it.

Apply the [schema upgrade](#check-an-upgrade) with the runtime and inference grants.
Set `[diagnostics.capture]` in the configuration for both `serve` and `serve-text`.
All three caps are required to enable capture: `max_events` (1–128),
`max_bytes` (128–32768, serialized event bytes), and `max_events_per_second`
(1–16, rolling second). `default_seconds` defaults to 600 and must be within
1–`max_seconds`; `max_seconds` defaults to 1800 and may not exceed it.
Config is read at process startup. At most one capture can run.

Only new request milestones are collected: dispatch permission, upstream/first
stream frame, terminal observation, evidence and accounting states, or
interruption. The projection is partial, including during collector startup or
storage failure. It does not contain request/response bodies, headers, token
values, or supplier secrets. Budget and expiry checks run server-side at append;
stopping, signing out of the authorizing session, and a `serve` restart fence
further writes. A `serve-text` restart does not reset the deadline or budgets.
Optional writes cannot trigger inference, retry a supplier, or change settlement.

Closed metadata is visible for 14 days, request events expire 30 days from
request start, and the private audit lasts 180 days. Stop commits independently
of the audit sink; cleanup retries a missing closure audit. The list shows up to
12 recent captures; new captures are refused at 1024 retained start records.

### Restricted customer sign-in

1. Bind the owner first. Register the site as a **public authorization-code
   client** at your identity provider (no client secret, PKCE S256, RS256 ID
   tokens). Set the optional `[customer_oidc]` fields in the example config.
   Send the exact redirect URI `{public_origin}/api/hosted/v1/auth/customer/{id}/callback`
   to the issuer. Pin its three endpoints on the issuer's own HTTPS origin.
   Cross-origin endpoints, dynamic discovery and confidential clients are not
   supported by this profile. Only a loopback development site may use an HTTP
   issuer on a literal loopback IP. Do not log callback query strings.
2. Give `serve` a separate cryptographically random 32-byte binary file at
   `ZENITH_RELAY_HOSTED_DIGEST_KEY_FILE` (Unix `0600`, Windows private ACL).
   Back it up separately from PostgreSQL. It binds the registration and protects
   case-sensitive subjects; do not replace it or reuse the vault key. A changed
   registration/key cannot silently take over earlier subjects. Omit the section
   to disable all customer sign-in and reads on that process.
3. In setup, a tester opens `/login`, completes sign-in and shares the displayed
   review ID with the owner through a trusted channel. It expires in ten minutes,
   proves no authority by itself and has no wallet. In `/admin` enter the IDs
   for this run (up to 16) and confirm with the owner passkey. Qualification
   lasts at most 24 hours. It enables only these identities; the landing does
   not advertise open registration.
4. Each approved tester signs in again to see their personal USD balance at
   `/app/overview`. A returning identity retains its wallet. Open **Account
   security** in the header to review and end active browser sessions. Sessions
   last up to eight hours, with a 30-minute idle limit and up to eight per
   customer. Restarting `serve` invalidates browser cookies; API keys and wallet
   history are independent. Pause access in `/admin` before changing testers.

On upgrade, migrate through `0053` and reapply [runtime grants](runtime-grants.sql)
before starting `serve`.

#### Sign-in limits

`serve` admits up to 64 starts per rolling three minutes and six per rolling
20 seconds for the same short-lived browser cookie. At most four starts can
prepare at once, independently of the four slots for token exchange and final
sign-in. Excess starts return `429` with `Retry-After`; they do not replace a
pending attempt. If the exchange slots are busy, the callback redirects to
`/login#busy` without consuming the attempt. The page never retries automatically.

An attempt lasts at most three minutes. Before contacting the issuer, the
server checks the installation and service revision, plus the configured
issuer binding and scope expiry in qualification. It then verifies the
subject and rechecks admission in PostgreSQL before creating a session. Setup
and maintenance only allow review IDs. The bounded counters and attempts are
process-local, pruned on the next start, and discarded on restart. They use
no email or subject input.

For remote qualification, keep `serve` on loopback behind a trusted HTTPS
ingress restricted to the intended testers. Bound request sizes, connection
concurrency and start/callback rates at that ingress, with separate budgets
so busy starts do not consume callback capacity. Apply per-source limits
using the peer observed at the trusted edge, not caller-provided `Forwarded`
or `X-Forwarded-For`; `serve` does not read those headers for login limits.
Cookies can be cleared, so the browser budget does not replace edge limits.
Exclude callback queries and cookies from logs. Qualify the ingress and real
issuer before opening access. In-app notices work independently; external
recipients stay disabled until HD-13 is qualified.

### Customer access cases

As owner, open one workspace from request detail, an exact workspace filter
in **Usage**, or a refund case. There is no customer directory or paired
Desktop access. **Suspend access**, **Reinstate access** and **Close account**
each require a reason and a new passkey assertion for the exact workspace
and revision. If the response is lost, **Check result** looks for the same
operation's receipt without changing access.

Suspension and closure end browser sessions, invalidate API keys, and block
new payment intents and requests. Queued requests cannot be sent and their
holds are released. Dispatched requests and already accepted payments still
reconcile. Reinstatement permits a fresh sign-in and new keys; it does not
restore old credentials. Closure is permanent and does not delete receipts,
adjust the balance, or issue a refund.

**Account security** links to closure support when `[policies].support` is
configured. The customer should save their receipts and request any refund
separately before losing access. Only close after verifying the customer's
request. The support link does not perform an account change.

### Customer API keys

An approved tester opens **API keys** from Overview, enters a name and selects
**Create key**. Copy the secret before closing the result. Reloads and retries
return only masked metadata; the server keeps a one-way verifier. If a client
loses the issuance response, look up the same `operation_id` using `GET /keys`.
Reissue creates a replacement without revoking the original; migrate the client,
then confirm revocation of the old key. Signing out does not revoke a key.

Each qualification allows 32 issuances per workspace and authorization epoch,
including replacements.
Keys cannot outlive that specific approval or extend the key they replace.
Suspension and revocation block subsequent credential checks. Neither owner
sessions nor inference keys can manage a customer's keys.

### In-app service notices

In `/admin/notifications`, create, edit or discard a draft. To send a saved
revision, choose **Send**, then **Confirm sending**. Confirm that revision with
an owner passkey in the same session. The server atomically records one inbox item
per already enrolled, eligible customer in the current qualification scope
(at most 16); if nobody is eligible, the draft remains unpublished.
Later sign-ins cannot add recipients. Published content is immutable, and the
owner sees the fixed recipient count, not customer identities or read activity.

The customer bell refreshes when opened or when returning to the page. **Mark as
read** applies only to the displayed unread items in that recipient's inbox.
Reading or publishing a notice does not change access, pricing or money.
Revocation and expired approval deny later reads; a new qualification scope
does not inherit an old scope's notices.

The API bounds the inbox to 32 unexpired publications and allows 16 drafts.
Notice commands use the expected revision and a UUID operation ID. On a lost
response, retry the same command without changing the ID; the server returns
the original acknowledgement. Publication and acknowledgement commit together.
The runtime prunes expired messages and read markers after 30 days and keeps
up to 2,048 command acknowledgements for a 30-day replay window.

### Private connection storage

Use a fourth, separate unprivileged `relay_vault` login with `CONNECT`.
Apply [vault-grants.sql](vault-grants.sql) with `psql -v ON_ERROR_STOP=1 -v vault_role=relay_vault -f hosted/server/vault-grants.sql`
as the migrator. Never grant these rights or mount the key/data directory in
the public web process.

1. Create an empty, private directory **outside the site, Desktop and key
   directories** on a local persistent filesystem. Provide a cryptographically
   random **32-byte binary** key in a separate private file, not hex or a
   passphrase. Unix: directory `0700`, file `0600`; Windows: restrict both ACLs
   to the operator/service identity. Do not use links or reparse points.
2. Select the vault role's DB credential, set `ZENITH_RELAY_HOSTED_VAULT_KEY_FILE`
   to the absolute key path and run `zenith-relay-hosted vault-check --vault-dir ABSOLUTE_PRIVATE_DIR`.
   The first run binds an encrypted installation marker to PostgreSQL. A repeat
   checks the marker and key; a missing bound marker or wrong key fails instead
   of initializing a replacement. No connection is imported by this command.
3. To provision a static qualification source, generate an operation UUID and
   a separate source UUID. Use the vault login with **one** explicit route:

   ```sh
   zenith-relay-hosted vault-provision --operation OP_UUID --source SOURCE_UUID --provider-file PRIVATE_KEY_FILE --proxy-file PRIVATE_PROXY_FILE --vault-dir PRIVATE_DIR
   ```

   For a direct route replace `--proxy-file PRIVATE_PROXY_FILE` with `--direct`.
   Use absolute private paths. Supply the exact synthetic bearer and the selected
   HTTP(S) proxy, including its authentication, without trailing newlines.
   Both slots and the receipt commit together or neither is visible. After a lost
   acknowledgement repeat the same command with the original IDs and inputs.
   A matching receipt confirms the original import; it does not undo later changes
   or revive revoked credentials. Different input or an existing source conflicts.
   A revoked proxy makes a proxied source ineligible rather than turning it direct.
   No provider request, pool membership or retail offer is created here.
4. The private worker API stores provider and proxy secret slots separately.
   Each write protects up to 64 KiB with ChaCha20-Poly1305, authenticating
   installation, slot, purpose, revision and generation. A synchronized,
   no-overwrite file publication precedes the PostgreSQL reference commit.
   A stale revision cannot overwrite a later write or a revocation. Failed
   commits leave the prior binding intact; collection uses a database lock and
   removes only unreferenced generation files. Keep the key, complete vault
   and matching database backup together as a recovery set, stored separately
   and protected. No automatic collection or live key rotation is enabled.

In **Admin → Connections** (`/admin/connections`), the owner can see saved API sources
before admitting them to a pool. The **API providers** and **Trash** tabs paginate
separately; search and state filters apply to the current page. Each card shows
the proxy binding and model checks that match the current tester scope and credentials. An unused
source shows its ID until its first successful check, unless it was copied with
a label from Desktop. Revoked keys remain marked in API providers, not in Trash.
Opening the list makes no provider requests
and returns neither secrets nor upstream or proxy URLs.

These storage operations are not routes for reading credentials. Backup-aware
retention and a cross-host restore drill remain gates before admitting real
credentials.

### Pair a Desktop

1. In **Admin → Connections → Desktop → Paired devices**, enter a device name.
   Connections are included; **View overview**, **View request history** and
   **Change pool membership** are independent and off by default. Paste the invitation into
   **Relay Hosted → Settings → Review server** on the intended computer within five minutes.
2. Check the displayed origin and installation ID. Copy the approval code from
   that computer into the same invitation in the admin panel. Review the
   permissions again and approve access.
   Confirm each invitation, approval or revocation with an owner passkey
   in the current session. Keep at most 16 pending or active devices.
3. Select **Check access** in Desktop Settings. The grant lasts 24 hours;
   sections without approval do not read server data. Overview shows rolling
   24-hour, 7-day or 30-day
   totals and speed from measured streams. Pool readiness and in-flight
   activity are current snapshots, independent of the selected period.
   Usage filters by period, exact model and
   execution status; up to 32 rows per page, with a 15-minute pagination
   window. Refresh to start a new window. No customer IDs, API keys, provider
   prices, raw errors or billing are returned to Desktop.
4. **Revoke access** on the server to reject subsequent reads immediately.
   **Forget on this computer** in Settings removes only its saved pairing. Pair again with
   a new invitation after expiry, revocation, or a change of destination or
   permissions. Existing connections-only grants do not gain additional access.

Use the server's exact HTTPS origin (HTTP is accepted only for loopback).
The invitation contains public IDs; the return code identifies the native
device's verifier, not its credential. Neither can be used as a bearer. The
web runtime stores no device secret and needs no vault permission for pairing.
Migrate through `0069` and reapply [runtime](runtime-grants.sql) and
[inference](inference-grants.sql) grants before starting the updated processes.
Older servers can still return the legacy overview without the new metrics.
See the [control API contract](openapi.yaml).

### Change pool membership from Desktop

1. In **Relay Hosted → Pool**, select **Add to pool** or **Remove from pool**.
   Adding requires an enabled source outside the pool with at least one current
   model check. Saving or copying a source does not admit it.
2. Paste the five-minute code into **Admin → Connections → Desktop → Pool review**.
   Review the device, source and exact models, then confirm with your passkey.
   Changing those inputs or revoking the device invalidates an unconfirmed code.
3. Refresh Pool in Desktop to see the acknowledged membership. Adding
   admits the latest eligible check per model; it leaves prices, model blocks
   and publication unchanged. Removing stops new work and lets dispatched work
   finish. The key and proxy stay saved. To add the source again, resume it in
   the owner panel and run fresh checks.

### Copy one Desktop API source

The copy requires the configured HTTPS origin. Apply the current
[runtime grants](runtime-grants.sql) and [vault grants](vault-grants.sql) after
migrating. On the same host, start a separate worker with the vault role's
`ZENITH_RELAY_HOSTED_DATABASE_URL_FILE` and `ZENITH_RELAY_HOSTED_VAULT_KEY_FILE`:

```sh
zenith-relay-hosted serve-transfers --bind 127.0.0.1:1445 --vault-dir ABSOLUTE_PRIVATE_DIR
```

At the TLS edge route only `GET /api/hosted/v1/transfer/status` and
`POST /api/hosted/v1/transfer/commit` to that loopback port. Pass `Authorization`
through unchanged; don't rewrite paths, retry the POST, or log bodies or
authorization. Keep the listener and vault private. Leave the browser preview
and confirmation routes on `serve`.

1. In Desktop Connections, choose **Copy an API source to the server** and review
   its name, server, API endpoint and proxy. Copy the review code to
   **Admin → Connections → Desktop → Desktop source copy**. The code contains no key or
   proxy credentials.
2. Review the device and route, then select **Approve one copy** and confirm it
   with an owner passkey. The preview needs no separate confirmation. The grant
   expires within ten minutes or when the pairing ends, whichever is earlier.
   Retry the same review after a lost reply; it does not extend the grant.
3. In Desktop, check the approval and select **Copy to server**. Desktop
   verifies its current source matches the review before sending it. If the
   result is unknown, use **Check server result** for the same operation;
   a committed receipt can be read for seven days without sending the key again.
   Use **Check result** in the admin panel to update the Connections list.

This copies a native Responses source with a canonical `/v1` endpoint and its
explicit proxy (including credentials), or an explicit direct route. The local
source is unchanged. The destination appears in Connections outside the pool;
the copy does not discover models, publish prices or prove reachability.
Qualification uses the exact reviewed route and never drops an imported proxy.
Migration `0049` expires unapplied approvals issued under the previous sign-in
window. It keeps existing pairings and committed copies. Create a new review
code in Desktop for an expired copy.

### Add an API source in the admin panel

With the same HTTPS transfer worker and grants, route
`POST /api/hosted/v1/transfer/intake/{operation_uuid}` to that worker. Pass
`Origin` and `Authorization` unchanged, limit bodies to 3,073 bytes, and disable
body/credential logs and POST retries. Keep `/api/hosted/v1/admin/*` on `serve`.
Then set `web_api_intake = true` under `[server]` and restart `serve`.

1. Open **Connections → API providers → Add API connection**. Enter the
   label, native Responses `/v1` endpoint, provider key, and direct or proxy
   route. If using a proxy, include any authentication in the write-only URL.
   **Models** accepts optional IDs awaiting verification.
2. Save with a fresh owner passkey. Approval covers this operation and its
   exact upload for two minutes in the current session. The transfer worker
   stores the key, any proxy and the receipt together. A lost reply can be
   checked in the same browser tab without resending credentials.
3. [Check a model](#check-a-model-in-the-admin-panel) in the local HTTPS panel
   or use the [qualification CLI](#first-funded-text-request). Saving never
   contacts the provider.

### Check a model in the admin panel

After migration `0060`, reapply the runtime and qualifier grants. On a local
HTTPS qualification installation set `web_model_checks = true` under `[server]`.
Run `zenith-relay-hosted qualify-text --watch --vault-dir PRIVATE_DIR` under the
separate qualifier login with the vault settings below. The worker has no
HTTP listener. Keep the installation's [outbound approval](#direct-https-outbound-approval).
An external proxied destination is still unsupported.

In **Connections → API providers**, open a web-saved connection's **Models →
Check model**. Enter the exact upstream ID and reviewed input/output charge
bounds, then confirm with a passkey. These bounds are not a context window.
The worker reads the saved route's catalog and, if the exact model is present,
sends one short non-streaming Responses request. Approval and status reads do
not contact the provider. Scope, connection or credential changes can invalidate
an approval before dispatch. An interrupted check is never sent again
automatically; reopen **Models** to read the result. For this path the closure
bound is 65 seconds and evidence lasts at most one hour within the active scope.

A verified model stays outside the pool until separately admitted. To offer
it to testers, set prices and publish from **Pool**.

### API source lifecycle

In **Admin → Connections**, confirm each change with an owner passkey:

| Action | Result |
| --- | --- |
| **Pause** / **Resume** | Stop new permits and checks, or resume after draining. Membership and current evidence are retained. |
| **Actions → Remove from pool** | Pause and remove all admitted bindings for this source. Keep the key and proxy; previous checks no longer authorize the source. |
| **Actions → Move to trash** | Keep the key and proxy for recovery. Exclude the source from routing and checks. |
| **Restore outside pool** | Return to Connections paused, without previous membership or check authority. To serve again, resume, qualify a new binding, admit it in **Pool → Members**, and set its purchase rates. |
| **Delete permanently** | From trash, after all work drains, revoke this source's provider key and proxy slots together. The source cannot be restored or reused; receipts and historical accounting remain. |

Use **Refresh status** to reread draining work. Accepted requests still settle
at their original rates. A compatible admitted supplier can serve the same
published offer; if none remains, the offer is unavailable until qualified,
priced supply returns. Lifecycle changes do not create or reprice an offer.
After a lost response, **Check result** reads the exact revisioned receipt
without repeating the action.

Removal revokes live vault references, not the credential at its issuer.
Unreferenced ciphertext remains subject to private vault cleanup and backup
retention. Reapply later removals on an isolated restore before any cutover.

### Pool members and activity

In **Pool → Members**, choose **Add member**, select the verified connection
and model, and confirm with a passkey. Open the card's pencil, turn individual
admitted models on or off, then confirm **Save** with one passkey. **Settings**
holds pause, resume and removal for the whole connection. Price and publication
changes stay in **Model rules**.

A disabled model cannot get a new dispatch permit on this source. Already
dispatched requests keep their original accounting. If another eligible supplier
remains, the existing offer stays available at the same price; otherwise it is
unavailable until qualified supply returns. Settings apply to the exact model ID
and survive a new check or readmission of the same source. Re-enabling a model
does not admit, qualify or publish it. If the settings changed elsewhere, review
the refreshed list; if a reply was lost, **Retry** checks the same command.

In **Pool → Members**, open **Pool rotation settings**. **Automatic** uses the
lowest-cost eligible connection. **Primary & reserve** uses the first ready
connection in the saved order, even if another costs less. Reorder by dragging
or using the arrows; confirm **Save** with a passkey. New holds pin the committed
rotation revision. A policy change does not reprice or restart work.

The board shows outstanding dispatches on the actual serving connection. It can
briefly show work draining on a removed member, and never predicts the next one.
Apply the current [runtime-grants.sql](runtime-grants.sql) after migration `0065`.

### Model display order

In **Pool → Model rules**, use the group and model handles to move a whole
publisher group or move a model within its group. A focused handle accepts
arrow keys, Home and End. Collapsing a group is local. The saved order also
applies to the customer's **Models** and **Status** after access filtering.
Hidden models retain their positions. Order does not change tariffs, grants
or execution.

Migration `0062` adds the layout. Apply the current
[runtime-grants.sql](runtime-grants.sql) after migrating.
With the migration credential, run
`zenith-relay-hosted import-catalog-publishers --file REFERENCE.json` to
replace the publisher projection from a trusted local Relay reference catalog.
It accepts the same JSON as Relay's metadata parser; a minimal example is
`{"openai/example-model":{"name":"Example model"}}`. The file must be at most
64 MiB. Only model IDs and publisher IDs are saved. Unmatched models appear
in **Other**; a reference import does not discover or publish models.

### Wallet credits

A local qualification checkout (migration `0033`) lets named testers review a
USD top-up, confirm it once and recover the same payment from **Balance**. The
server saves the amount, fee and quote before a separate worker can claim the
invoice. A lost merchant reply is resolved by lookup; a browser return never
changes a wallet.

**Continue payment** in Balance (migration `0035`) opens the original invoice
after a fresh lookup by the same merchant. An accepted quote need not still be
valid. A paid, closed, expired or unverified invoice gives no checkout link.
Reopening takes no invoice slot and creates no payment or wallet posting.

In **Admin → Payment methods** (`/admin/payment-methods`, migration `0034`),
choose from the sandbox profiles installed by the private payment worker.
A new installation has none. Add a merchant connection, select and order its
methods, and choose who pays the fee from the adapter's supported options.
Set the USD minimum, quick amounts, custom entry and open-invoice limit.
**Save draft → Check draft → Publish** is explicit; the check applies to the
saved revision for ten minutes. Methods with no eligible amounts stay in the
draft but are omitted from checkout. Confirm publication or disabling with the
passkey for that action; no new sign-in is needed. Changing an accepted term
retires the old method ID for new confirmations; accepted invoices retain their
amounts and binding. Reordering unchanged methods or changing the open-invoice
limit keeps their existing quotes.

A personal workspace starts with an empty USD wallet. The private payment
worker accepts a paid invoice only through its configured verifier. The
accepted invoice fixes the wallet and credit; the event must match its merchant
binding, reference, payable amount and currency. Posting, balance update and
receipt commit together. Redeliveries return the original receipt, including
after a restart. A mismatched or unverified event never credits another wallet.

Reapply [runtime](runtime-grants.sql) and [payment worker](payment-grants.sql)
grants after upgrading. A local sandbox worker registers the methods for its
provisioned merchant binding using [`register_sandbox_profile`](src/payments/registry.rs).
For supported handoffs, implement `checkout_origin` and `reopen` and poll
[`resume_next`](src/payments/checkout.rs) independently of invoice creation.
Return a link only for the original invoice under its accepted terms. The
worker checks the exact HTTPS origin; the web process cannot reach the merchant
or reconcile a credit. The handoff expires after six seconds in an unlogged
exchange, is consumed on read, and never enters history. Exclude data in
`relay_hosted.checkout_resume_checks` from logical backups and omit SQL
parameter logging for payment queries. The signed sandbox adapter exists only
in tests.

#### Voluntary refunds

After migration `0036`, reapply [runtime-grants.sql](runtime-grants.sql) and
[payment-grants.sql](payment-grants.sql) before restarting the services.
In **Balance → Payment history**, a tester can request part or all of a
credited sandbox top-up in whole USD cents, excluding the processing fee.
Each payment has at most one open request. The request does not affect the wallet.
In **Admin → Overview → Refunds** (`/admin/payments`), approve or decline the
case with a passkey confirmation for that decision. Approval reserves the amount only
if both the payment's remaining refundable credit and the wallet cover it.
Balances separate inference and refund reserves.

The [private refund adapter](src/payments/refunds.rs) submits a claimed refund
using its durable ID and the original invoice and merchant binding, even if
the payment method has since been disabled. After a lost reply or restart,
page `pending_lookups` and reconcile the same ID rather than submitting again.
Verified success consumes the reserve and posts a refund receipt; a verified
final refusal releases it. An unconfirmed result keeps the reserve, including
in maintenance. The browser cannot call the merchant or post the outcome.
Real merchants, non-USD settlement, fee refunds and disputes require qualification.

### First funded text request

The [text worker](inference.openapi.yaml) serves the models the owner publishes
to the current named testers. Nothing is seeded. The web process cannot read a
provider credential, run a model check or send inference.

1. Save a synthetic source as above and approve a tester scope in
   `/admin`. Create separate `CONNECT`-only qualifier and inference logins.
   As the migrator, apply [qualification-grants.sql](qualification-grants.sql)
   with `psql -v ON_ERROR_STOP=1 -v qualification_role=relay_qualifier -f hosted/server/qualification-grants.sql`
   and [inference-grants.sql](inference-grants.sql) with
   `psql -v ON_ERROR_STOP=1 -v inference_role=relay_inference -f hosted/server/inference-grants.sql`.
   Select each command's own credential using `ZENITH_RELAY_HOSTED_DATABASE_URL_FILE`;
   both workers also need `ZENITH_RELAY_HOSTED_VAULT_DATABASE_URL_FILE` and the vault key file.
2. With the qualifier login, run `zenith-relay-hosted qualify-text --print-scope`.
   Prepare a review file using that scope, the source UUID, a fresh operation UUID
   and the actual model ID and limits from your local synthetic provider. Example shape:

   ```json
   {
     "id": "<new UUID>", "scope": "<printed UUID>", "source": "<vault source UUID>",
     "source_label": "Text fixture", "endpoint": "http://127.0.0.1:9100/v1",
     "model": "<exact upstream ID>", "provider_revision": 1, "proxy_revision": 1,
     "uses_proxy": true, "max_input_bytes": 40000, "max_output_tokens": 30000,
     "tokens_per_byte": 1, "overhead_tokens": 0, "close_after_s": 65, "valid_for_s": 3600
   }
   ```

   The sizing values are the operator-reviewed charge bound, not a measured context
   window. For a source that never had a proxy, use `false` and revision `0`;
   after revocation, use the actual tombstone revision. For local fixtures use
   literal loopback HTTP endpoints and optional loopback HTTP proxies. For
   direct HTTPS see [outbound approval](#direct-https-outbound-approval). Store no
   secrets in the profile.
3. Run `zenith-relay-hosted qualify-text --profile REVIEW.json --vault-dir PRIVATE_DIR`.
   It reads the source's catalog and sends one explicit short Responses check through
   the selected route. A missing model, unusable proxy, incomplete usage, stale
   credential or expired scope cannot add inventory. Repeating the identical completed
   operation does not send again; an unconfirmed send is not retried. The check is
   operator work, never a customer request or charge.
4. Open **Pool → Members** at `/admin/pools`. Add the verified model to the pool,
   then open **Model rules**. Select a source and save its **Purchase prices**
   for input, cache reads and output (USD per million tokens). You can also click
   the model count on the source card under **Connections → API providers** to
   edit the same prices for its admitted models. The **API default markup (%)**
   shown once in that dialog applies across the connection's models. A blank
   default makes no proposal; explicit zero proposes the purchase price.
   - **Fixed prices:** complete all three selling rates and save them.
   - **Use API defaults** or **Model markup:** choose **Review prices**. The
     server takes the highest resulting price per component across admitted
     sources with compatible sizing and complete proposals. A model markup
     replaces, not adds to, the source default. If the inputs change, review again
     before publishing.
   - **Publish to testers** applies the reviewed rates across the scope. The
     model needs at least one admitted, compatible, currently checked source
     with purchase prices. Losing one source does not remove the listing if
     another qualifies. A warning about selling
     below purchase cost does not block publishing. Customers see only selling
     prices. Reductions affect new admissions only; increases require advance
     notices and are currently rejected. Withdrawing stops new sends;
     republishing does not revive pre-withdrawal queued attempts.
5. With the inference login, run `zenith-relay-hosted serve-text --bind 127.0.0.1:PORT --vault-dir PRIVATE_DIR`.
   It can start with an empty catalog and does not need a restart after publication.
   A tester sees effective prices on `/app/models`; their bearer key lists available
   IDs at `GET /v1/models` on the worker. These are reads, not provider probes.
   Changing a credential or proxy invalidates the old observation even if the
   connection later returns to direct mode. Do not expose this listener publicly.

The client sends `POST /v1/responses` with its bearer key, `model`, a text
`input` and `max_output_tokens`. PostgreSQL saves one hold at the selling
tariff and pins the eligible inventory. Automatic compares each ready source's
purchase bound for the same input and output budget, without assuming a cache hit.
The worker skips capped or cooling routes and waits only until the claim deadline
if otherwise eligible capacity is busy. PostgreSQL rechecks the grant, selected
inventory, rate revision and source capacity before authorizing the send.
Reusing the same `Idempotency-Key` and content
returns `409` with the original `X-Request-Id`, never another answer or charge.
Use that originating key on `GET /api/hosted/v1/requests/{id}/status` to read
its accounting state. A conflicting payload returns `409` without an ID.

The worker commits the validated terminal outcome, nullable usage counts and
observation time before settling the wallet. It stores no response body. If
accounting is interrupted, recovery applies the saved observation once against
the original deadline and selling tariff, even when recovery runs after that
deadline. Recovery runs on worker startup and periodically; `reconcile-text`
runs one batch without vault access, including while admissions are paused.

A worker lost before its send claim releases the hold at the claim deadline.
After a possible send, the request is not replayed. Without timely complete
usage, close at the saved deadline using only evidenced priceable components,
or waive the hold. Exact late usage that lowers a partial charge produces one
separate credit at the original tariff. The original receipt is unchanged.
A credit that does not fit in the wallet is retried when there is room. Late
higher usage never posts a second debit. An over-bound result caps the debit
and fences the source observation that served it. A `429` with `Retry-After`
records the provider's wait for future requests to that source and model
without retrying this request.

The physical send permit separately pins the purchase-rate revision, UTC month
and maximum supplier cost. Editing purchase prices affects future permits, not
customer holds or earlier attempts. Terminal usage that fully prices an attempt
records the supplier expense once, even if it arrives after the customer receipt
has closed. Without enough evidence, the cost remains bounded rather than being
counted as zero. A purchase cost above its bound is recorded in full and fences
the route; it never increases the customer's debit.

Migration `0020` withdraws existing text listings that had no purchase rates.
Their selling drafts, historical tariffs and receipts stay intact. Set purchase
prices and republish to resume new sends.

In **Connections → Purchase limit**, leave **No Relay limit** or set a monthly
USD cap for a saved API connection. Spend and outstanding bounds count across
its models, customers and keys, even without a cap. Changing the cap does not
reset the UTC month. If counted exposure plus the next bound exceeds the cap,
that source cannot send. With no ready alternative, the worker releases the
unspent customer hold. The cap does not fund the provider.

Migration `0021` rebuilds monthly exposure from the attempt journal. Claimed
pre-`0020` requests with no purchase record make that source/month unpriced:
new sends are blocked while a cap applies in that month.
Migration `0022` adds versioned markups and selling reviews without changing
previous tariffs. Reapply [runtime-grants.sql](runtime-grants.sql) after
migrating; it includes the owner-only input reader used by the pricing handler.
Migration `0023` adds per-request supplier selection. Reapply
[inference-grants.sql](inference-grants.sql) before restarting the text worker;
it removes the old single-source permit and grants the request-bound one.
Migration `0024` recovers eligible late usage already retained as supplier evidence.
Migration `0025` adds durable JSON terminal capture. Reapply the inference
grants after migration: the worker now captures and reconciles through separate
functions and no longer has access to the direct settlement function.

#### Customer reports

On the normal web server `/app/overview` charts logical requests, recorded
tokens and net posted charges in rolling 24-hour, 7-day or 30-day UTC windows.
Open an interval to see the same population and exact totals in `/app/usage`.
Filter that snapshot by model, API key or execution outcome. **Download CSV**
includes every matching receipt, not just the current page, with UTC bounds,
public key prefixes and original, credited and net microUSD values. Missing
observations and unposted charges are blank. Request starts determine usage
buckets; `/app/billing` uses journal posting time and the wallet read is current.
Refresh creates a new report with later evidence.
**Request details** opens the current receipt. An affected request can show a
customer-safe explanation and an explicit status refresh or support action.
Support copy includes only versions, time, the request ID, public code, coverage
and the request's execution/accounting states. Diagnostics do not enter saved
reports or CSV.

Migrations `0029`–`0030` require reapplying [runtime-grants.sql](runtime-grants.sql).
Reports pin safe receipts for ten minutes, with up to 8192 logical requests,
16 active reports per workspace and 96 per installation. Direct CSV downloads
have an 8 MiB cap and two concurrent renders per web process. Over-limit
requests do not return partial reports. Foreign and expired report IDs cannot
download files. The report cache is disposable; preserve the source journal,
not these short-lived copies, in backups. Report reads never send inference.

#### Model status

`/app/status` follows `/app/models` order and shows the customer's recorded
requests, cache reads and measured completion time for rolling 24h/7d.
The worker measures from the start of dispatch preparation after reservation
to a validated completion, before settlement, for JSON and streaming.
The displayed average covers measured completions only and is not time to first
token. No samples are backfilled from receipts. Missing values stay neutral.
Withdrawal removes the row on refresh. Status reads never contact providers.

After migration `0063`, reapply [runtime-grants.sql](runtime-grants.sql) and
[inference-grants.sql](inference-grants.sql) before restarting their processes.
Migration `0066` also requires the current runtime grants. The web maintenance
loop removes timings after 168 hours and availability slots after their full
interval leaves the 7-day window, without changing usage or balances.

To collect availability, give a dedicated PostgreSQL login (no group
memberships) the [model monitor grants](model-monitor-grants.sql):
`psql -v ON_ERROR_STOP=1 -v model_monitor_role=relay_monitor -f hosted/server/model-monitor-grants.sql`.
Set `ZENITH_RELAY_HOSTED_MODEL_MONITOR_DATABASE_URL_FILE` to its private DSN
file and run `zenith-relay-hosted collect-model-status --prometheus 127.0.0.1:9090`
with the installation's `ZENITH_RELAY_HOSTED_CONFIG`. Use a private Prometheus
listener on numeric loopback. Schedule this one-shot command externally (for
example, every five minutes), after qualifying the independent probe feed.
`serve` does not run it.

The feed is `relay_hosted_model_probe_success{publication_id="<UUID>"}`.
Publish one `1` (healthy) or `0` (failed) per UTC five-minute bin, timestamped
at the actual completion of a separately qualified check for that publication.
Omit unknown outcomes. Preserve the event timestamp; do not re-stamp a cached
gauge at each scrape. Scrape success, catalog checks and customer requests
are not probe results. One unambiguous series must cover all six bins:
all healthy → operational, all failed → unavailable, mixed → degraded.
Missing bins, duplicate observations, invalid values and duplicate series leave
the publication's slot unknown.

The collector uses the database clock and queries raw samples for at most
the last two completed half-hours, via
[Prometheus instant range-vector queries](https://prometheus.io/docs/prometheus/latest/querying/api/#instant-queries).
It sends only currently eligible publication IDs, without credentials, request
retries, redirects or proxies; each response is limited to 128 KiB and each run
to 30 seconds. Malformed or annotated query results fail closed. Output
contains counts only. An accepted slot can be an identical retry; a conflict
keeps the first observation. Previously accepted slots survive a later query
failure.

- `model_monitor_targets(origin)` returns up to 64 configured `items`
  (`model`, `publication_id`, `published_at_ms`), or `{"error":"status_capacity"}`
  without partial results. A target is not evidence of availability. Price edits
  preserve the publication ID; withdrawal and republication replace it.
- `attest_model_status(origin, publication_id, slot_at, state)` accepts
  `operational`, `degraded` or `unavailable` for a complete UTC half-hour.
  `recorded` also acknowledges an identical retained result for the current
  publication. `conflict` means the slot already has a different result;
  `inactive` means the installation/scope/publication is no longer current;
  `invalid` includes pre-publication, incomplete, expired or unobserved slots
  submitted an hour or more after their end. Skip slots without evidence.

#### Native text streaming

For an existing JSON-verified inventory binding, run `qualify-text --profile`
with `{"id":"<fresh operation UUID>","scope":"<active scope UUID>","stream_inventory":"<JSON profile id>"}`.
Use the same qualifier login and vault options. The separate check requires a
nonempty text delta, a matching `response.completed` and complete final usage.
An identical successful check does not send again. A changed credential, proxy,
endpoint or inventory binding does not inherit SSE evidence.

Set `"stream": true` in the same text request. Without a verified compatible SSE
supplier the worker returns `403` without a hold or provider send. A model ID in
`GET /v1/models` does not imply SSE availability. The worker forwards original
events through the first valid `response.completed` or `response.incomplete`.
It records a matching `response.failed` without forwarding the provider error
details. For `relay_outcome_pending` or a broken body, consult the same request's
status; neither means the provider did not run it.

The worker limits upstream requests to 30 seconds, idle body reads to 10 seconds,
events to 1 MiB and streams to 4 MiB. It queues one event, with up to five seconds
per downstream send. A disconnect stops local reading without replay. Nonterminal
usage can settle only as partial evidence at the saved deadline; absent priceable
usage, the hold is waived. Unresolved supplier cost stays counted separately.

After migrations `0026`–`0028`, reapply [qualification-grants.sql](qualification-grants.sql)
and [inference-grants.sql](inference-grants.sql) before restarting the workers.

### Direct HTTPS outbound approval

For an approved external source, add one `[[network.outbound_policy.text_sources]]`
entry to the private server config used by both `qualify-text` and `serve-text`.
Set `source` to the stored source UUID, `endpoint` to its exact
`https://hostname/v1` URL and `ipv4` to a reviewed public address for that host.
Use the same endpoint in the profile and set `uses_proxy` to `false`.
The route uses port 443, validates TLS for the hostname and connects only to
that address; it does not resolve from DNS, follow redirects, or use environment
proxies. Private, special-use and known metadata addresses are rejected.
The deployment's egress firewall must also block locally routed and platform-only destinations.

External proxied sources can be stored but cannot qualify on this path. Local
fixture proxies are unchanged. The observer saves the pin in PostgreSQL.
Before changing or removing a pin, suspend the affected listings and stop the
text workers. Restart them on the same updated config, then run a fresh
qualification with a new operation UUID before republishing on a new pin.
A mismatched route cannot dispatch. The worker can use a compatible approved
alternative or release the unspent hold.
Reapply [qualification-grants.sql](qualification-grants.sql) after migration `0019`.

The loopback installation and named tester restrictions still apply. Use only
synthetic fixtures until the provider, accounting and deployment gates in
the [Hosted design](../../docs/hosted/HOSTED_RELAY_ARCHITECTURE.md#design-decisions-that-must-not-be-guessed)
are satisfied for the intended route.

## Configuration and HTTP

Configuration is read at process start; unknown TOML keys are errors. `server.bind`
requires loopback and a nonzero port. `server.public_origin` is a fixed HTTPS
origin (HTTP only for loopback development). Optional `auth.admin_rp_id` must equal
its exact domain; first-owner setup requires it. Forwarded headers cannot change
either value. Do not use the development origin for a real service or expose
the development listener directly. `branding.name` is 1–64 non-control characters
without surrounding whitespace. Connection limits are required: pool 1–64,
acquisition 100–30,000 ms, statement 100–60,000 ms, lock 100 ms up to the
statement limit. For example a zero pool or `mode = "live"` is rejected.

Set `[policies]` entries `terms`, `privacy`, `refunds` and `support` to reviewed
HTTPS pages for this operator. Omit any unpublished entry. Credentials, query
parameters and fragments are rejected. Relay displays links without fetching
those sites or supplying another operator's policies.

[openapi.yaml](openapi.yaml) defines the public read, routine owner calls and
separate local setup exchanges. Public config contains no installation ID, RP configuration,
wallet, source, role, or customer data. The control API never returns the SPA for
unknown endpoints. The production web build rejects incompatible responses;
development [preview fixtures](../web/README.md) are independent of the server.

## Verify

From the repository root:

```sh
cargo test --locked --manifest-path hosted/server/Cargo.toml
cargo test --locked --manifest-path crates/relay-core/Cargo.toml --test hosted_execution_boundary
```

Set `RELAY_TEST_PG_BIN` to a PostgreSQL **bin** directory containing `initdb`
and `pg_ctl`, then run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test bootstrap --test payments_postgres -- --ignored`.
Each test creates and stops its own temporary SCRAM-protected cluster on loopback;
no test accepts an external database URL. On Unix run as a non-root user. For the
full browser journey, first build the web artifact and set `RELAY_HOSTED_TEST_WEB=1`.
Install the web package's Playwright Chromium or set `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH`.
The test uses a fresh browser profile and a virtual authenticator, not a real passkey.
With the same web build and browser, run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test customer_identity -- --ignored`
for owner approval, customer login and checkout against the built site. It checks
origin, CSRF, session and intent binding, one-use assertions and receipt recovery
after a lost response across qualification and payment actions. The loopback
issuer counts actual calls: busy starts preserve pending attempts; stale and
expired callbacks never exchange a code; expiry during exchange creates no
session. Checkout checks draft isolation, changed revisions, check expiry,
ordered methods, both fee choices,
fixed and custom amounts, cap reduction, and retirement without cancelling
accepted invoices. It also checks quote expiry, lost browser and merchant replies,
cross-wallet reads, the concurrent open-invoice cap and a verified $10.00 credit
for one $10.50 payment, including history after a restart. It reopens the same
invoice twice after quote expiry and rejects cross-wallet, mismatched, expired
and paid handoffs. The same journey checks notice revision and session binding,
rollback, receipt recovery, audience cutoff, private read markers and expiry.
It requests a $2 refund from that $10 credit, verifies the exact decision and
replays a lost approval response with the same operation ID. Retiring the
original method does not stop the refund. A lost merchant reply keeps $2
reserved until lookup posts the refund once. It also checks failure releases
the reserve, declining creates no posting, and payment history stays isolated
across wallets after restart.
The journey also checks suspension, reinstatement and closure through the
owner browser, exact receipt lookup after a lost response, access isolation,
and denial of case reads and writes by a valid paired Desktop.
Run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test owner_recovery -- --ignored`
for concurrent replacement attempts, old-key and session denial, one audit
receipt and an unchanged funded wallet. The normal listener exposes no recovery
route, and the fenced restore refuses the local command.
Run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test owner_passkeys -- --ignored`
for backup enrollment, sign-in with both keys, rejection of foreign-session and
replayed challenges, and competing revocations of the last two keys.
Run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test first_funded_request -- --ignored`
for atomic source/proxy provisioning, idempotent replay, qualification through a counting proxy, owner
publication in the browser, a verified USD 10 credit, a USD 2 hold, a USD 1.20
charge for 50,000 tokens across price changes, a separate USD 0.31 purchase
expense pinned at dispatch, linked customer receipts, monthly cap enforcement,
withdrawal, duplicate delivery and recovery across worker restarts. Model and
limit writes check one-use passkey confirmation, audit attribution and receipt
recovery without re-signing in. Two-source pricing checks exact values, stale
inputs at confirmation and customer visibility. The cap scenario checks a
one-microUSD shortfall, exact fit and outstanding cost after the customer hold
has closed. The browser pauses and resumes an API source, checks a lost response
without a second change, and verifies that a revoked source offers no controls.
Both tests pair a native device, check inventory before/after restart, and revoke
the grant. They use synthetic identities and credentials; neither contacts a real provider or merchant.
Run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test request_selection -- --ignored model_monitor`
for scoped grants, rolling retention, and collector runs against a counting
loopback Prometheus fixture. It covers recovery and outage readings, neutral
gaps, conflicts, republication and status reads with no provider calls.
Run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test desktop_source_copy -- --ignored`
for one-use approval in the owner browser, exact-plan and device fences, a
lost-approval reply, atomic native commit, and qualification through the copied proxy.

With PostgreSQL, run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test request_selection -- --ignored`.
Two counting providers exercise whole-request purchase ranking, cap and capacity
changes between quote and permit, long cooldowns without replay, fixed customer
tariffs, stale and incompatible supply, key revocation and actual-supplier fencing.
The `supply_confirmation_race` filter checks selling edits against permit
acquisition, cap changes against in-flight purchase accounting, and both
orderings of a pause and a physical permit. It verifies the original tariff
through fallback and settlement, unchanged pricing inputs, blocked checks and
receipt replay.
The `customer_access_cases` filter checks closure against waiting admission
and dispatch, release of a queued hold, and exactly-once settlement of a sent
request and an accepted payment without affecting another workspace.
The `refund_and_real_inference` filter checks concurrent admission, the shared
wallet limit, journal rollback and restart recovery of a refund during maintenance
without consuming a simultaneous inference hold.
The SSE cases exercise independent qualification, unbuffered text, terminal
identity, replay, idle timeout, disconnect and recovery after a wallet lock.
The `device_observation_cases::metrics` scenario exercises the Desktop pool
snapshot and speed from paced SSE, including scope, retention and unchanged receipts.
The partial/waiver case waits for the original deadlines (about 65 seconds).
The `usage_report_cases` filter isolates UTC boundaries, tenant scoping, report
limits and a late credit between pages. The funded browser journey checks
chart-to-receipt navigation, filters and the exact CSV before and after a posted correction.
The `owner_overview_cases` filter checks rolling and All time bounds, cross-workspace
aggregation, unknown token totals and late counts before a charge correction posts.
The `owner_usage_cases` filter checks exact boundaries, tied-date pagination,
error membership and expired/customer session denial. The funded browser verifies
chart-to-Usage navigation, owner-only detail, request lookup and the updated
timeline and charge after a correction posts.
The `request_diagnostic_cases` filter covers binding, deduplication, retention,
capacity and separate owner/customer views. The SSE tests verify observations and
the unchanged terminal evidence, journal and send count when the diagnostic write
fails. The funded browser also checks owner diagnostic filters, customer support
copy and the same receipt correction independent of diagnostic history.
Run `cargo test --locked --manifest-path hosted/server/Cargo.toml --test operations_recovery -- --ignored`
for operation writer isolation, missing and stale observations, sink failure,
grouping and expiry. The funded browser journey also checks worker failure and
recovery in Operations without another send or wallet change.
Use the `diagnostic_capture_cases` filter on `--test request_selection` for
capture authority/fences, mutation-free preparation, completed-request rejection,
duplicate and concurrent limits, expiry, audit recovery, web-session reset and
a stream completing with the collector denied. The funded browser confirms one
exact capture, recovers a lost acknowledgement without another sign-in, shows
actual milestones and stops without another send or charge.
The `restore_cases` filter on `--test request_selection` also requires
`pg_basebackup` and `pg_verifybackup` from the PostgreSQL bin directory. It
starts a verified physical backup on a second loopback port after stopping the
original, and checks the restored vault against the separately held key. The
source commits a dispatch, credit, refund, key revocation and customer closure
after the backup. The drill replays the closure from external evidence, checks
idempotency, denied resubmission, preserved holds and receipts, and the fence
across restart. It uses synthetic effects only; it does not measure off-host
WAL delivery, recovery time or recoverable data loss.

With the web build and Chromium, run
`cargo test --locked --manifest-path hosted/server/Cargo.toml --test desktop_source_copy -- --ignored`.
The TLS fixture needs `openssl` on PATH.
The tests cover passkey approval over HTTPS, atomic vault writes, restart recovery
and a qualification run through the copied authenticated proxy. A rejected proxy
produces no direct traffic.

With PostgreSQL, Bun and OpenSSL available, run
`cargo test --locked --manifest-path hosted/server/Cargo.toml --lib -- --include-ignored`.
The outbound fixtures verify TLS and redirect handling, that a connection error
does not replace the pinned client, and that a saved check rejects a changed or
missing pin after restart. They make no external calls.
