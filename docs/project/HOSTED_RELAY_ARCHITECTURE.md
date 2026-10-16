# Hosted Relay Architecture

Status: proposed target design. This document does not describe shipped hosted
behavior. The current implementation and open work remain in
[PLANNING.md](PLANNING.md) and [ROADMAP.md](ROADMAP.md).

## Decision

Build Hosted Relay as a multi-user service with one identity and account API
shared by the website, Telegram, and future entry channels. End-user
applications connect directly to its inference API with user-scoped Relay API
keys. The Relay desktop is an operator control client, not a required hop for
user requests.

Use PostgreSQL as the durable source of truth for hosted identities,
ownership, grants, key metadata, configuration revisions, request attribution,
usage, continuation ownership, and Relay's own customer accounting. Keep
provider credentials in an encrypted vault whose encryption key is not stored
in PostgreSQL. Any retained continuation payload has a separate, explicit
encrypted-state policy; it is not part of usage history or response replay.

Start as a modular monolith: one versioned service boundary and one deployment
unit for the API, with bounded background workers. Keep local Relay and its
single-owner server behavior separate while hosted contracts are proven.
Reuse `relay-core` for provider routing, protocol adapters, pool scheduling,
stream validation, and request semantics. Build the hosted identity,
authorization, PostgreSQL, accounting, and management layers around it. Do
not fork protocol behavior or import Zenith production billing rules.

This is a rebuild of the hosted control plane, not a reason to rewrite the
desktop application or the protocol engine. If the current SQLite-backed
`relay-server` cannot keep a clear ownership boundary, introduce a separate
hosted server crate rather than scattering hosted-mode branches through the
single-owner server. Consolidation can be considered after hosted acceptance.

## Product boundaries

- A person can use the website, Telegram, or another supported channel to
  access the same Relay account, keys, balance, model grants, and usage.
- A compatible client calls Relay's public API directly. It does not need a
  browser session, Telegram bot, or desktop process.
- The desktop authenticates as an operator and manages the hosted service:
  provider connections, pools, models, grants, limits, users, audited balance
  adjustments, and operations.
- The hosted service can serve operator-owned shared capacity and
  workspace-owned private capacity. Ownership and sharing policy are explicit;
  importing a credential never makes it globally available by default.
- Relay does not conceal account sharing, spoof provider identity, or promise
  that a provider permits a particular shared-account arrangement. Every
  connector and pooling mode needs a permission review before being offered.
- Separate official provider API credentials from consumer subscriptions and
  OAuth identities. A paid shared pool may use only a provider product and
  credential whose applicable terms permit serving Relay customers. A valid
  key or working endpoint is not proof of that permission. BYOK credentials
  remain private to their workspace and are never silently pooled or resold.
- The first release may run one service instance with several workers and pool
  members. Cross-node scheduling is not required, but database and ownership
  boundaries must not assume that one process owns durable state.

## Service shape

Keep these modules in one hosted backend initially. They are security and
ownership boundaries, not a mandate to deploy microservices.

| Module | Responsibility |
| --- | --- |
| Identity | Relay users, external sign-in identities, sessions, account linking |
| Workspace | Ownership, membership, roles, pool and billing scope |
| Management API | Desktop operator actions, configuration revisions, audit |
| Inference API | Protocol endpoints, key authentication, admission, streaming |
| Catalog and grants | Public model identity, executable routes, per-workspace access |
| Accounting | Price snapshots, reservations, settlement, credits, refunds |
| Usage | Logical requests, physical attempts, user and operator views |
| Credential vault | Encrypted provider credentials and controlled transfer |
| Workers | Refresh, payment events, cleanup, reconciliation, bounded jobs |

The request path is:

```text
Website / Telegram / future account clients
                  |
             Identity API
                  |
     shared account, key, balance, usage APIs

Compatible client -- Relay API key --> Inference API
                                      |-- identity and scope
                                      |-- grant, quota, balance admission
                                      |-- pool scheduler and relay-core
                                      |-- provider credential vault
                                      |-- usage and settlement

Relay desktop -- operator session --> Management API
                                      |-- pools, providers, models, grants
                                      |-- users, limits, activity, audit
```

The website and Telegram bot are clients of the same domain operations. They
must not keep separate balances, user registries, payment truth, or key logic.
The inference API must remain available when those interfaces or the desktop
are offline.

## Ownership model

Use a person identity and a billing/ownership boundary as separate concepts:

- **User** is a human identity that can authenticate through one or more
  linked channels.
- **Workspace** owns provider connections, pool policy, model grants, API
  keys, usage scope, and a wallet. A newly registered person receives a
  personal workspace automatically.
- **Membership** links users to workspaces with explicit roles. At launch, a
  personal workspace has one owner and does not need invitations or shared
  credentials. Keep platform staff roles separate from workspace memberships;
  add workspace roles only when shared workspaces are a real product need.
- **Pool** is a capacity and policy boundary owned by the platform operator
  or a workspace. It contains physical provider members and configured source
  routes.
- **Grant** allows a workspace to use a model or pool. A grant cannot create
  a route or override provider entitlement, physical quota, protocol
  capability, or health.

Keep all authorization server-side and action-specific. Platform staff actions
use a separate privileged identity and are audited. If workspace collaboration
is added later, define distinct roles for provider management, billing,
credential transfer, and read-only access; never infer one permission from
another. Provider secrets are not returned to any role.

Every tenant-owned row carries `workspace_id`. References between tenant rows
use composite foreign keys including that ID, preventing a valid object ID
from being attached across workspaces. PostgreSQL row-level security can add
defense in depth, but application authorization remains mandatory. Set
workspace context transaction-locally; never trust a workspace ID from a
request body as authority.

For a personal-workspace-only release, keep the workspace abstraction anyway.
It makes key scope, imported pool ownership, accounting, and future
multi-person access consistent from the beginning.

## Identity and account channels

