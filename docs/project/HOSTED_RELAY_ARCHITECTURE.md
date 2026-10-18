# Hosted Relay Architecture

**Proposal, not shipped behavior.** Current single-owner contracts are in
[PLANNING.md](PLANNING.md); implementation order is in [ROADMAP.md](ROADMAP.md#delivery-stages).

## Decision

Build a multi-user service around `relay-core`, initially as one modular backend
and deployment unit with bounded workers. PostgreSQL owns durable hosted state;
provider secrets use an encrypted vault with keys outside the database.

Reuse the existing protocol, routing, scheduler, and stream semantics. Do not
fork the codec engine or scatter Hosted-mode conditions through the single-owner
server. A separate hosted crate is preferable if its ownership cannot stay clear.
This proposal does not require a desktop or local-server rewrite.

## Product boundaries

- Website and Telegram are clients of one identity/account API, not separate
  balances, user registries, payment truth, or key logic.
- Applications call inference directly with scoped keys. Desktop is an operator
  management client, not a required inference hop.
- Capacity is explicitly platform-shared, workspace-private, or shared by grant.
  Importing a credential does not make it public or commercially usable.
- Provider/product permission, region, credential type, and supported billable
  usage must be reviewed before offering a shared paid route. Working personal
  subscriptions or BYOK keys are not permission to pool/resell them.
- No identity spoofing, sharing concealment, or production Zenith credentials,
  inventory, customer data, or internal billing/routing code.

## Service shape

| Module | Owns |
| --- | --- |
| Identity/workspace | People, linked identities, sessions, ownership, membership |
| Management | Operator pairing, configuration revisions, audit, transfers |
| Inference | Key authentication, grants/admission, `relay-core`, streams |
| Catalog/grants | Public model projection and exact runnable route access |
| Accounting | Tariff snapshots, reservations, journal, settlement/payments |
| Usage | Logical requests, physical attempts, safe user/operator reports |
| Vault/workers | Protected credentials, renewal, bounded jobs/reconciliation |

Inference remains available when website, Telegram, or Desktop is offline.
These are module boundaries, not a requirement to deploy microservices.

## Ownership model

A user is a person; a workspace owns keys, connections, pool policy, grants,
usage, and a wallet. Registration creates a personal workspace. Staff/operator
roles are distinct from workspace membership; collaboration roles can wait.
A grant permits access but cannot override provider entitlement, route features,
physical quota, or health.

Tenant records carry `workspace_id`; references use tenant-scoped composite
foreign keys. Server-side action authorization is mandatory even with RLS.
Workspace context is transaction-local and never trusted from request JSON.
Private credentials cannot be listed, exported, or used across tenants.
Ownership transfer is audited, not a label change.

## Identity and account channels

Federated channel linking is later work, not an alternative to the initial
[passkey-only administrator sign-in](#passkey-only-administrator-access).

External identities have a unique `(provider, subject)` linked to internal
`user_id`. Linking requires an authenticated account or approved one-time
challenge, never a matching display name/email. Telegram IDs and payment
callbacks are not authentication by themselves.

Browser sessions use secure HTTP-only cookies and CSRF protection. Sessions
are bounded/revocable; recovery and unlinking cannot orphan the owner.
Payment intents resolve the owning workspace server-side.

## Authentication and keys

| Credential | Boundary |
| --- | --- |
| Account session | Human account operations |
| Inference key | One workspace's model/request scope; no management access |
| Operator session | Action-scoped management, preferably device-paired and short-lived |

Inference keys have a non-secret prefix and high-entropy secret with a one-way
verifier. Show the secret once. Record scope, assigned user when applicable,
limits, expiry, revocation, and last use. Rotation creates a replacement first;
revocation affects new requests without rebuilding runtime. A key narrows grants,
never broadens them. Provider keys never authenticate Relay clients.

### Passkey-only administrator access

The initial human sign-in to the Hosted administration panel is passkey-only
through WebAuthn/FIDO2. Google/OIDC login, passwords, email/SMS sign-in, Telegram
authentication, and wallet signatures are outside this first implementation.
A passkey stored in Google Password Manager is not Sign in with Google.
This is planned behavior; the current single-owner server's management-token
authentication and Desktop pairing remain unchanged until explicitly migrated.

**First owner.** Provision the installation owner through an explicit server-local
setup operation, not the first public registration. A short-lived, one-use setup
authorization binds the installation, setup session, and first credential
registration. Do not put it in URLs, logs, or committed configuration. Verify the
WebAuthn registration before atomically consuming setup authorization and creating
the owner/credential binding. Concurrent setup, failure, or restart must not create
another owner or reopen completed setup. A passkey proves identity, not authority:
server-side owner/admin roles authorize every management action; workspace ownership
does not grant installation-wide administration.

**Registration and sign-in.** Use a maintained server-side WebAuthn library, not
custom signature verification. Configure a stable RP ID and an explicit HTTPS
origin allowlist; never derive trusted origins from untrusted proxy headers.
Use discoverable credentials for sign-in without an email/password. Server-issued
cryptographically random challenges have bounded lifetimes, bind the operation
and setup or authentication session, and are consumed atomically once, including
failed verification attempts. Verify challenge, ceremony type, origin, RP ID,
credential/user binding, signature, and user-presence/user-verification flags.
Require user verification for administrator credentials and assertions; unsupported
authenticators fail clearly rather than falling back to weaker authentication.

**Storage and sessions.** Store credential IDs, public keys, opaque user handles,
owner bindings, library-required counter/backup metadata, and revocation state in
PostgreSQL. Relay never receives or stores a passkey's private key. Follow the
library's synced-passkey and counter semantics; a zero or unchanged signature
counter alone is not universal proof of replay. Successful verification creates
a fresh bounded, server-revocable session with Secure/HttpOnly cookies, an explicit
SameSite policy, and CSRF protection. Recheck authorization on each action. A
customer inference key or provider secret cannot create an administrator session.

**Privileged changes and recovery.** Require fresh passkey user verification for
adding/removing passkeys, changing administrators, exporting credentials, and
changing payment/security settings. Offer a second passkey during setup; do not
remove the last usable credential without a verified replacement or explicit
recovery. If all passkeys are lost, recovery requires an audited server-local
operator procedure, revokes existing sessions and lost credentials, and enrolls
a replacement through a new bounded ceremony. No public reset endpoint, permanent
master-key login, email/SMS bypass, or automatic trust in a matching email address.
Bound pre-authentication requests and retained challenges; audit credential and
role changes without storing assertions, setup secrets, or session tokens.

Before exposing the panel, acceptance must establish:

- Setup authorization is single-use and expires; concurrent setup and restarts
  cannot take over an existing installation or leave an owner without a credential.
- Wrong origins/RP IDs, missing user verification, invalid signatures, expired or
  replayed challenges, and foreign/revoked credentials cannot establish a session.
- Supported synced and device-bound passkeys work, including supported zero-counter
  behavior; key registration/revocation cannot cross users or elevate roles.
- Session fixation, CSRF, revocation, stale privilege checks, and missing fresh
  verification cannot bypass management boundaries. Recovery and loss of the last
  credential preserve an accountable owner without enabling an alternate login.
- Inference keys stay non-administrative, existing personal-server authentication
  is not silently changed, and diagnostics/backups expose no private auth material.

Implementation references: [WebAuthn](https://www.w3.org/TR/webauthn-3/),
[server-side registration](https://developers.google.com/identity/passkeys/developer-guides/server-registration),
and [server-side authentication](https://developers.google.com/identity/passkeys/developer-guides/server-authentication).
Recheck the selected library and applicable specification at implementation time;
these references do not commit Relay to a JavaScript backend or a Google login.

## Pools, connections, and model access

A route is available only under the authenticated grant/key, an enabled member,
compatible endpoint/credential/features, and current quota/health policy.
`GET /v1/models` projects that key's usable public IDs. Separate account metadata
may expose supported context modes, limits, and customer prices, not private
upstream inventory or another tenant's credential state.

Effective input limits combine model/route evidence, grant, key, and service
limits; output is independent. Unknown hard limits are not advertised guarantees.
Route identity includes public/upstream model, endpoint, protocol, adapter,
evidence, and revision. Aliases/protocol bindings do not multiply physical capacity.

## Provider onboarding and native API surfaces

A connector records provider/product, credential kind, region, exact API/version,
features, limits, usage/quota method, pricing, permission review, and revision.
Revalidate its official contract at implementation time; provider/example URL
tables are not evidence that a model or plan supports a surface.

Native upstream and client ingress are separate: permitted ingress bridges use
the existing four codecs to reach the chosen model's exact native surface.
Unsupported required semantics fail before dispatch. A route may require
`native_only`; this is not a global prohibition on ingress conversion.

Versioned model mappings are explicit. No cross-provider/model substitution or
fallback. Credential rotation stays within the same provider, product, model,
surface, compatible capabilities, and existing retry contract. Relay keys are
replaced with only the selected vault credential at the provider boundary.

Do not sell a route without permitted commercial use, supported billable usage,
a valid tariff, and finite enforceable reservation bounds. Otherwise keep it
private/BYOK-only where permitted, or disabled for paid serving.

## Request lifecycle

Keep durable execution and accounting state separate:

```text
execution: received → admitted → reserved → dispatched → streaming
           → completed | failed | cancelled | uncertain
accounting: none → held → settled | released
                      → held_for_reconciliation → settled | released
```

1. Bound parsing/connections and authenticate key/workspace before lookup.
2. Validate request, grants, route features, context/output caps, idempotency,
   and tariff. Size prepared input before the reservation transaction.
3. Atomically create the request and bounded reservation; no transaction spans
   provider I/O or streaming.
4. Acquire bounded physical capacity and execute through a server-created core
   scope. If the selected route changes cost bounds, adjust/revalidate the hold
   before dispatch, not after execution.
5. Capture upstream usage before conversion and validate completion. Record one
   logical outcome and every dispatched attempt, including uncertain failures.
6. Commit reusable continuation state before advertising terminal success.
   Settle once and release unused funds. Disconnect cancels supported upstream
   work but does not prove non-execution or zero expense.

### Reconciliation and estimated settlement

Undispatched requests release immediately. Missing final usage enters a finite,
snapshotted reconciliation deadline; worker retries/restarts cannot extend it.
Final supported usage received before closure takes precedence. At deadline:

| Evidence | Closure policy |
| --- | --- |
| Valid cumulative partial usage | Settle supported observed components as `provider_partial`; missing components remain unknown. |
| No substantive output forwarded | Launch default: release as `policy_waiver`; provider expense may still exist. |
| Substantive output, no usable final usage | Only the published bounded stream-estimation policy, marked `settled_by_stream_estimate`, capped by the hold. Unsupported hidden components are waived, not invented. |

Persist deadline, policy/estimator version, observation watermark, substantive
output counters, and observation gaps. Headers/heartbeats are not substantive
output, and transport writes do not prove human receipt. Overdue recovery uses
the original deadline. No route may leave a hold indefinitely.

Late usage improves expense evidence without reopening the reservation. Under
the original tariff, a lower customer amount creates one compensating credit;
a higher amount is operator exposure, not an automatic extra debit. Markup
differences alone are not a refund. Closure does not make uncertain inference
safe to repeat.

### Idempotency and SDK error responses

Authenticate first. Scope the digest of `Idempotency-Key` to workspace, key,
and endpoint; bind it to the canonical incoming request and snapshot effective
defaults. Enforce uniqueness in the request/reservation transaction.

v1 retains no generated body for duplicate replay. Planned duplicate POSTs are
non-2xx protocol-shaped errors, decided before SSE success headers:

| Condition | HTTP | Planned code |
| --- | --- | --- |
| Same key, different request | 409 | `relay_idempotency_conflict` |
| Running or uncertain | 409 | `relay_request_in_progress` |
| Terminal, original body unavailable | 409 | `relay_request_result_unavailable` |

Use the ingress protocol's error envelope, `no-store`, safe request/error-code
headers, and `x-should-retry: false`. Test supported SDKs with default retry and
ignored hints; duplicates must still have no execution/accounting side effects.
Never return empty successful JSON/SSE or suggest a new key to recover an answer.

A scoped Relay status endpoint exposes execution/accounting states, usage
completeness, settlement basis, and currency/precision, not prompts, answers,
continuation content, credentials, or foreign metadata. Pending amounts are not
zero. Publish a durable deduplication window covering execution/reconciliation/
supported retries. Without a key, each POST is a new request; prompt similarity
cannot establish duplication. Existing core retry safety still applies.

## Durable continuations and restart recovery

The initial schema needs tenant/key-scoped `continuation_sessions`: origin
request/attempt, provider/product/member, model/endpoint/protocol, route and
adapter version, authorization generation, turn/version, call correlation,
expiry/state, and optional encrypted payload reference. Scoped lookup digests
are not authentication.

Native blocks/signatures/replay material require an explicit encrypted-state
retention policy, with independent access, keys outside PostgreSQL, byte/TTL
bounds, deletion, and schema version. They are not usage/audit records or
generated-response replay. Staff usage screens cannot read them. When content
retention is forbidden, use supported provider references or complete compatible
client history, otherwise reject resume. Metadata alone is not restart-safe replay.

Commit a completed-turn checkpoint before terminal success. Earlier streamed IDs
remain pending; partial tool arguments are not resumable. Restore only committed,
unexpired state after key/grant/owner/authorization/version checks. Restart is not
reauthentication; refresh preserves ownership only under the connector contract.

Advance turns with short version-CAS transactions and fenced execution leases.
Concurrent distinct turns cannot overwrite state; deliberate branches need child
identities. Expired leases imply uncertain execution, not permission to resend.
Graceful upgrades stop admission and drain/flush under a deadline. Abrupt restart
cannot preserve sockets or unobserved output; uncertain work enters reconciliation.
Persist hard quota/retry deadlines separately from disposable affinity/cache hints.

## Customer balance and provider expense

An append-only double-entry journal owns money; balances are reconstructible
projections. Use one explicit wallet currency and checked integer subunits.
Choose precision, rounding, and tariff before paid traffic. Corrections are
compensating entries, not edits/deletions. Keep customer charge, physical provider
expense, provider quota, and operator pricing/margin distinct.

Reservation has two stages:

1. Size the actual prepared request after bridge/continuation expansion using
   a supported counter or versioned conservative route-specific text/media bound.
   Account for the most expensive possible context band. Bytes are not exact
   tokens; the model's whole context window is not this request's input size.
2. Resolve a client output limit or the key/workspace/service default and a
   finite enforceable route bound. This applies to every paid protocol. Include
   supported reasoning/cache/media/tool costs without double-counting components.

Snapshot tariff, sizing method/result, context band, output cap, and calculation
inputs. Unsupported/unbounded requests or missing tariffs cannot enter paid
dispatch. Stopping visible output does not prove stopped provider billing.
Settle one logical customer charge from supported evidence; each physical
attempt retains separate expense/usage even when it fails.

### Estimation risk and provider expense

Stream estimates are customer policy, not confirmed usage/expense. Retain:

- final/partial/missing usage completeness and nullable component observations;
- the logical settlement reference and its basis/tariff/cap;
- estimator version, sizing/output counters, and observation gaps, without bodies;
- confirmed/estimated/unknown provider expense and its currency/evidence;
- divergence risk and bounded reasons for unobserved reasoning/cache, missing
  final usage, estimates, gaps, or waiver.

Merge cumulative snapshots once. Unknown expense cannot produce confirmed profit.
Compare matching currency and request/attempt coverage; preserve original
estimates as audit evidence after reconciliation.

### Wallet transactions and contention

Use short `READ COMMITTED` transactions with guarded updates, not a separate
balance read and unconditional decrement. The reservation update checks the
authenticated wallet/workspace/currency, positive amount, and `available >= hold`.
The unique operation/idempotency claim, wallet update, reservation, balanced
postings, and request price snapshot commit together or all roll back.

Settlement/release conditionally closes the exact held reservation once. For
hold `H` and charge `C`, require `0 <= C <= H`, remove `H` from reserved, and
return `H - C` to available. Release uses `C = 0`; a verified zero tariff may
have zero hold, but missing pricing may not. Losing concurrent closers read the
committed result instead of decrementing reserved again.

Use uniform lock order and bounded lock/statement timeouts. Retry whole
transactions for supported transient failures. A lost commit acknowledgement
requires same-operation lookup, never inference resend. All credit/refund/adjustment
and spend-counter writers use this accounting owner. Measure wallet contention;
`READ COMMITTED` does not remove locks or make arbitrary multi-row decisions safe.

Payments follow server intent → signed verified event → idempotent journal
credit → receipt. Provider event IDs are unique. Callbacks cannot choose the
workspace/amount. Duplicate, delayed, refunded, and out-of-order events have
explicit transitions; administrative money changes require actor/reason/reference.

## Usage and operator views

Store tenant/key-scoped logical outcomes and separate physical attempts with
route revision, execution certainty, usage, expense, retry reason, and risk.
Never sum the logical charge once per attempt. A shared key identifies a key
and workspace, not the human who used it; individual attribution needs assigned keys.

User views are tenant-scoped. Privileged changes/reads are audited. Operational
logs, audit, usage, expense, and journal have separate access/retention. No raw
prompts, responses, cookies, authorization, or provider bodies by default; authorized
continuation content is the separate policy above. Metrics labels are bounded,
not key/user/prompt/arbitrary-input labels. Deletion retains required financial
records while removing eligible personal data.

## Local and hosted pool transfer

Use a revisioned preview/apply/recovery operation, not implicit synchronization.
Manifests contain portable settings, route declarations, rules, and credential
references, not user wallets, usage, keys, sessions, or conversations.

Static API sources default to `copy`. OAuth defaults to `stage_and_drain`:
atomically drain/disable the local refresh owner before transferring its secret.
Secrets move only after confirmation over HTTPS directly into the destination
vault. Unsupported transfers require destination sign-in.

Validate/apply the target revision and return per-item outcomes. Failure before
apply rolls back Hosted and restores the drained local account; success keeps
the source drained until an explicit completion/restore. Delete the source only
after operator verification and confirmation. Reverse transfer follows the same
rules. No newer revision or opaque conversation state is overwritten/transferred.

## Data and deployment

PostgreSQL owns identities/workspaces, grants/keys, pool/route revisions,
wallet/journal/reservation/payment records, request/attempt/audit/import records,
and continuation metadata. Initial migrations include tenant-scoped references,
deduplication, reservation closure uniqueness, and continuation versions/expiry.
This is a domain inventory, not a prescribed table-per-noun schema.

Start with one versioned API image, bounded workers, PostgreSQL, encrypted vault,
and HTTPS; Docker Compose is the installation contract. Keep database and secrets
off public ingress and out of committed images/configuration. Choose region from
provider permission/availability, user latency, residency, payments, and operations;
narrow the launch set instead of assuming one region supports every provider.

Back up consistent database/vault/key material and authorized continuation state.
Restore in isolation, validate references/balances/revocation/expiry, reconcile
in-flight money, then accept traffic. Backups do not extend TTLs or revive revoked
keys. Caches are not identity/wallet/grant authority. Multiple inference nodes
require shared admission/member leases, durable ownership, and idempotent usage.

## Capacity and server sizing

No unmeasured hardware-to-user/generation promise. Distinguish registered users,
active users, concurrent generations, and request shape. Effective capacity is
the minimum of server/database/worker capacity, eligible physical-member limits,
provider quota, workspace limits, and key limits. Shared upstream principals
need a shared quota fence; aliases and protocols are not extra slots.

Bound per-key/workspace/global concurrency, queue age, bodies, stream duration,
and reservations. Select limits from the real request mix and measure active
streams, p95 admission/DB/lock waits, 429s, CPU/memory/network, settlement lag,
and vault/refresh failures. Add another inference node only after its shared-state
and recovery contracts exist, not from a speculative server-price table.

## Delivery and acceptance

The order is in [ROADMAP.md](ROADMAP.md#delivery-stages). Paid launch remains
blocked until acceptance establishes:

- Linked channels share identity/state; tenant/key isolation prevents enumeration,
  cross-workspace reads, grants, continuation reuse, and spending.
- Physical capacity/quota is counted once; model/context projection matches grants
  and exact runnable provider surfaces without cross-vendor substitution.
- Real PostgreSQL tests cover same-wallet contention, duplicate closure/payments,
  failed postings, rollback, deadlock/timeout retry, and lost commit acknowledgements.
- Cancellation/late usage/restart settles once or remains explicitly uncertain;
  reconciliation deadlines release holds and preserve estimate/expense uncertainty.
- Supported SDK JSON/SSE duplicate requests receive valid non-2xx envelopes,
  including ignored retry hints, without another hold or generation.
- Graceful/abrupt restart restores only valid committed completed turns, including
  turns awaiting tool results. Pending, expired, revoked, foreign, or unavailable
  payload state fails without content retention or replay being silently enabled.
- Routes have reviewed commercial permission, region, billable usage, tariff,
  finite bounds, and real-client/provider qualification.
- Transfers and isolated backup/restore recover without deleting source credentials;
  logs, exports, operator views, and metrics expose no forbidden content/secrets.