All sign-in methods resolve to one internal `user_id`. Store external
identities as `(identity_provider, provider_subject, user_id)` with a unique
constraint on the provider and subject.

- Website authentication creates a Relay session using secure, HTTP-only
  cookies and CSRF protection for state-changing browser requests.
- Telegram authentication proves control of a Telegram identity through a
  verified bot flow or short-lived linking challenge. A raw Telegram user ID
  supplied by a caller is not authentication.
- Linking another identity requires an already authenticated Relay session or
  a one-time challenge approved by the existing account. Do not merge accounts
  automatically by display name, phone number, or an unverified email.
- Sessions are revocable and have bounded lifetime. Account recovery and
  identity unlinking must not orphan the workspace owner.
- Payment callbacks identify a payment, not a person. The server resolves the
  payment intent to the owning workspace before crediting its wallet.

The Telegram bot can remain an entry and payment channel, but all balance
changes go through the same idempotent accounting API used by the website.

## Authentication and keys

Separate three credentials:

1. **Account session** authenticates a person to website or Telegram account
   operations.
2. **Inference key** authenticates a compatible client to public model and
   generation APIs. It is scoped to exactly one workspace, has explicit
   permissions and limits, and cannot call management endpoints.
3. **Operator session** authenticates the desktop to management APIs with
   operator or workspace roles. Prefer short-lived device authorization or
   OAuth-style pairing over a permanent shared management token. Store its
   refresh credential in the OS credential store.

Inference keys have a non-secret lookup prefix and a high-entropy secret. Store
only a one-way verifier for the secret; show the complete key once at
creation. Record its workspace, creator, label, allowed models/protocols,
optional spend or request limits, creation, last-use, expiry, and revocation.
Key rotation creates a replacement before the old key is revoked. Revocation
must take effect for new requests without waiting for a runtime rebuild.

A key may narrow workspace access but cannot broaden it. User identity,
workspace, roles, pricing, and grants are resolved by the server from the
authenticated session or key. Ignore client-supplied identity fields for
authorization and billing.

Keep inference, account, payment callback, and management routes on separate
authorization policies. A request key is never a management credential; a
provider key is never a Relay client credential.

## Pools, connections, and model access

Each physical provider account or API source belongs to one pool and has one
ownership classification:

- platform-owned shared capacity;
- workspace-owned private capacity;
- explicitly shared workspace capacity with named grants.

Moving a connection between owners is an audited transfer, not an edit to a
label. Secrets are referenced through the vault. A workspace cannot list,
export, or use another workspace's private connection. Operator-managed shared
capacity is visible as shared capacity, with an explicit policy for who may
use it.

Keep the public model catalog separate from route eligibility. For a workspace
and protocol, a model is available only when all of these are true:

- a workspace grant permits it;
- at least one granted pool contains an enabled physical member for it;
- the source credential and endpoint are valid for the selected upstream
  protocol;
- required input, output, tool, image, reasoning, and transport features are
  supported by the exact route;
- account quota, route health, and operator policy permit dispatch.

`GET /v1/models` returns only the compatible public model IDs available to the
authenticated key. A separate Relay account/catalog endpoint may return
extended metadata such as context modes, output limits, supported protocols,
features, and customer price. Do not expose private upstream model names,
credential state, or another workspace's inventory in the public catalog.

Treat short and long context as named user-facing modes resolved to numeric
limits for the exact grant and route. Enforce the effective input limit as the
minimum of model evidence, source/route capability, workspace grant, key
policy, and service safety limit. Enforce output independently. A model name,
catalog row, or marketing label cannot establish a long-context route. Unknown
hard limits remain unknown and must not be advertised as guaranteed capacity.

The selected route stores its public model, upstream model, endpoint,
protocol, adapter, capability evidence, and configuration revision. Protocol
adapters for one physical member do not create extra capacity, quota, or
rotation weight.

## Provider onboarding and native API surfaces

Treat each provider product and API surface as a versioned connector contract,
not just a base URL and key. A source records its provider and product, credential
kind, upstream region and endpoint, native API surface/version, model inventory,
supported features, hard context/output limits, usage fields, quota method,
price schedule, configuration revision, and permission-review state. Secret
material is referenced through the vault.

For this hosted product, keep two policies separate:

- **Native upstream only**: the selected model is executed through the
  provider's documented native request, response, streaming, error, and usage
  contract. Relay never silently substitutes an OpenAI model with a Kimi,
  Gemini, or other vendor model, and never uses a different vendor as a
  fallback just because the first route failed.
- **Bridged ingress allowed**: the client may use any of the four supported
  public contracts (Responses, Chat Completions, Anthropic Messages, or
  Gemini). `relay-core` converts that ingress contract to the selected
  model's native upstream protocol and converts the response back. The bridge
  must reject the request before dispatch when a required field, feature, tool,
  continuation, or usage semantic cannot be represented without loss.

Therefore a Chat Completions request may reach a model whose native upstream is
Responses, while the actual provider call remains a native Responses call.
This is a protocol bridge, not a cross-provider substitution. A provider's own
OpenAI-compatible wire format is still native pass-through when Relay targets
that provider directly.

Bridging is enabled by default only when the route's capability contract and
pricing policy support it. A paid route may set an explicit `native_only`
constraint on its `model_grant` or route revision when protocol conversion is
undesirable for its usage or accounting semantics. `native_only` is a
route-level policy, not a global Hosted-server prohibition.

Initial surface candidates, subject to revalidation against the provider's
official API contract during implementation:

| Provider | Native surface candidate | Relay rule |
| --- | --- | --- |
| OpenAI | Responses API, `/v1/responses` | Upstream remains Responses; supported client contracts may bridge into it unless the route is `native_only`. |
| Google Gemini | `generateContent` and `streamGenerateContent`; evaluate Interactions as its own surface | Upstream remains Gemini; do not bridge between separate Google products or surfaces implicitly. |
| xAI / Grok | xAI Responses API | Select an explicit xAI surface per route; ingress bridging does not make xAI interchangeable with OpenAI. |
| Moonshot / Kimi | Kimi Platform API, `/v1/chat/completions` | Preserve Kimi-specific fields and behavior; keep API Platform credentials distinct from other Kimi products. |
| Z.ai / GLM | Official PaaS `/api/paas/v4/chat/completions` API | Preserve the documented GLM request and response; ingress conversion must not change the upstream vendor. |

These paths are connector examples, not a promise that every account, model,
region, or API plan supports them. Expose a provider-specific base URL or
explicit namespace so identical paths such as `/v1/responses` cannot select an
upstream by guesswork. Model discovery is scoped to the authenticated key and
that public API surface. Do not infer provider identity from a model string.

At ingress, never forward a Relay inference key to a provider. Replace it only
with the selected vault credential, bridge the client contract when the route
allows it, preserve the provider-native payload and response semantics, and
reject unsupported required features before dispatch. Do not rewrite model IDs
except through an explicit, versioned route mapping. Return provider-scoped
errors and usage; do not silently substitute another vendor's model. Rotation
may select another credential only inside the same provider, product, model,
native surface, and compatible capability contract, and only under the request
retry rules below.

Do not make a route sellable until its commercial-use permission, regional
availability, native usage extraction, customer tariff, and finite reservation
bound are validated. Subscription quota is not a billable usage record. If a
connector cannot produce supported billable usage, keep it private/BYOK-only
where permitted or disable paid serving; do not estimate a customer charge
from an unrelated provider's pricing.

## Request lifecycle

Every generation is a durable logical request with zero or more physical
attempts. Keep execution state separate from accounting state:

```text
request:     received -> authenticated -> admitted -> reserved -> dispatched
             -> streaming -> completed | failed | cancelled | uncertain
pre-dispatch: rejected | failed
reservation: none -> held -> settled | released
                         -> held_for_reconciliation -> settled | released
```

1. Apply TLS, connection, body-size, and parser limits before expensive work.
2. Authenticate the inference key and resolve `workspace_id`, `key_id`, and
   allowed scope from server state.
3. Validate the protocol, model ID, request shape, stream controls, and
   idempotency semantics. Reject unsupported required features before dispatch.
4. Check workspace grant, key scope, rate/concurrency limits, requested
   context, output cap, current route eligibility, and the customer's price
   schedule. Perform the pre-dispatch input sizing described below before
   opening the reservation transaction.
5. In a short PostgreSQL transaction, create the request and reserve the
   stage-two amount calculated from the selected route, context band, input
   estimate, and effective output cap. Do not keep a database transaction open
   while waiting for a provider or streaming to a client.
6. Acquire bounded scheduler capacity and select an eligible physical member
   under pool health, quota, fairness, affinity, and rotation rules. A shared
   account's capacity and provider quota are counted once across all users.
7. Execute through `relay-core` with a server-created scope containing the
   workspace, request, permitted members, model, protocol, and limits. The
   client cannot replace this scope.
8. Preserve native provider semantics where supported; otherwise use a typed
   adapter that rejects required fields it cannot represent. Capture upstream
   usage before output conversion, validate terminal stream completion, and
   retain safe partial-usage observations.
9. Write one logical outcome and each actually dispatched attempt. Commit
   reusable continuation checkpoints before advertising terminal success.
   Settle the reservation idempotently from the usage evidence and snapshotted
   pricing/closure policy, then release unused reserved funds.
10. On disconnect, cancel upstream work where supported. Persist cancellation,
    partial usage, and uncertain execution distinctly; do not report success
    just because an HTTP connection closed cleanly.

### Reconciliation and estimated settlement

Release an undispatched request's reservation immediately. Settle a valid
terminal usage outcome under its tariff and release the unused remainder.
Otherwise enter `held_for_reconciliation` with an absolute deadline and the
route's snapshotted closure policy. The policy sets a finite TTL, normally
5-10 minutes from entry; retries of the reconciliation job cannot extend it.
Final supported usage takes precedence if it arrives before closure. At the
deadline apply this evidence order:

1. If validated cumulative usage was captured from stream frames, use the
   reported components under the partial-usage policy and release the unused
   reservation. Mark the basis `provider_partial`; a cumulative snapshot is
   not proof of final totals. Missing reasoning or cache counters stay unknown.
2. If no substantive output was forwarded, the launch default is a customer
   waiver: release the reservation, record `policy_waiver`, and retain the
   attempt's observed or unknown provider expense. Headers, heartbeats, and
   response-start events do not count as substantive output. No visible output
   does not prove non-execution or zero input/reasoning cost; acceptance alone
   is not a billable-usage counter.
3. If substantive output was forwarded without usable terminal usage, apply
   only the route's published stream-estimation policy. Use the input estimate
   and supported visible output deltas, excluding framing, repeated snapshots,
   and metadata. Record `settled_by_stream_estimate`, estimator revision, and
   the uncertainty described under
   [estimation risk](#estimation-risk-and-provider-expense). The customer charge
   is capped by the reservation. Unsupported or unbounded components are
   waived by the launch policy, not invented from hidden reasoning or an
   assumed cache hit.

A worker owns the deadline transition and performs it idempotently. A paid
route without a deterministic policy for all three evidence shapes is not
sellable; it may not leave wallet funds in
`held_for_reconciliation` indefinitely or treat missing usage as zero.

The reconciliation record stores `deadline_at`, the last usage observation,
substantive output counters, their durable observation watermark,
provider-acceptance evidence, and the selected closure policy. An observation
gap after a crash stays explicit; transport writes do not prove receipt by the
human client. Recovery processes overdue records without resetting deadlines.
Closing the customer's reservation never proves provider completion or makes
an uncertain attempt safe to repeat.

Late provider usage updates expense evidence without reopening the reservation.
For launch, a lower customer amount calculated under the original tariff
produces one idempotent compensating credit; a higher amount is recorded as
operator exposure, not an automatic extra debit. Differences due solely to
the published markup are not refunds or accounting errors.

### Idempotency and SDK error responses

For billable inference, clients should send an `Idempotency-Key` when they may
retry after a network failure. Scope its stored digest to the workspace,
inference key, and endpoint, and bind it to a canonical incoming-request hash.
Snapshot effective defaults on first admission; a changed server default does
not turn the same incoming request into a new operation. Authenticate before
lookup, and enforce uniqueness in PostgreSQL in the request/reservation
transaction. Concurrent duplicates must not create a second attempt or hold.

v1 does not retain generated response bodies for duplicate replay. A duplicate
generation POST returns a structured **non-2xx** error, never a successful JSON
generation placeholder, an empty body, or a successful empty SSE stream:

| Condition within the deduplication window | HTTP status | Relay code |
| --- | --- | --- |
| Same key, different request hash | 409 | `relay_idempotency_conflict` |
| Same request still running or execution uncertain | 409 | `relay_request_in_progress` |
| Same request terminal; original body is not retained | 409 | `relay_request_result_unavailable` |

For `stream=true`, decide this before sending SSE success headers. Render the
error in the ingress protocol's error envelope: OpenAI-style `error` for
Responses/Chat, the Messages error envelope for Messages, and the Gemini error
envelope for Gemini. Do not return an OpenAI error schema on every endpoint.
Expose the stable Relay code and safe metadata through that envelope's
extensions and response headers, with acceptance tests for the supported SDKs.
An illustrative Responses/Chat error is:

```json
{
  "error": {
    "type": "invalid_request_error",
    "code": "relay_request_result_unavailable",
    "message": "The original request has finished; its response body is not retained.",
    "relay": {
      "request_id": "req_example",
      "execution_state": "completed",
      "accounting_state": "settled",
      "usage_ref": "/relay/v1/requests/req_example",
      "retryable": false,
      "new_charge_created": false
    }
  }
}
```

Return `Content-Type: application/json`, `Cache-Control: no-store`,
`X-Relay-Request-ID`, `X-Relay-Error-Code`, and the SDK retry hint
`x-should-retry: false` on these HTTP conflicts. A bare 409 can trigger SDK
automatic retries; the header is a compatibility hint, not a guarantee for
every SDK/version. Test default retry settings as well as retries disabled;
unsupported clients need an explicit no-retry setting or Relay-aware handler.
If a client ignores the hint, every duplicate must still have zero execution
and accounting side effects. Do not advise retrying with a new key merely to
recover a lost answer; that would be a separately billable generation.

The status endpoint in `usage_ref` is Relay-specific, not a generation endpoint.
It requires the original key's request-read scope or an authorized account
session, returns only the caller's request, and exposes execution/accounting
states, usage completeness, settlement basis, and charge metadata. Monetary
amounts include currency and subunit precision; pending values are not zero.
It returns no prompt, answer, continuation payload, provider credential, or
another tenant's metadata. Ordinary SDKs receive a typed API error and need
client handling to inspect this endpoint; this is not transparent replay.

Deduplication records survive restarts for a published retention window that
covers execution, reconciliation, and supported client retries. After that
window, key reuse has no deduplication guarantee. Without the header, each
POST is a new logical request; Relay cannot infer duplicates from similar
prompts or assume every SDK supplies a stable key automatically.

Retries are allowed only when the existing Relay execution contract proves
that the prior attempt was not sent or rejected before execution, before output
reached the client, and within a shared request retry budget. Unknown execution
is not safe to replay, even when the caller repeats the same idempotency key.
A continuation is bound to its workspace, key scope, model, route owner, and
provider state; it cannot cross users or silently move to an incompatible
account.

Bound per-key and per-workspace request rate, concurrency, body size, token
limits, waiting queues, and stream duration. Return stable Relay error codes
with a request ID. Never let a slow or disconnected client retain an unlimited
provider lease.

## Durable continuations and restart recovery

Persist correctness-critical continuation ownership from the initial hosted
schema, not only after adding more inference nodes. A route-owner mapping
alone is not enough for bridges that need the original signed/native blocks
or tool-call state. Use two separate layers:

- **`continuation_sessions` in PostgreSQL**: tenant/key-scoped identity,
  origin logical request and physical attempt, provider/product, physical
  member, upstream model/endpoint/protocol, route revision, adapter/state
  version, credential authorization generation, parent/turn version, tool-call
  correlation, expiry, lifecycle state, and an optional encrypted payload
  reference. Use scoped opaque digests for response/history lookup; a provider
  response ID is not authentication.
- **Encrypted continuation state, only under an explicit retention policy**:
  the minimum original native blocks, signatures, reversible tool mappings,
  or replay material needed by that connector. Keep it separate from
  `logical_requests`, `physical_attempts`, usage, audit, and API response
  replay. Keys live outside PostgreSQL; enforce tenant binding, bounded bytes,
  TTL, deletion, and a versioned schema. Staff usage screens cannot read it.

Metadata persistence is required. Payload retention is not implicitly enabled:
the workspace/route must explicitly allow it, including any user content that
the required native blocks contain. When native provider-retained state is
sufficient, store only its protected reference and ownership. If content
retention is forbidden, require complete compatible client-supplied history or
reject the unsupported resume mode. Never promise restart-safe bridged replay
using metadata alone, or override a client's storage/privacy restriction.

The completed-turn checkpoint is the recovery boundary. Commit its ownership
and necessary payload before releasing the final successful response or
terminal success event. An ID emitted earlier in a native stream is only
pending until that checkpoint commits. A completed assistant turn waiting for
tool results is resumable; a truncated tool-call argument stream is not.
Checkpoint failure cannot be ignored or turned into successful reusable state.

Restore only unexpired, committed checkpoints. Revalidate workspace/key
permission, exact owner, route compatibility, authorization generation, and
state-schema support. A process restart alone does not create a new credential
generation; reauthentication or credential replacement follows the connector's
invalidation contract. Ordinary token refresh preserves ownership only when
that connector proves the same provider session remains valid. Revocation,
expired provider state, or incompatible state yields an explicit continuation
error, not silent rotation, stripped signatures, or a restarted tool loop.

Serialize advancement of one continuation with a short compare-and-swap
transaction on its turn/version and a fenced execution lease. Duplicate
submissions use request idempotency; simultaneous different next turns cannot
overwrite one checkpoint. Deliberate branches need distinct child identities.
Never hold this transaction open during provider execution. Lease expiry after
a crash means uncertain execution, not permission to resend the prior turn.

On a planned container upgrade, stop admission, drain active requests for a
bounded period, flush completed checkpoints and usage, then stop. Abrupt
restart cannot preserve an open HTTP/SSE/WebSocket connection or recover
unobserved output. In-flight attempts without a committed terminal checkpoint
remain uncertain and enter reconciliation; completed turns, including those
awaiting tool results, can continue when their provider state is still valid.

Soft affinity, disposable transport observations, and diagnostic caches can
remain process-local. They cannot replace the durable owner or financial
state. Restore applicable hard quota/retry deadlines from durable evidence;
an empty cache is not permission to reset a provider limit.

## Customer balance and provider expense

Use an append-only, double-entry journal as the monetary source of truth.
Maintain a balance projection for fast reads, but reconstruct and verify it
from journal postings. Never edit a balance as an isolated number and never
delete financial entries; corrections are compensating transactions.

Each wallet has one explicit currency. Use checked integer fixed-point amounts,
never floating-point arithmetic. Choose the launch currency, subunit precision,
rounding policy, and customer tariff before enabling paid traffic. Do not
silently mix or convert currencies.

Represent these separately:

- **Customer charge**: amount Relay bills the workspace under its published
  tariff.
- **Provider expense**: actual, estimated, or unknown cost incurred by a
  physical provider attempt.
- **Provider quota**: subscription or account allowance observed from a
  provider; it is not a customer balance.
- **Operator price/margin**: configured commercial pricing, not provider
  expense evidence.

For a request, move a bounded amount from wallet-available to wallet-reserved
before dispatch. Use a two-stage calculation:

1. **Pre-dispatch input sizing** runs before the PostgreSQL reservation
   transaction, after bridge preparation and continuation expansion. Prefer a
   supported token counter; otherwise use a versioned conservative bound for
   that route's text and media shapes. A byte heuristic is not an exact token
   count, and a universal fixed media weight is not a safe bound. Reserve for
   the most expensive applicable context band within that bound; final reported
   usage determines the actual tariff band. Reject requests whose supported
   bounds cannot fit the route limits. Do not use the advertised full context
   window as a substitute for sizing this request.
2. **Stage-two reservation** runs after the route and tariff revision are
   known. Normalize the output cap to the explicit client limit, or to
   `default_reserved_output_tokens` resolved by key, workspace, and service
   policy. The launch default is configurable (for example, 8,192 tokens);
   it applies to every paid protocol, not only Messages. An explicit larger
   output cap must be requested by the client and must fit the route's hard
   output limit and wallet reservation bound.

The server does not reserve against the model's full passport window merely
because the client omitted an output limit. The effective reservation is
derived from the input estimate, selected context band, effective output cap,
allowed request options, and the exact public tariff. Map the effective cap to
supported native upstream controls, including billable reasoning where needed.
Do not add reasoning twice when an upstream output cap/count already includes
it. Stopping visible output locally is not proof that upstream computation and
billing stopped. Cache reads/writes, media, and paid tools need their own
supported bounds. If a candidate route changes these bounds before dispatch,
revalidate and adjust the reservation atomically first. Reject paid dispatch if
no valid tariff or finite enforceable bound exists.

Snapshot the tariff version, input-sizing method and result, context band,
effective output cap, and reservation calculation inputs on the request. After
the request, settle the customer charge once from validated billable usage and
release the unused reservation. Partial execution follows the reconciliation
policy above. Missing provider usage is `unknown`, never an invented zero;
only routes with an approved billable-usage policy may serve paid traffic.
Retries do not create duplicate logical customer charges, while every attempt
that reached a provider retains its own expense and usage outcome.

### Estimation risk and provider expense

`settled_by_stream_estimate` describes a customer settlement policy, not
confirmed upstream usage or expense. Visible text cannot establish hidden
reasoning tokens, cache hits/writes, or processing that continued after a
disconnect. An uncached input estimate can overstate the customer tariff while
unobserved thinking can understate provider expense; neither direction is
established until comparable evidence arrives.

For every physical attempt retain numeric evidence and an explicit audit
classification:

| Field | Meaning |
| --- | --- |
| `usage_completeness` and component observations | Final, partial, or missing upstream usage, with observation time/watermark. Absent reasoning/cache counters remain null, not zero. |
| `customer_settlement_ref` | Link to the logical request's one settlement and its basis, tariff, cap, and closure policy; not another per-attempt customer debit. |
| `estimate_method`, `estimate_version`, estimate inputs | Input-sizing result, substantive output delta counts, and known observation gaps. No saved prompt or output text is needed for these counters. |
| `provider_expense_state` | Confirmed, estimated, or unknown, with nullable amount/currency and evidence source. Never copy the customer charge here. |
| `charge_expense_divergence_possible` | Required true when closure relies on stream estimates or incomplete reasoning/cache/delivery evidence; false must not be the default for missing evidence. |
| `divergence_reasons` | Bounded codes such as `hidden_reasoning_unobserved`, `cache_hit_unknown`, `cache_write_unknown`, `final_usage_missing`, `input_sizing_estimated`, `observation_gap`, or `customer_waiver`. |

Capture upstream counters before translating them for the client. Keep
provider-reported counters and Relay estimates separate, including through a
protocol bridge. Cumulative observations are merged once, not summed as
independent usage frames; preserve provider semantics where reasoning or cached
input is already included in a total.

The operator sees the settlement basis, uncertainty reasons, and expense
evidence together. Aggregate dashboards distinguish confirmed margin from
estimated exposure; unknown expense cannot produce a confirmed profit. The
expected difference from Relay's published markup is not itself an anomaly.
Any later numeric comparison must use matching request/attempt coverage,
currency, and the snapshotted tariff. Preserve the original estimate and risk
flag as audit evidence after reconciliation rather than rewriting history.

### Wallet transactions and contention

Use short `READ COMMITTED` transactions with guarded row updates for hosted
wallet operations. Do not require `SERIALIZABLE` for the single-wallet hot
path and do not perform a separate balance read followed by an unconditional
decrement. The wallet is a synchronous, reconstructible journal projection.
A reservation's wallet update has this shape:

```sql
UPDATE wallets
SET available = available - $1,
    reserved = reserved + $1,
    version = version + 1
WHERE workspace_id = $2
  AND wallet_id = $3
  AND currency = $4
  AND $1 > 0
  AND available >= $1
RETURNING wallet_id, available, reserved, version;
```

Amounts use the checked integer subunits of the wallet currency. Wallet and
workspace IDs come from authenticated server state. This update is only one
part of a transaction, never a standalone money operation:

1. Claim a unique accounting operation ID and the scoped logical request
   idempotency identity. A committed duplicate returns its recorded result,
   without repeating any wallet mutation.
2. Apply the guarded wallet update. No returned row is a rejection, not a
   successful reservation; roll back the operation claim and all financial
   writes. Database failures are not reported as insufficient balance.
3. Insert the reservation, balanced `journal_postings`, and request/pricing
   snapshot in that same transaction. Commit all of them before dispatch; a
   failed journal or reservation write rolls back the wallet update too.

Settlement and release use a conditional transition of that exact reservation
from `held` or `held_for_reconciliation` to one terminal state. The transition,
unique closure operation, wallet projection, and postings commit together.
For a held amount `H` and authorized charge `C`, require `0 <= C <= H`, subtract
`H` from reserved, and return `H - C` to available. Release is the same closure
with `C = 0`. A losing concurrent closer rolls back all its writes and reads
the committed outcome; it must not decrement the aggregate reserved balance
again. A validated zero-price route may record a zero hold, but missing pricing
must never be converted into one.

Conditional updates still acquire row locks and wait behind a concurrent
writer. `READ COMMITTED` rechecks the balance predicate after that writer
commits; it does not remove contention. Use a uniform lock order for wallet,
reservation, and journal operations, bounded lock/statement timeouts, and
bounded whole-transaction retries for deadlocks or transient failures. Never
retry only the decrement. If the commit acknowledgement is lost, look up the
same operation ID before retrying; do not resend inference.

Other balance writers (credits, refunds, adjustments) use the same accounting
owner and transaction rules. Additional key/workspace spend counters must be
updated atomically with reservation in a consistent lock order. This design
does not make arbitrary multi-row decisions safe under `READ COMMITTED`.
Measure wallet lock wait, transaction latency, database pool wait, and
reconciliation lag with 8-16 concurrent requests in one workspace as well as
many workspaces. No wallet lock or database transaction spans provider I/O,
streaming, or a callback to an external payment service.

Payment flow is:

```text
payment intent -> provider checkout -> signed callback verification
               -> idempotent provider event -> journal credit -> receipt
```

Use a unique provider event ID and idempotency key. A callback cannot choose
its credited workspace or amount; those come from the server-side payment
intent and verified provider response. Duplicate, delayed, invalid, refunded,
and out-of-order callbacks have explicit transitions. Admin credits, debits,
refunds, and corrections require an actor, reason, reference, and immutable
audit event. Payment adapters can be selected independently from the ledger
contract.

## Usage and operator views

Store one logical request row with `workspace_id`, authenticated `user_id`
when the credential identifies one, `key_id`, public model, timestamps, final
state, customer charge, usage counters, and safe diagnostics. Store physical
attempts separately with attempt number, pool/member reference, upstream model,
protocol/adapter, configuration revision, dispatch outcome, latency, provider
usage, provider expense state, retry reason, and the estimation-risk fields
defined above. Do not sum a linked logical customer settlement once per
physical attempt.

Attribute identity from the authenticated session or the user to whom an
individual inference key is assigned, never from request JSON or a model name.
Issue separate keys per user when operator reporting must identify the actual
person. A shared key can identify the key and workspace, but cannot prove which
human used it; label that activity accordingly instead of attributing it to the
key creator. Persist no prompt or response body by default. Secrets,
authorization headers, cookies, raw provider errors, and provider response
bodies do not belong in logs, audit records, metrics labels, or support exports.

Users see only their workspace's request history, balance journal projection,
keys, model access, and safe errors. Operators can filter activity by user and
inspect route/attempt metadata, balance operations, and status, but raw prompts
and provider secrets remain unavailable by default. Sensitive administrative
reads and all state-changing actions are audited. Retention and account
deletion must preserve legally required financial records while removing or
anonymizing eligible personal data.

Keep operational logs, immutable audit events, request usage, provider expense,
and financial journal as distinct data sets with independent access and
retention policy. Metrics use bounded labels; never label by key, user, prompt,
or arbitrary model input.

## Local and hosted pool transfer

The local pool is an authoring and migration source, not an automatic
synchronization authority. Settings presets, account transfer, hosted grants,
and user billing data remain distinct records.

Use a revisioned import operation:

1. The operator connects to the hosted service and selects the destination
   workspace and local pool.
2. The desktop creates a manifest of portable settings, physical connections,
   route declarations, model rules, and credential references. It does not
   include user balances, usage history, API keys, sessions, or conversations.
3. The server stages the manifest and returns a preview: added/changed items,
   duplicates, ownership, models without viable routes, context/features,
   expected grants, and credentials requiring reauthentication.
4. The transfer default depends on the credential kind:
   - static API sources use `copy`, allowing bounded parallel verification;
   - OAuth connectors such as `codex` and `excel_bps` use
     `stage_and_drain`. Before the server receives the secret, the desktop
     atomically marks the local account `draining` or `disabled`, so two
     refresh owners cannot race the same provider session.
   A move remains explicit and keeps the source recoverable until hosted
   verification completes.
5. Transfer secrets only after explicit confirmation, over HTTPS, directly
   into the server vault. Never write them into a portable preset, ordinary
   log, or support bundle. Credentials that cannot safely transfer require a
   fresh sign-in on the destination.
6. Validate the full target configuration, authorize ownership, apply one
   revision, build the runtime, and perform a bounded probe without paid
   generation unless explicitly requested.
7. Report the resulting revision and per-item outcome. On failure, roll back
   the hosted revision. For `stage_and_drain`, restore the local OAuth account
   to `active` automatically if the hosted revision was not applied; for a
   successful apply, keep the local account drained until the operator
   explicitly completes the move or restores it.
8. Mark a move complete only after the operator verifies the hosted pool;
   then remove the source copy through an explicit confirmation. A failed
   transfer must not delete the local credential or leave it permanently
   disabled.

The reverse flow follows the same preview, permission, secret, revision, and
recovery rules. A workspace can export only connections it owns or is allowed
to transfer. Never transfer opaque provider conversation state or silently
overwrite a newer local or hosted revision.

## Data and deployment

PostgreSQL owns durable hosted state. Use append-only migrations and explicit
transaction boundaries. Expected top-level records include:

```text
users, external_identities, sessions
workspaces, memberships, pools, pool_members, sources, credential_refs
model_grants, route_revisions, api_keys
wallets, journal_transactions, journal_postings, reservations
payment_intents, payment_events
logical_requests, physical_attempts, audit_events, import_operations
continuation_sessions, continuation_payload_refs
```

This is a domain inventory, not a required one-table-per-name schema. Enforce
unique idempotency keys, tenant-scoped foreign keys, valid state transitions,
nonnegative available balances, and append-only financial/audit records in
database constraints or narrowly owned transaction code. The first hosted
schema includes scoped deduplication keys, one terminal closure per reservation,
continuation origin links to logical requests/physical attempts, and
continuation version/expiry indexes. Payload references are optional and obey
the encrypted-state policy. Tenant-facing continuation references must not
make shared provider inventory or another workspace's state enumerable.

Choose the first deployment region as the intersection of the initial paying
users' latency needs, each provider product's documented regional availability,
data-residency obligations, payment support, and an operator's ability to
support that jurisdiction. Do not select a region to bypass a provider's
availability or account restrictions. If no single region satisfies the
initial provider set, narrow the launch set or design an explicitly reviewed
provider-specific egress path; do not pretend one region supports every source.
Start with one API deployment region and add regions only when user demand,
provider rules, and measured latency justify the added state and recovery
complexity.

The first hosted deployment should have one versioned API image, bounded
workers, PostgreSQL, a persistent encrypted credential store, and HTTPS
ingress. Docker Compose is the canonical installation; an install script may
prepare and run that same deployment. Do not expose PostgreSQL to the public
internet. Keep database, vault, and payment credentials out of images and
compose files committed to source.

Backups must include a consistent database snapshot and the matching encrypted
vault material. Test restore into an isolated environment and validate keys,
credential references, balances, usage links, and a synthetic request before
accepting restored traffic. Include authorized continuation payloads and their
key material in the recovery plan without extending TTLs or resurrecting
revoked keys and expired provider state. Reconcile restored in-flight financial
operations before enabling paid dispatch. Health endpoints expose readiness,
not secrets.

Add caches or a distributed queue only when measured load requires them.
PostgreSQL remains the authority; ephemeral caches never decide identity,
authorization, wallet balance, model grants, or settlement. Multi-instance
inference later requires shared admission limits, physical-member leases,
cross-node enforcement of the existing durable continuation ownership, and
idempotent usage delivery.

## Capacity and server sizing

Do not use registered-user count as an inference capacity promise. Track these
quantities separately:

- **Registered users**: identities and workspaces stored in PostgreSQL. A
  single node can hold many thousands of mostly idle users; this number is
  primarily a product, database, and support concern.
- **Active users**: users making requests during a time window. This drives
  authentication, model listing, usage reads, and admission traffic.
- **Concurrent generations**: requests that have reserved a physical member or
  are streaming from a provider. This is the critical capacity number.
- **Request shape**: input tokens, output cap, long-context mode, tools,
  images, stream duration, and whether the request needs a durable
  continuation. A long-context stream can consume the same server slot for
  much longer than a short completion.

The effective generation limit is the minimum of the independent ceilings:

```text
C_effective = min(
  C_server,
  C_database_and_workers,
  sum(C_eligible_member),
  C_provider_route_quota,
  C_workspace,
  C_key
)
```

`C_eligible_member` is counted per physical provider member, not per protocol
adapter or public model alias. If two credentials belong to the same upstream
principal and share a provider quota, aggregate them under one quota fence
instead of counting them as independent capacity. Provider limits can also be
measured in requests per minute, tokens per minute, daily quota, spend, or
concurrent sessions; the scheduler must enforce all applicable dimensions.

For a first single-node paid deployment, use admission limits below as
starting policy, not as a service-level guarantee:

| Deployment profile | Typical node shape | Initial generation cap | Policy |
| --- | --- | ---: | --- |
| Budget launch, all-in | 4 vCPU, 8-16 GB RAM, local PostgreSQL, bounded workers | 20-30 | Prefer short context; queue briefly, then return a stable overload error. |
| Compute-focused launch | 8 vCPU, 16 GB RAM, local PostgreSQL, separate backup target | 40-60 | Increase only after load tests and provider-quota observation. |
| Long-context-heavy pool | Any of the above | 10-25 | Use a separate context band and stricter per-workspace limits. |

These caps assume an async Rust service, bounded request bodies, streaming
backpressure, no prompt/response archiving, and a small number of management
workers. They are deliberately below a theoretical CPU maximum so that
PostgreSQL commits, settlement, refresh jobs, TLS, and operator actions keep
latency predictable. They must be replaced by measured limits from a
synthetic load test using the real request mix.

At launch, enforce a small hierarchy such as:

- 2-4 concurrent generations per inference key;
- 8-16 per workspace;
- a global server cap from the table above;
- a separate bounded waiting queue per priority class;
- immediate rejection with a request ID and retry hint when the queue or
  reservation budget is exhausted.

Provider accounts normally become the first bottleneck. For example, ten
eligible physical members with an observed limit of four concurrent requests
each provide at most forty provider slots before token, spend, or daily
quotas reduce the usable number. The server must not promise forty slots if
only two members are healthy, if the same upstream quota is shared, or if the
requested model/context is supported by fewer members.

Scale decisions should use measured signals: active streams, queue age,
provider `429` rate, p95 admission latency, database pool wait time, CPU,
memory, network, settlement lag, and vault/refresh failures. First increase
the single node or add pool members when the provider is the bottleneck.
Introduce a second inference node only after shared PostgreSQL admission,
member leases, continuation ownership, idempotent usage delivery, and
restore/recovery tests exist. A `$100` single-node installation is therefore
appropriate for an initial controlled launch, not for an HA or
unbounded-public-traffic promise.

## Delivery and acceptance

The dependency order is owned by
[ROADMAP.md](ROADMAP.md#delivery-stages); the workstreams below define the
acceptance scope, not a separate implementation schedule.

1. **Contract and threat model**: decide workspace ownership, shared versus
   private provider capacity, sign-in/linking methods, currency, pricing,
   partial-failure charging, data retention, provider commercial-use permission,
   and launch regions. Record each decision before schema or payment
   integration.
2. **Hosted foundation**: PostgreSQL migrations, identity linking, workspaces,
   membership checks, device-paired desktop management, audit, and vault
   references, including scoped request idempotency and continuation ownership
   in the initial schema. No public paid generation yet.
3. **Catalog and pool control**: management CRUD, configuration revisions,
   credential lifecycle, grants, route health, and per-workspace model
   projection. Verify private inventory isolation.
4. **Accounting**: immutable ledger, reservations, settlement, adjustments,
   idempotent payment events, bounded reconciliation, and explicit estimation
   risk. Prove `READ COMMITTED` guarded updates, atomic postings, concurrency,
   and recovery before enabling charges.
5. **Inference**: user-scoped keys, native-surface model listing, request
   admission, safe route handoff into `relay-core`, provider-native streaming,
   usage attribution, SDK-compatible duplicate errors, and committed
   continuation recovery. Allow qualified ingress bridges without substituting
   upstream vendors or models. Enable only routes with approved commercial
   use, supported pricing, and billable usage policy.
6. **Entry channels and import**: website and existing Telegram flows use the
   same identity/account APIs; add local-to-hosted and hosted-to-local
   revisioned transfer.
7. **Release operations**: Docker installation, TLS, monitoring, rate limits,
   backup/restore, migrations, incident recovery, and real-client/provider
   acceptance.

Release is blocked unless tests prove:

- two channels resolve to one linked user and the same workspace state;
- no user or key can enumerate, read, spend, or infer another workspace's
  private resources;
- the same physical member is counted once for quota, load, and rotation even
  when several workspaces can use it;
- concurrent reservations cannot overspend, and ledger replay matches the
  balance projection;
- competing reservations on one wallet, duplicate settlement/release jobs, a
  failed posting insert, deadlock/timeout retries, and a lost commit
  acknowledgement leave one financial effect or a full rollback; test these
  against real PostgreSQL, not only an in-memory accounting mock;
- retries, timeouts, cancellation, late usage, payment callbacks, and process
  restarts settle exactly once or remain explicitly uncertain;
- reconciliation releases holds by the configured policy after its deadline;
  hidden-reasoning and cache-hit/miss fixtures preserve missing counters and
  the divergence flag, never present a stream estimate as confirmed expense,
  and do not create a second charge from late usage;
- SDK JSON and streaming retries receive a valid non-2xx error envelope for
  completed, in-progress, and conflicting duplicates; test supported
  OpenAI/Anthropic SDK versions with default retries and with retry hints
  ignored, proving one logical request, reservation, and generation;
- after graceful and abrupt restarts, an unexpired completed-turn checkpoint
  resumes on its exact owner, including a turn awaiting tool results; missing
  payload, expired state, changed credentials, revoked keys, incompatible
  versions, and cross-tenant references fail without replay or substitution;
- a crash before checkpoint commit cannot turn a pending response/tool call
  into a reusable continuation; simultaneous next turns are fenced, and
  continuation-storage refusal does not silently enable content retention;
- model lists and context limits match exact grants and runnable routes;
- each public provider surface preserves the selected upstream provider's
  native request, response, stream, error, and usage contract; ingress bridges
  may convert client protocols, but there is no cross-provider model
  substitution or fallback;
- no shared paid route is active without documented provider permission,
  regional availability, billable usage, and bounded customer pricing;
- imported credentials, ownership, and configuration recover after partial
  failure without deleting the source;
- logs, metrics, exports, and user/operator views contain no forbidden secrets
  or raw prompt/response data; any authorized encrypted continuation content
  is separately access-controlled and expires under its explicit policy.
