# Hosted Relay roadmap

The [system lifecycle](HOSTED_RELAY_ARCHITECTURE.md#step-by-step-scenario-map)
describes what the service must do. This roadmap defines the order of
implementation and how to prove that each stage works.

**Current stage:** restricted customer qualification. The [core fixture](../../crates/relay-core/tests/hosted_execution_boundary.rs)
exercises the first H01 boundary. The [server](../../hosted/server/README.md) implements
migration, public guide and policy pages, local first-owner binding, routine passkey
sign-in, the authenticated closed overview and logout. The [bootstrap test](../../hosted/server/tests/bootstrap.rs)
can run the production web build against it. The [tariff calculator](../../hosted/server/src/accounting.rs)
previews seller rules and values complete token counts; the catalog and inline
price editor preserve its decimal precision. The private [connection vault](../../hosted/server/src/vault.rs)
supports fenced write, rotation, read and revocation for provider and proxy material.
The [payment worker](../../hosted/server/src/payments.rs) posts an already accepted,
verified invoice to a personal wallet and its journal once; the
[PostgreSQL fixture](../../hosted/server/tests/payments_postgres.rs) covers concurrent
redelivery, isolation, rollback and restart. The [customer journey](../../hosted/server/tests/customer_identity.rs)
uses a loopback OIDC issuer and the production web build for verified-subject review,
passkey-approved admission, repeated sign-in, a scoped wallet read and
[customer-owned keys](../../hosted/server/README.md#customer-api-keys), including
concurrent issuance, lost acknowledgement, revocation and suspension. The
[first funded request](../../hosted/server/tests/first_funded_request.rs) exercises a separate
[restricted text worker](../../hosted/server/README.md#first-funded-text-request):
one verified credit, one provider send, a PostgreSQL hold and debit, a separate
supplier expense pinned at dispatch, linked Usage and Balance receipts, customer
isolation, idempotency and restart recovery.
[Customer reports](../../hosted/server/README.md#customer-reports) share a saved
selection across Overview, filtered request pages and direct CSV. A posted credit
appears on refresh without changing an open report.
[Model status](HOSTED_RELAY_ARCHITECTURE.md#model-status-history) follows the
customer catalog order and reads recorded request metrics, including measured
completion latency, without provider sends. The opt-in [monitor collector](../../hosted/server/README.md#model-status)
attests complete availability slots from private Prometheus evidence for the
current publication. Loopback tests cover gaps, outage and recovery, replay and
republication. Live probe and alert deployment remain open.
[In-app service notices](../../hosted/server/README.md#in-app-service-notices)
support owner drafts, passkey-confirmed publication and private customer read
markers in the restricted installation. Public fanout and external delivery remain open.
The text worker saves validated terminal status and counts before wallet
settlement. Recovery applies that evidence at the saved tariff using its
observation time. Late final usage can post a linked credit without rewriting
the original partial charge. [PostgreSQL cases](../../hosted/server/tests/request_selection.rs)
cover blocked settlement, restart, replay, and late supplier expense in its original UTC month.
[Native text SSE](../../hosted/server/README.md#native-text-streaming) requires a
separate check per inventory binding. Loopback cases cover streaming before the
terminal, duplicate suppression, partial usage after EOF, disconnect and
recovery after a wallet lock. Edge and live provider qualification remain open.
The private vault command creates a static source's provider and selected proxy
in one commit with a replayable receipt; a separate qualifier verifies a
named model through the pinned direct or proxy route. **Connections** shows the owner
stored API sources, their proxy bindings and current model checks without probing them.
From the same page the owner can [pair a Desktop](../../hosted/server/README.md#pair-a-desktop)
for read-only inventory and optional Overview and Usage access. The customer
journey exercises scoped approval and revocation against the native device client;
pairing does not copy a source.
The owner's **Pool** page admits verified inventory, saves source purchase rates
and defaults, and reviews fixed or marked-up selling rates before publishing to
the named testers. Customer Models and the key catalog read that publication.
The restricted worker defaults to shared Automatic rotation to choose the lowest
whole-request cost among ready, compatible suppliers at one published tariff.
The owner can save **Primary & reserve** and member order from Pool → Members.
That strategy ranks eligible connections in the saved order for new holds.
PostgreSQL pins the eligible inventory and rechecks the selected supplier before
dispatch. [Two counting suppliers](../../hosted/server/tests/request_selection.rs)
cover cost selection, cap and capacity races, cooldown, stale approvals and supplier fencing.
The cap race uses distinct keys and models after uncapped spend and checks the
retained supplier bound after a lost acknowledgement and restart.
Losing one credential does not remove the listing while compatible supply remains;
withdrawal and expired customer approval still deny new execution.
Identity approval alone cannot enable inference or public registration.
[Local owner recovery](../../hosted/server/README.md#recover-owner-access)
replaces lost passkeys without changing the service or customer authority.
[Owner passkey management](../../hosted/server/README.md#manage-owner-passkeys)
adds verified backups and revokes individual keys and their sessions.
[The owner action matrix](HOSTED_RELAY_ARCHITECTURE.md#owner-confirmation-by-action)
lists the actions that require single-use, session- and intent-bound passkey confirmation.
[Customer access cases](../../hosted/server/README.md#customer-access-cases)
support per-workspace suspension, reinstatement and closure. Reinstatement does
not reactivate old sessions or keys. An isolated restore can replay access
decisions from independently preserved receipts.
Account import, automated off-host recovery, public identity methods and checkout, public sale
and scheduled tariff increases, retained aggregate reports/exports and deployment
qualification remain open.
[Desktop qualification](../project/ROADMAP.md#current-confirmation-scope) continues separately.

Start with the [delivery stages](#delivery-stages) and [first engineering task](#first-engineering-task).
The [journey tests](#journey-test-gates) specify the observations required;
the [decision register](HOSTED_RELAY_ARCHITECTURE.md#design-decisions-that-must-not-be-guessed)
identifies what stays closed without evidence. Development and release commands are in
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Implementation baseline

| Starting point in this checkout | What the server must supply |
| --- | --- |
| [Core ingress](../../crates/relay-core/src/gateway.rs), [runtime](../../crates/relay-core/src/runtime.rs) and [usage callback](../../crates/relay-core/src/usage.rs) | Reuse selection, codecs and observations. The loopback router, single-key LAN wrapper and callback are not tenant admission, a durable dispatch permit or an accounting journal. |
| [Native device client](../../crates/relay-hosted-device/src/lib.rs) and [Desktop pairing](../../desktop/src-tauri/src/launcher/hosted_device.rs) | Approved devices read server inventory and can separately be granted overview and safe usage history. Static source/proxy copy uses a separate approval. Explicit pool membership changes remain open. |
| [Public config](../../hosted/server/openapi.yaml), [web entry](../../hosted/web/src/main.tsx) and [preview snapshot type](../../hosted/web/src/features/app/data.ts) | Preserve the closed production entry. Customer and owner reads and acknowledged writes need their own authority; never serialize the combined development snapshot. |

## Hosted multi-user Relay

The first public Hosted release is a complete prepaid model-token service backed
by operator-managed account/API-provider capacity. It has
owner-configured Google/Telegram/OIDC customer login, one identity/wallet across
channels, scoped keys, direct inference and local-pool control parity in the web
owner panel. Desktop is a [limited operator console](HOSTED_RELAY_ARCHITECTURE.md#desktop-hosted-scope)
with Overview, Usage, and Connections and pool, not full web-admin parity.
The Hosted panel has no local Applications section.
The [Hosted target contract](HOSTED_RELAY_ARCHITECTURE.md)
owns product decisions, launch defaults and safety gates.

Use its [recommended first-production stack](HOSTED_RELAY_ARCHITECTURE.md#recommended-first-production-stack)
and [empty-host acceptance sequence](HOSTED_RELAY_ARCHITECTURE.md#from-an-empty-host-to-paid-service)
as the baseline. Dependencies and deployment artifacts are part of the planned
implementation. Use the shared core for the new Hosted runtime.

### Delivery stages

Build in the following increments. H00–H17 are **owners and acceptance gates**,
not a queue of eighteen complete subsystems. The [dependency map](#package-dependency-map)
specifies which contracts each increment consumes. Keep each increment buildable;
complete the [design gate](HOSTED_RELAY_ARCHITECTURE.md#specification-before-implementation)
for its affected interfaces, not the DDL or UI for every future feature.

| When | Deliverable | Proceed only when |
| --- | --- | --- |
| 1. After Desktop review: prove the integration boundary (H00/H01) | Select one synthetic text route: `POST /v1/responses`, non-streaming, one model, one bounded tariff. Exercise two principals against one physical member in a core fixture; identify the last check before **each** provider send. | Denied work produces zero sends, authorized work one; caller headers cannot select the tenant, the two scopes cannot share private state or double physical capacity. If this cannot be expressed, fix the narrow core interface before building the service around it. |
| 2. Boot a closed installation (H02/H03/H05; first H13 read) | Start the independent server on an empty PostgreSQL database, migrate, provision the owner locally, serve the versioned `/api/hosted/v1/public/config` to the production web build. | A restart preserves owner and `setup`; a second setup cannot claim it. Anonymous reads contain only public fields. Anonymous admin access, funding and generation are denied. An absent or incompatible API shows unavailable, not preview data. |
| 3. Follow one funded request end to end (slices of H04–H10, H12–H14 and the owner transition below) | In a disposable installation in `qualification`, register two allowlisted synthetic customers. Fund one through a verified test-adapter event; issue a key, reserve, dispatch to a counting loopback provider, settle and show the receipt in Usage and wallet history. Use migrated PostgreSQL and real handlers. | Replay the funding event and the same idempotent request; interrupt after reserve, dispatch intent and settlement commit. Assert send count, one economic effect, no foreign read/spend, and either the same receipt or an uncertain outcome with bounded hold closure. |
| 4. Widen only the proven path (H07–H16) | Add streaming and other offered operations one at a time. Bind pool/catalog/status, notices, customer and owner actions and the limited Desktop console to live local APIs. Exercise revocation, transfer with proxy, restore and upgrade while ingress is closed to the public. | Qualify each advertised combination, including interrupted streams. Run the production web build without fixtures; Desktop can close without interrupting the server. A failed route stays unavailable without blocking unrelated qualified routes. |
| 5. Open paid service (H16/H17) | In restricted `qualification`, use separately authorized identities, an approved merchant and **one explicitly permitted** capacity class. Exercise the real funding → inference → receipt journey and isolated restore before an owner opens `live`. | Measured recovery and capacity, scoped client/provider/payment evidence and commercial decisions match this installation and revision. A failed gate keeps public admission closed; existing refunds, callbacks and settlement still run. |

**How stages 3 and 4 reach inference:** use a disposable installation with fixture
identity/payment adapters, a loopback provider and no live credentials. Its owner
performs `setup → qualification` through the revision-checked H02/H05/H15
operation, with H16 readiness for the selected test scope. Allowlist verified
issuer/subject pairs before first registration; bind their workspaces on callback.
An unlisted third subject cannot register, fund or dispatch. Use the same guards,
payment verification and ledger as the real service. No seeded wallet balance,
public test-credit endpoint, direct mode update or production demo switch.
Only the required transition/readiness slice is needed here, not all of H15/H16.
Stage 5 replaces fixture evidence with authorized real evidence.

Review commercial feasibility at H00, not for the first time at H17: identify
which exact capacity the operator may sell and which merchant can fund/refund it.
If the intended subscription class cannot be approved, keep it disabled. An
API-source-only launch is a product-scope decision, not an automatic fallback.
The isolated technical proof can proceed without either commercial decision.

Internal stages do not reduce the first-public-release scope. One qualified
account family is enough initially; additional Claude/other account connectors
are future work, not promised support inferred from an API adapter.

Excel/BPS deep acceptance and additional subscription connectors are not a
prerequisite for a Hosted release that does not offer those credential classes.
No shared paid route is inferred from a working personal account.

### First engineering task

**Question:** can a trusted host stop a generation at the last point before the
shared core sends it, without giving two customers two copies of one physical slot?
The [in-process entry](../../crates/relay-core/src/gateway/hosted.rs) and
[counting-upstream fixture](../../crates/relay-core/tests/hosted_execution_boundary.rs)
implement this first slice.

**Fixture:** two host-created workspace/key principals A and B; one shared scheduler,
one static API source and member, one slot, one model, native `POST /v1/responses`
over loopback HTTP. Accept only bounded, non-streaming text, with a fixed synthetic
tariff revision/bound supplied as an input, not calculated or charged here. No
OAuth, tools, media, continuations, alternate transport or fallback route. No
new server, database, vault, merchant, external provider or web/desktop screen.

**Where to work, in order:**

1. Exercise the Hosted boundary fixture. Use an
   independent counting upstream and a controllable in-memory host. Put barriers
   at slot acquisition, host acknowledgement and upstream receipt; do not infer
   order from sleeps, HTTP status or the core usage callback.
2. Expose a narrow in-process entry from [the core gateway](../../crates/relay-core/src/gateway.rs)
   for this operation. The host supplies the principal, logical request, approved
   member/model and bound; no HTTP header/body may supply that authority. Reuse
   request preparation, candidate leasing and dispatch; do not mount the local
   router, synthesize a LAN key or construct a runtime per customer.
3. Follow [`dispatch_request_attempt`](../../crates/relay-core/src/gateway/execution/request/dispatch.rs)
   into [`send_authorized_request_for_route` and `send_prepared_authorization`](../../crates/relay-core/src/runtime/authorization/dispatch.rs).
   The latter reaches [HTTP execution](../../crates/relay-core/src/runtime/http3/send.rs);
   some **other** credential paths can call it again after authorization repair.
   [Current `AuthorizationDispatch`](../../crates/relay-core/src/runtime/authorization.rs)
   carries local budget, lease and observation context, not a Hosted dispatch claim.
   Bind a fresh host decision to the logical request, attempt, physical member,
   authority revision and approved bound. Only an acknowledged current attempt
   can enter the send path, once. Keep the local lease/budget guards; do not
   hold their locks across the host acknowledgement or provider I/O.
   A revocation committed before the host claim denies it. An attempt claimed
   first may drain once acknowledged; the claim cannot authorize another attempt.
4. Run each case below with a fresh fixture. Count generation POSTs **at the
   provider** and record scoped decisions and maximum concurrent sends separately.
   Losing a reply about a send is not evidence that no send happened.

| Case | Arrange / act | Required observation |
| --- | --- | --- |
| Allow and isolate | A sends a valid request with a misleading caller-supplied tenant label; B then presents A's attempt as its own. | A owns the only accepted attempt and result; B gains neither. **1** POST total, none attributable to B. |
| Refuse | Deny A's final host decision; separately submit a request for an unoffered operation. | **0** POSTs in each run; no alternate member, successful result or stranded slot. |
| Compete | Hold A at the provider, then start B for the same member. Release A after observing B waiting. | While A runs: **1** POST and no second occupied slot. With both still eligible: **2** POSTs in total, peak concurrency **1**, distinct requests and results. |
| Revoke while waiting | Same barrier, but remove B's grant before A releases the member. | A finishes normally. B is denied on recheck; **1** POST total. Acquiring a slot is not authorization. |
| Reuse a prior decision | After one evidenced pre-execution refusal from the provider, exercise the attempt boundary again for the same logical request. Refuse a fresh decision; also try the consumed first decision. | **1** POST total, **0** additional. This is a boundary test, not a requirement to make a static API key retry on 401. Any later retry-capable route must also exercise its actual inner retry path. |
| Lose the host acknowledgement | Host records the attempt but withholds its reply before the send; retry the same operation lookup. | **0** POSTs. No implicit permission or second attempt; recover the existing claim. The missing reply alone cannot clear a recorded claim as not sent. |
| Lose the provider outcome | The upstream counts A's POST, then drops the response before any reliable execution evidence reaches the core. Repeat the same scoped lookup. | **1** POST total, outcome uncertain; neither lookup, lease expiry nor fallback generates another answer. |

From the repository root:

```sh
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --test hosted_execution_boundary
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --test scheduler --test accounts
```

The existing [physical-attempt](../../crates/relay-core/tests/support/physical_attempt_safety.rs),
[capacity](../../crates/relay-core/tests/support/route_capacity.rs) and
[fallback](../../crates/relay-core/tests/support/route_fallback.rs) suites protect
local behavior alongside the Hosted fixture. **Stop before widening Hosted ingress**
if a send bypasses the host, a refused attempt sends, a slot doubles, an ambiguous
send repeats, or fixing the boundary would require changing Desktop semantics.
A pass establishes only this in-process contract. Crash durability, money and
real provider permission have separate gates.

### Post-release expansion

After the first Hosted release passes H17 acceptance, the next product focus is
additional subscription authorizations and supported application integrations,
as ordered in the [Desktop delivery focus](../project/ROADMAP.md#delivery-focus).
Use the existing shared core rather than a separate execution implementation.
The [connector track](../project/ROADMAP.md#future-connectors) qualifies each
credential family's lifecycle, execution and permitted deployment scope; the
[application track](../project/ROADMAP.md#future-application-integrations)
qualifies client launch, connection and recovery behavior. Hosted admission of a
new subscription route still requires its own commercial and technical evidence.
Maintenance and qualification of already offered integrations continue before
this expansion stage; these additions do not enlarge first-release scope.

### Customer website implementation slices

H13 owns the web shell and typed transport, H14 the customer cabinet, and H17
end-to-end acceptance; use their [package definitions](#hosted-implementation-work-packages). The
[website contract](HOSTED_RELAY_ARCHITECTURE.md#customer-website) owns routes and
data meanings; [web design](HOSTED_RELAY_WEB_DESIGN.md) owns presentation.
Port the [existing site's reusable parts](HOSTED_RELAY_ARCHITECTURE.md#public-source-and-operator-installations)
into the public distribution. Ship the guide and policy pages with the default
site. At H00 decide whether a working Image Studio gates the Zenith-operated launch.
Any installation offering Studio must qualify its funded customer journey.

### Hosted implementation work packages

The [web design specification](HOSTED_RELAY_WEB_DESIGN.md) defines WS-01–WS-24,
visual/interaction baselines and the design-review sequence. H00 reviews this
presentation contract before H13/H14/H15 implement it. Complete screen descriptions
and mockups do not close runtime, accessibility, security or paid-release gates.

Keep package identifiers stable when decomposing them further.
Architecture owns target names and semantics;
this plan owns dependencies, implementation order and evidence. Do not create a
second schema/API definition in task descriptions or treat a planned path as an
existing implementation.

For each affected subtask, resolve the architecture's
[specification gate](HOSTED_RELAY_ARCHITECTURE.md#specification-before-implementation)
or identify its blocking decision. Keep contract changes in their owning sections;
routine progress and check results go in the task conversation, not new documents.

Documentation acceptance checks links, names, transition coverage, schema/API
consistency and requirement-to-test traceability. Implementation acceptance proves
the behavior with actual code and the appropriate database/client/runtime tests.
Paid acceptance additionally requires separately authorized real integrations and
recovery evidence. Internal completion of one package never narrows public scope.

#### Package dependency map

Only the files linked in the current-stage description are evidence of delivery;
other named packages/tests below are targets. The
[implementation contract map](HOSTED_RELAY_ARCHITECTURE.md#implementation-contract-map)
owns target names/types, table inventory, transaction rules, API names and open
decisions. Use `Hxx.nn` for subtasks and `HC-xx`/`HD-xx` for contract/decision
references in their acceptance descriptions. Track implementation status separately
from these identifiers.

| Package | Result | Prerequisites for the affected slice |
| --- | --- | --- |
| H00 | Reviewed design/decision coverage | Existing Hosted contract and current-code boundary |
| H01 | Verified shared-core integration/reuse map | H00 scope, trust boundary and expected results for the selected route |
| H02 | Hosted composition, configuration, schema/migration and API foundations | H00/H01 first boundary proof; HD-04/08/10 for the bootstrap schema, public config and local artifact/config contract only |
| H03 | Vault consistency, durable jobs and domain outbox foundation | H02, HD-05 |
| H04 | Tariffs, ledger, wallet and reservation primitives | H02, H03; affected HD-04 postings before each money operation |
| H05 | Installation owner, passkeys and scoped Desktop pairing | H02, H03, HD-03 |
| H06 | Customer identity, sessions, linking and recovery | H05 session/authority separation and HD-03 for the method; H04 empty-wallet primitive for first registration, not settlement or refunds |
| H07 | Inventory, pools, catalog, grants and qualified routes | H01, H03, H04, H05 |
| H08 | Top-up, verified events, refunds and payment reconciliation | H04, H06; HD-02 for a real adapter, not the test double |
| H09 | Keys, limits, request sizing and scoped idempotency admission | H04, H06, H07, HD-06 |
| H10 | Core execution, settlement, cancellation and uncertainty | H03, H09 |
| H11 | Committed continuation and restart recovery | H03, H10 |
| H12 | Usage/diagnostic/reporting/notice/export/retention projections | H03 outbox, H07 catalog; H08/H10 committed events for money/request views; H11 only for resume reporting; HD-13 only for external notices |
| H13 | Default web shell, typed API and branding | H02 contract; H05/H06 for authenticated acceptance; H07 for public model/pricing acceptance |
| H14 | Complete customer cabinet | H08, H09, H12, H13 |
| H15 | Complete web owner panel, scoped Desktop console and transfer | H05, H07, H08, H09, H12, H13; readiness interface from H02 |
| H16 | Release packaging, operations, backups and recovery drills | H02/H03 foundation; H10–H15 for full acceptance |
| H17 | Paid-product acceptance and supported deployment matrix | All first-release obligations and applicable HD decisions; verify unoffered surfaces stay closed |

Avoid circular gates: H04 supplies a wallet-initialization primitive to H06, not
an identity implementation. H07 commits publication with the H03 outbox; H12
delivers the event later. Stateless H10 requests do not depend on H11; H11 adds
checkpointed resume. H15 can implement a transition that rejects missing H16
evidence; the first successful public opening is tested at H17.

#### H00 — Specification and decision coverage

**Owns:** HC-01–HC-12. Keep contracts in architecture and presentation in web design.

- **H00.01 — Scope:** confirm journeys, exclusions and default/custom frontend scope.
- **H00.02 — Decisions:** use the [decision register](HOSTED_RELAY_ARCHITECTURE.md#design-decisions-that-must-not-be-guessed) to identify the accountable person and evidence for the affected slice. Record a missing decision as a blocker for that use, not as a global approval.
- **H00.03 — Dictionaries:** complete affected fields, APIs, states and permissions in their existing owners.
- **H00.04 — Threat/failure coverage:** map authorization, race, outage and recovery invariants to tests.
- **H00.05 — Change control:** update affected contracts, consumers and tests together; retain reuse provenance/licenses.
- **H00.06 — Scenarios:** check SC-01–SC-09 against those contracts; name blockers instead of inventing facts.
- **H00.07 — First offer:** use the [one-route boundary fixture](#first-engineering-task) in stage 1 and the [receipt fixture](#one-request-statistics-fixture) in stage 3. Mark other inference entry points closed until their admission and usage contracts are qualified; a protocol name or Desktop success is not a paid offer.

**Acceptance:** consistent links, identifiers, dependencies and expected checks.
Runtime and commercial qualification follow implementation.

#### H01 — Shared-core qualification and reuse boundary

**Owns:** HC-01/04/06, HD-12. Existing anchors are `GatewayRuntime`,
`gateway::router`, `gateway::network_router`, core adapters/usage and Desktop
vault/refresh/configuration flows. Do not rewrite the four codecs.

Existing shared account entry points now include
`providers::chatgpt::connector`, `accounts::management` and
`automations::reset_resources`. The in-memory host fixture covers reuse without
Tauri; server authorization context, durable jobs and tenant isolation still
require H01 qualification.

Quota/model monitoring, reset redemption, OAuth attempt/checkpoint lifecycle,
account import and batch removal now expose shared workflow ports. Desktop calls
these services with its native locks, credential store, transactions and events.
Hosted must implement tenant-scoped ports and prove its own durable ownership;
the Desktop extraction alone does not close H01 acceptance.

- **H01.01 — Integration:** identify core entry points, cancellation/lease/refresh ownership and restart boundaries.
- **H01.02 — Auth context:** carry server-created tenant/key/request identity, not caller-supplied attribution.
- **H01.03 — Evidence:** capture terminal, usage and attempt certainty before conversion for each offered protocol.
- **H01.04 — Retry:** verify pre-execution proof, ownership and output visibility; adapt connector failure tests.
- **H01.05 — Reuse:** make the smallest shared interface/vault change and check Desktop/server callers.
- **H01.06 — Parity:** resolve or exclude unsupported mutations, including weekly quota wake.
- **H01.07 — Rotation qualification:** implement the [shared selection and Hosted admission checks](HOSTED_RELAY_ARCHITECTURE.md#shared-rotation-and-hosted-admission). Cover supported strategies, warm/opaque ownership, fresh/missing expiry/reset evidence and mixed account/API capacity with deterministic workloads. Compare policy efficiency separately from correctness; affinity counters are not provider cache-hit measurements.
- **H01.08 — Paid handoff:** qualify the [per-generation dispatch boundary](HOSTED_RELAY_ARCHITECTURE.md#paid-ingress-and-dispatch-boundary). Start with the static one-route fixture. Before enabling any route that can generate again after a repair, retry or transport switch, exercise that actual path with a fresh decision and a denied second send.

**Acceptance:** the [first engineering task](#first-engineering-task) gates the
closed bootstrap. The rest of H01 qualifies each additional route, rotation
policy or connector before it is offered. Qualify account connectors separately from codecs.
Synthetic alternate-family fixtures must cover known quota without additional
reset support, provider-specific authorization and unknown quota telemetry.
Missing ChatGPT-specific metadata must not erase known quota, imply unlimited
capacity or invent reset support or a ChatGPT credential type. These fixtures do
not qualify that family for real sign-in or serving.

#### H02 — Hosted composition, database and contract foundations

**Owns:** HC-01/02/07/11, HD-04/08/10. Target: `hosted/server/{Cargo.toml,src/,
migrations/,openapi.yaml,tests/support/}`; existing builds remain independent.

- **H02.01 — Process:** startup validation, graceful admission stop and bounded shutdown; no automatic legacy migration.
- **H02.02 — Config:** validate typed settings, origins, secrets and finite limits before admission.
- **H02.03 — Schema:** tenant keys, operation uniqueness and append-only migrations scoped to each package.
- **H02.04 — DB:** bounded SQLx pool, short transactions, separate runtime/migrator roles and safe retries.
- **H02.05 — API:** typed routes/errors, correlation, pagination and compatibility; domain packages own endpoint schemas.
- **H02.06 — Harness:** disposable PostgreSQL and deterministic clock/provider/payment fixtures.
- **H02.07 — Service control:** persist mode/admission epoch; boot in closed `setup`, fence transitions and qualification scope.
- **H02.08 — First read:** expose versioned public configuration to the built web client. Verify migration from empty (and from the prior supported schema once one exists), server restart and an incompatible web/API pair. Unregistered inference paths have no fallback to the core router; API errors must not return the SPA shell.

**Acceptance:** `tests/{bootstrap,schema_postgres,api_contract,service_modes}.rs`
cover boot/upgrade, migration races, invalid configuration, rollback, tenant reuse,
persisted mode and stale/lost transition acknowledgements. No DB connection spans
a provider stream; H05/H15 authority and H16 readiness guard later public actions.

#### H03 — Vault generations and durable jobs

**Owns:** HC-09/11, HD-05. Target: `src/vault.rs`, `src/jobs.rs`, `src/jobs/`,
their migrations and `tests/vault_jobs.rs`.

- **H03.01 — Secret format:** qualify the existing Desktop vault; version ciphertext/manifests and restrict permissions.
- **H03.02 — Commit:** durable ciphertext → transactional reference/fence → old-generation cleanup; recover orphans.
- **H03.03 — Rotation/deletion:** fence refresh/removal and revoked restores; missing keys fail closed.
- **H03.04 — Jobs:** typed deduplicated payloads, CAS leases, bounded retries/deadlines; reconcile uncertain effects.
- **H03.05 — Operations:** expose safe queue/cleanup failures and include vault generations in backups.
- **H03.06 — Outbox:** transactional unique events, bounded fanout and checkpoints through the existing job runner; H12 owns delivery.

**Acceptance:** `tests/vault_jobs.rs` covers missing/corrupt keys/files, disk-full,
commit cut points, stale workers and restart. Replay cannot repeat a payment,
refund or generation; revoked authority cannot write.

#### H04 — Exact tariffs, journal and wallet primitives

**Owns:** HC-05/09, HD-04. Target: `src/accounting.rs`, `src/accounting/`,
accounting migrations and `tests/accounting_postgres.rs`.

- **H04.01 — Prices:** revisioned bands/components, proven zero rates, checked arithmetic and one final rounding.
- **H04.02 — Ledger:** balanced unique postings and reconstructible available/reserved projections.
- **H04.03 — Wallet creation:** atomic H06 identity/workspace/wallet initialization.
- **H04.04 — Reserve/close:** guarded holds, release, late credit and same-operation recovery; preserve tariff and sizing evidence.
- **H04.05 — Reversals:** resolve HD-04 refund/chargeback/adjustment policy, including spent funds and restricted accounts.
- **H04.06 — Contention:** define lock order across keys, top-ups, suspension, tariffs and competing closers.
- **H04.07 — Money cases:** implement [SC-03](HOSTED_RELAY_ARCHITECTURE.md#money-scenarios-and-worked-checks), including separate refund reserves.
- **H04.08 — Cache components:** qualify disjoint ordinary/read/write usage and mixed write lifetimes for each paid route; never count both aggregate writes and their TTL buckets, or substitute provider reference prices for owner tariffs.
- **H04.09 — Precision across clients:** use the same exact rate vectors in PostgreSQL settlement, API serialization and the web display/owner editor. Distinguish USD per million tokens from integer wallet microUSD; cover a positive sub-microUSD rate, confirmed zero, null and values beyond JavaScript's safe integer range.
- **H04.10 — Seller pricing:** implement the [API default, model markup and fixed selling prices](HOSTED_RELAY_ARCHITECTURE.md#api-source-rates-and-the-public-tariff). Return exact component prices for review; snapshot the published selling tariff per reservation and the actual purchase revision per attempt.

**Acceptance:** `tests/accounting_postgres.rs` proves no concurrent overdraft,
balanced/replayable postings, idempotent close/refund and commit-ack recovery.
Prove that a complete zero-exposure tariff gets one zero reservation/closure
without moving funds; a missing required rate or overflow rejects before dispatch.
Refund reserves remain unavailable to inference until a proven release commits.

Use two synthetic API sources for one offer. Purchase rates in USD per million
(uncached input, output, cache read) are A `(2, 3, 0.50)`, B `(1, 6, 0.25)`.
With only A and no default, purchase alone gives no selling price; `0%` gives
A's rates, not a free offer. At A `+25%`, B `+10%`, the proposed selling
rates are `(2.50, 6.60, 0.625)` across both complete sources. A model override
of `+50%` replaces both defaults: `(3, 9, 0.75)`. A fixed model price of
`(3, 5, 0.40)` wins over both proposals, even below B's output cost. Leaving
fixed output blank blocks publication, not just that field; an explicit zero
remains zero. Returning the model to its default restores the inherited proposal
without changing another model, context band or published tariff.

#### H05 — Owner provisioning, passkeys and Desktop authority

**Owns:** HC-03/10, HD-03/08/12. Target: `src/auth.rs`, `src/auth/`,
owner/device schema and `tests/admin_auth.rs`.

- **H05.01 — First owner:** server-local, installation-bound, single-use enrollment; atomic credential/owner assignment.
- **H05.02 — WebAuthn:** verified library-backed RP/origin, discoverable-credential and user-verification contracts.
- **H05.03 — Sessions:** protected cookies, CSRF, expiry/authorization epoch and fresh-proof action matrix.
  The [customer journey](../../hosted/server/tests/customer_identity.rs) proves
  one-use confirmation of tester admission, pause, payment-method publication
  and retirement, refund approval and decline, and exact-revision notice
  publication without a new session. It checks foreign sessions, wrong
  intents, changed targets and receipt-only replay.
  The [funded journey](../../hosted/server/tests/first_funded_request.rs) does the
  same for model admission, publication, withdrawal and purchase-limit changes,
  including a price review invalidated by changed purchase inputs. Its diagnostic
  capture case checks the exact scope, receipt-only replay and the original
  timeout. The [action matrix](HOSTED_RELAY_ARCHITECTURE.md#owner-confirmation-by-action)
  separates confirmation from draft edits and capture stop.
- **H05.04 — Recovery:** second/last-key guards and audited server-local recovery; no weaker public reset.
  The [lost-passkey journey](../../hosted/server/tests/owner_recovery.rs) verifies
  same-owner replacement, racing ceremonies, revoked web and Desktop access,
  and unchanged customer money. The [backup-passkey journey](../../hosted/server/tests/owner_passkeys.rs)
  verifies enrollment, sign-in with both keys, one-use confirmation and the
  last-key guard under concurrent revocation.
- **H05.05 — Desktop:** bounded owner-approved pairing/scopes/revocation and browser step-up for the [limited Desktop scope](HOSTED_RELAY_ARCHITECTURE.md#desktop-hosted-scope); enforce operation-level grants on the server, not full owner authority or an admin bypass.
  The customer and [native copy journeys](../../hosted/server/tests/desktop_source_copy.rs)
  verify one-use confirmation of pairing changes and copy approval, receipt-only
  retries, and a copy fenced by the reviewed device revision and grant expiry.

**Acceptance:** `tests/admin_auth.rs` rejects wrong origin/RP, replay, foreign/stale
authority, CSRF/fixation and setup races. Paired-device credentials cannot invoke
web-only customer, key, money or settings operations even through direct API calls.
Qualify real authenticators separately.

#### H06 — Customer registration, linking and recovery

**Owns:** HC-03, HD-03/08 and conditional HD-13. Target: `src/identity.rs`, `src/identity/`, customer
auth handlers and `tests/customer_identity.rs`; use H04 wallet initialization.

- **H06.01 — Integrations:** verified Google/Telegram/OIDC adapters with issuer, redirect, state/nonce/PKCE validation.
- **H06.02 — Registration:** atomically create one person/workspace/wallet per verified subject.
- **H06.03 — Linking:** prove both identities; prevent reassignment and removal of the last login method.
- **H06.04 — Sessions:** revocable cookies, CSRF and outage/recovery flows; no owner/device authority.
  The [qualification journey](../../hosted/server/tests/customer_identity.rs) exercises
  own browser-session reads and revocation, competing revocations, expiry and
  restart, including recovery when a revocation reply is lost.
- **H06.05 — Suspension/closure:** fence new access in the affected workspace, including its keys and queued dispatches. A confirmed closure retains required receipts, lets accepted obligations reconcile and reapplies revocation on restore. A support request alone is not proof of closure or refund.
  The [owner and customer journey](../../hosted/server/tests/customer_identity.rs)
  covers suspension, reinstatement, closure, stale revision and a lost response.
  [PostgreSQL cases](../../hosted/server/tests/request_selection.rs) exercise
  waiting admission and settlement, and replay a post-backup closure on an
  isolated restore.
- **H06.06 — Abuse/recipients:** bounded attempts without enumeration/lockout; enforce service mode and qualification scope.
  The [restricted sign-in profile](../../hosted/server/README.md#sign-in-limits)
  bounds starts and exchanges separately and checks a callback's scope before
  contacting the issuer. The local journey verifies throttling, one-use state,
  stale revisions and expiry during exchange. Public ingress and live issuer
  acceptance remain open; external recipients need consent, proof and revocation
  under HD-13.

**Acceptance:** `tests/customer_identity.rs` covers all adapters, callback/link races,
last-login protection, redirects/CSRF, suspension, recipient changes and scope spoofing.
No second wallet, email-based merge or customer leakage. Authorize live login separately.

#### H07 — Credential inventory, pools, catalog and route grants

**Owns:** HC-04/09, HD-01/12. Target: `src/management.rs`, `src/management/`,
`src/catalog.rs`, `src/catalog/`, capacity migrations and
`tests/{pool_catalog,provider_onboarding}.rs`.

- **H07.01 — Inventory:** scoped account/API sources, secret generations, refresh and health; saving is not pool admission.
- **H07.02 — Lifecycle:** separate add/reauthenticate/park/enable/disable/drain/trash/restore/remove; fence dispatch and ownership.
  [Static API pause, resume, park, trash, restore and removal](../../hosted/server/README.md#api-source-lifecycle)
  are implemented. The funded browser journey and `supply_confirmation_race`
  cover confirmation, lost receipts, draining, retired bindings and unchanged
  settlement. Removing drained trash revokes both live secret slots; storage
  retention and recovery qualification remain separate. The OAuth lifecycle remains open.
- **H07.03 — Parity:** reuse local validators for controls, proxies, automations and request/publication policies.
  [Pool Members](../../hosted/server/README.md#pool-members-and-activity) uses the
  existing source lifecycle and shows observed dispatches. The restricted worker
  supports Automatic and Primary & reserve with revisioned, passkey-confirmed
  member order. Counting suppliers cover the policy/hold race, restart, busy
  primary and unchanged customer tariff. Round robin, request shares and
  quota-aware modes still need Hosted qualification.
- **H07.04 — Catalog:** separate granted customer routes from bounded public descriptors/tariffs at `GET /public/models`.
  Preserve freshness/privacy; prices do not belong in `/public/config`, and aliases add no capacity.
  [Restricted model display order](../../hosted/server/README.md#model-display-order)
  is server-owned and shared by the owner editor, customer Models and Status.
  Anonymous publication and automatic reference refresh remain open.
- **H07.05 — Qualification:** require HD-01 and finite billable bounds for paid routes; reuse core discovery/validation.
  The [local HTTPS model check](../../hosted/server/README.md#check-a-model-in-the-admin-panel)
  queues a confirmed Responses check for a web-saved source. The separate
  qualifier uses its saved route; the result does not admit or publish it.
- **H07.06 — Races:** revision-check refresh/removal, activation, tariffs and draining; reads cannot activate or probe providers.
  The display-order checks cover a changed inventory, concurrent saves and unchanged
  price/enablement and key catalog. Stale writes return a conflict and the browser rereads order.
- **H07.07 — Publication:** implement [SC-06](HOSTED_RELAY_ARCHITECTURE.md#price-and-model-change-walkthrough), preserving holds and fencing failed/superseded activation.
- **H07.08 — Provider onboarding:** implement [direct server sign-in/import](HOSTED_RELAY_ARCHITECTURE.md#server-side-provider-connection-onboarding), supported-method discovery and protected attempt state. Keep provider credentials separate from customer/admin identity and pool admission; qualify cancellation, duplicate callbacks, wrong-principal reauthentication, expiry and restart.
- **H07.09 — Model controls:** combine per-member and pool-wide rules with grants and route evidence. Edit inline in Pool rules; Models reads effective publication. Apply the approved standard audience to eligible existing/new customers without manual pool membership; private grants stay separate. Effective disablement preserves historical prices, inventory and already dispatched outcomes.
  In the restricted API pool, [Members → Edit](../../hosted/server/README.md#pool-members-and-activity)
  saves per-source model flags with a passkey. The final permit rechecks the flags
  under the source lock. Already dispatched work settles, and the same offer can
  continue through another eligible source. The standard audience remains open.
- **H07.10 — Discovery to projections:** a new core model appears in owner inventory without a frontend edit. Match or withhold grants, publication and tariffs independently. Emit a revision for affected readers; a catalog refresh cannot publish a selling price, invent availability history or trigger generation.
- **H07.11 — API supply:** version each source's purchase rates and default markup separately from the public offer's selling rule. Adding, removing or repricing a supplier changes the next markup preview, never a saved or published tariff. Reject publication of a stale markup review. Source IDs, purchase rates and markups stay out of public/customer responses.
- **H07.12 — Supplier controls:** save the owner's optional monthly USD purchase limit per API connection, with revision checks. Provider credit/quota is connector evidence, not a Relay-editable balance.
- **H07.13 — Paid fallback consent:** persist the pool/offer's primary supply class and exact seller-approved backup API IDs. Default to no backups; catalog refresh, import and rotation edits cannot widen the scope. Expose this as one owner control in Pool rules, not a customer choice.

**Acceptance:** `tests/pool_catalog.rs` covers lifecycle/revision/grant isolation,
alias capacity, unknown quota and controlled-clock publication/notice races, plus company-block
and within-company ordering, disabled-position retention, discovery reconciliation and stale-order rejection.
Compare the [three model reads](HOSTED_RELAY_ARCHITECTURE.md#pools-connections-and-model-access)
for a public offer, a private grant and a narrower inference key. Revoke the key
between two reads: the second exposes no IDs. The upstream request counter stays
unchanged across all reads, including stale snapshots.
Qualify Desktop/server families together; disclose unsupported mutations.
`tests/provider_onboarding.rs` proves actor/connector/target isolation, one-use
completion and that direct Hosted onboarding works without a Desktop runtime.
Pool tests distinguish configured enablement from runtime eligibility and public
visibility; reordering/collapsing groups cannot change rotation or settlement.

#### H08 — Payments, receipts, refunds and external reconciliation

**Owns:** HC-05/09/12, HD-02/04/09. Target: `src/payments.rs`, `src/payments/`,
payment migrations, webhook routes and `tests/payments_postgres.rs`.

The [local checkout](../../hosted/server/README.md#wallet-credits) exercises
owner draft/check/publication, quote, confirmation, invoice lookup, reopening and credit
with a sandbox worker. The [voluntary refund path](../../hosted/server/README.md#voluntary-refunds)
exercises customer requests, owner review, reserves and same-operation reconciliation.
Public funding remains closed pending qualified merchant checkout and reconciliation.

- **H08.01 — Adapter:** qualify the actual merchant contract, currencies, origins, signatures and idempotency; no guessed FX/provider.
- **H08.02 — Quote and intent:** quote first without opening checkout; confirm a matching unexpired quote once. Persist the accepted quote/operation before the external call. After a lost reply, retrieve the existing intent; expiry blocks a new confirmation, not lookup of one already committed.
- **H08.03 — Event:** verify signature, merchant, environment and intent, then durably handle duplicates/reordering before acknowledgement. A matching test-mode reference must never credit a live wallet.
- **H08.04 — Credit/receipt:** one posting per economic effect, not per event ID; redirects cannot credit money.
- **H08.05 — Refund/dispute:** implement the HD-02/04/09 states, refundable remainder, compensation and audit.
- **H08.06 — Reconcile:** resolve missed webhooks/acknowledgements by operation ID without resending uncertain effects.
- **H08.07 — Lifecycle:** reserve refund funds against concurrent spend; maintenance still processes existing payments/reconciliation.
  Notices follow committed effects and cannot alter money.
- **H08.08 — Payment method builder:** implement the [adapter catalog, merchant connections and published methods](HOSTED_RELAY_ARCHITECTURE.md#payment-method-builder). Start empty. Expose the ordered customer list separately from owner drafts and credential replacement. A fork can register its own adapter without changing the wallet or the checkout UI.

**Acceptance:** `tests/payments_postgres.rs` covers signatures, amount/currency,
duplicate/reordered credit, commit loss, concurrent refunds, spent-funds reversals,
late success and delivery outage. In `hosted/web/tests/checkout-flow.mjs` and the synthetic adapter
tests, close checkout, reload history and resume twice after quote expiry but
before invoice expiry. Both clicks must reopen the original merchant invoice:
one intent, one invoice, no wallet credit from resuming. Mark it paid or expire
it between rendering the row and clicking: the stale action returns status, not
a checkout. An unsupported or uncertain invoice offers no resume action; another
workspace cannot use it or obtain its destination.

Leave a synthetic SBP invoice for a $10 wallet credit open and select card for
the same credit. Reviewing creates no second invoice. Reusing the SBP operation
ID for the card quote must conflict. Confirm it with a fresh ID, then replay:
exactly two invoices and two history rows. Pay the card
invoice: the wallet gains $10 and the SBP invoice remains payable. Pay that one
too and replay both callbacks: the wallet has gained $20 in exactly two credits.

With two open invoices and a cap of three, race two new confirmations on
separate database connections. Only one calls the merchant; the other gets
`open_invoice_limit` without consuming its quote. Lose the merchant reply and
expire the accepted quote: the unresolved intent still counts, retry returns
that intent, and another wallet's quota is unchanged. Lower the cap below the
open count: no invoice is cancelled and eligible ones still resume. At capacity
the form links to open payments. Verified closures allow a new confirmation,
not an automatic submission; duplicates cannot free another invoice's slot.

With a synthetic multi-method adapter, publish
one of two methods: a draft, failed check, test credential or unsupported channel
must not appear at live checkout. Disable the published method after creating an
intent: a stale quote cannot start a new payment, but a duplicate verified late
event credits the original wallet once and its refund still resolves.

Publish two methods on the same test connection with a fixed $0.50 fee. For a
$10 wallet credit, checkout must charge $10.00 when the merchant pays and show
$10.00 + $0.50 = $10.50 when the customer pays. Both successful payments must
credit exactly $10. Change the second method to merchant-paid: a new quote shows
$10.00, but the earlier confirmed payment and its receipt still show $10.50.
Reject a fee choice the method or channel does not support.

In `billing.spec.ts` and `payments_postgres.rs`, set a $5 minimum and $5/$10
quick amounts. With custom entry on, $7.50 can be quoted but $4.99 cannot.
Turn it off: Balance shows only the quick buttons, and a direct $7.50 quote or
first confirmation of its old quote is rejected without opening checkout.
A method capped at $8 then offers only $5; a method capped below $5 is absent.
Reject publication of an empty fixed list. Live merchant/money acceptance remains separate.

#### H09 — Keys, limits, sizing and admission

**Owns:** HC-03/05/06/07, HD-06/07/08. Target: `src/access.rs`, `src/access/`,
admission parts of `src/inference/` and `tests/admission_postgres.rs`.

- **H09.01 — Keys:** scoped one-time issuance, verifier storage, expiry/rotation/revocation; no secret replay. Reject issuance/replacement for a customer's wallet under owner or paired-device authority, including direct API calls.
- **H09.02 — Policies:** enforce workspace/key precedence and units without multiplying budgets; recheck before dispatch.
- **H09.03 — Sizing:** snapshot finite post-expansion exposure and eligible tariff bands; reject unknown/unbounded requests.
  H11 must qualify persisted continuation expansion before resume is offered.
- **H09.04 — Idempotency:** authenticated scoped digests/default snapshots; conflict on changed input. A matching duplicate recovers the original request ID even if no first headers arrived; no redispatch or cross-scope lookup.
- **H09.05 — Transaction:** atomically claim/check/reserve/persist; recover lost acknowledgement through the same operation.
- **H09.06 — Queue:** bound/cancel/expire work and atomically revalidate holds; no silent tariff increase beyond approved spend.
- **H09.07 — Fencing/fairness:** service mode, authorization epoch, due policy and budgets apply at reserve and dispatch through the shared scheduler. Qualify bounded waiting and starvation resistance across workspaces separately from connection selection; extra keys cannot multiply a workspace's share or limits.
- **H09.08 — Multiplexed ingress:** a connection is not a prepaid generation. Before offering client WebSocket, test independent turns, revocation/tariff change between turns and disconnect/reconnect. One accepted turn gets one request/hold; a denied turn gets neither an upstream send nor another turn's result.
- **H09.09 — API cost selection:** pass H07's approved rates and comparable request sizing to the shared Automatic selector. Use fresh health before estimated cost; keep ownership, core wait/retry fences and explicit pool strategies intact.
- **H09.10 — Purchase limit:** enforce the [per-connection allowance](HOSTED_RELAY_ARCHITECTURE.md#api-purchase-limits) in the physical attempt permit transaction, across all strategies. Reconstruct period totals from attempts; H10 reconciles each bound independently of the customer charge.
- **H09.11 — Subscription to paid API:** enforce the [fallback decision](HOSTED_RELAY_ARCHITECTURE.md#paid-api-fallback-for-subscription-offers) before every paid backup permit. Compare admitted and current consent, require fresh exhaustion evidence, and reuse H09.10 and the core's ownership/retry checks.

**Acceptance:** `tests/admission_postgres.rs` covers concurrent keys, foreign/revoked
IDs, exact balance, unknown/zero price, overflow, deadlines, digest conflicts and
lost acknowledgements. HTTP/SSE/SDK duplicates create no extra hold or generation.

Publish H04.10's fixed `(3, 5, 0.40)` tariff and use its sources A and B.
For 1,000 uncached input tokens and a 100-token output budget, Automatic picks
healthy B (purchase estimate 1,600 microUSD vs A's 2,300). Put B in cooldown:
A serves instead. With 100 actual output tokens the customer pays **3,500
microUSD once** either way. For 100 input / 1,000 output, A costs 3,200 vs B's
6,100; the customer pays **5,300** at that usage even if only B is available.
The seller absorbs the difference. Exercise an allowed pre-execution fallback
and a tariff increase between admission and settlement: one hold, one charge at
the original price, separate attempt costs. With B unavailable and A's required
purchase rate missing, assert zero paid sends; an uncertain send is not retried.

For H09.10, record $55 of source A purchases with **No Relay limit**, then set a
$60 limit. Race two $4 purchase bounds through different keys/models: at most
one gets an A permit. Lose the acknowledgement and restart; $4 is counted once,
not freed for a second send. Reconcile supported expense of $3: $58 accounted,
$0 outstanding, $2 available. Lower or toggle the cap; neither spent nor unresolved
amounts reset. Cross UTC month-end with an open attempt; a late result updates
only its original month. A provider top-up cannot open a capped source; raising
or disabling the cap cannot override confirmed upstream exhaustion. Exercise a
qualified alternate at the same customer price and, with none, zero sends and
release of an unspent customer hold. No private budget reason reaches the customer.

For H09.11 use two subscription fixtures and a counting metered API. Assert
actual sends, the owner attempt record and the customer journal:

| Case | Required result |
| --- | --- |
| Default off; then on with only one subscription exhausted | Zero API sends in both cases. When the second subscription is ready, it serves. A missing quota reading, an unclassified `429` or a busy slot is not exhaustion. |
| Both confirmed exhausted; consent for this offer and API, with purchase headroom | One API send and one logical charge at the saved selling tariff. A second model/tier or newly discovered API inherits no consent. An approved API-only offer needs no subscription evidence. |
| Last subscription removed; cap exhausted; or consent revoked while queued | Zero new paid backup sends. Enabling consent after admission cannot widen the queued request. A permit already committed before revocation settles once, even after restart. |
| Subscription rejects before execution vs drops the connection after a possible send | Only proven quota rejection with repeatable input, valid ownership and no client output permits a paid attempt. Uncertain sends, emitted output and an opaque subscription-bound continuation yield zero API sends. |
| Subscription quota recovers | The next independent request uses subscription capacity. Do not move an active API stream; a later pinned turn cannot bypass current consent. |

#### H10 — Execution, settlement and uncertainty

**Owns:** HC-05/06/09, HD-06/07. Target: `src/inference.rs`, `src/inference/`,
accounting settlement/reconciliation workers and `tests/execution_recovery.rs`.

- **H10.01 — Handoff:** persist fenced dispatch through H01 with authoritative identity; no SQL locks across provider I/O.
- **H10.02 — Observation:** retain pre-conversion terminal/usage evidence and safe protocol errors; stateless execution must work before H11 adds resumable success.
- **H10.03 — Streaming:** bound backpressure/duration and handle disconnect/cancel; HTTP success is not completion or retry proof.
- **H10.04 — Close:** settle once at the saved tariff. At the original deadline, test [partial usage, applicable estimate and waiver](HOSTED_RELAY_ARCHITECTURE.md#reconciliation-and-estimated-settlement) as distinct cases. Partial usage outranks an estimate; with no usable usage or applicable estimator, release even if output was observed.
- **H10.05 — Late evidence:** uniquely credit lower customer amounts; higher expense stays with the operator, never reopen/resend.
- **H10.06 — Restart:** recover the same request/fence/deadline across send, stream, terminal, commit and acknowledgement cut points.
- **H10.07 — Bound breach:** inject authoritative usage priced above the saved hold. Cap the customer debit, retain the discrepancy, block new paid admissions for the affected route/variant pending requalification and reconcile once. Other routes and existing obligations remain independent.
- **H10.08 — Purchase reconciliation:** replace the attempt's outstanding purchase bound with evidenced expense exactly once, in the original UTC month. Keep unresolved exposure counted after customer settlement. Inject an above-bound supplier cost: record it in full, fence that variant and leave the customer debit unchanged.

**Acceptance:** [request selection and recovery](../../hosted/server/tests/request_selection.rs)
uses PostgreSQL and controlled upstreams for cancellation, duplicate terminals,
bounded hold closure and supplier exposure. The restricted native text slice
uses provider partials or a waiver; the separately approved estimator remains planned.
`hosted/server/tests/first_funded_request.rs` owns the cross-package step 3 proof:
verified funding, two tenants, one physical send, exact posted charge, matching
usage and recovery. Assert durable rows and the upstream counter, not only HTTP status.

#### H11 — Continuations and durable completed turns

**Owns:** HC-06/09, HD-05/06. Target: `src/continuations.rs`,
`src/continuations/`, checkpoint schema and `tests/continuation_recovery.rs`.

- **H11.01 — Eligibility:** qualify native reference, full history or permitted encrypted payload per route; reject unsupported resume.
- **H11.02 — Scope:** bind lookup to tenant/key/provider/member/model/protocol/adapter/authority, expiry and turn version.
- **H11.03 — Checkpoint:** commit complete turns/tools before resumable success; use CAS/fenced advancement and branch ownership.
- **H11.04 — Payload:** opt-in encrypted/versioned storage with scoped size/TTL limits, not usage-screen or replay access.
- **H11.05 — Recovery:** restore only committed compatible authorized state; apply revocation, expiry and cleanup.
- **H11.06 — Provider updates:** qualify the shared [OpenAI migration contract](../../.agents/skills/relay-protocol-adapters/references/contracts.md#openai-model-migrations), including ordered configuration/tool updates, preserved reasoning identity and native versus summary compaction. Cache evidence alone cannot establish valid continuation.

**Acceptance:** `tests/continuation_recovery.rs` covers turn races, missing/incomplete
payloads, foreign/expired/revoked state and restart/version mismatch. Disabled means
no retention; limit failures cannot silently add storage or change routes.

#### H12 — Usage, reports, export and retention

**Owns:** HC-07/08, HD-07/09 and conditional HD-13. Target: `src/usage.rs`, `src/usage/`,
`src/usage/diagnostics.rs`, reporting/retention jobs, schema,
`tests/usage_reports.rs` and `tests/diagnostics.rs`.
Customer notices additionally target `src/notifications.rs`, `src/notifications/`,
their schema/routes and `tests/notifications.rs`, reusing H03 outbox/jobs.

- **H12.01 — Evidence:** separate retained financial anchors from expiring request/attempt detail, expense and audit.
- **H12.02 — Metrics:** implement canonical UTC populations, coverage and unknown/stale states; count logical requests once.
- **H12.03 — Projections:** version/rebuild aggregates and late credits idempotently. Bind report pages, charts and export to the same committed projection watermark; a correction between pages appears on refresh, not in half of one report.
- **H12.04 — API/export:** bounded scoped cursors and formula-safe CSV; separate current wallet/activity from historical filters.
- **H12.05 — Cleanup:** reviewed retention, reconciliation exclusions, tombstones and backup expiry before production purge.
- **H12.06 — Support:** bounded sanitized labels/correlation and audited lookups, not raw IP/body/header archives.
- **H12.07 — Diagnostics:** implement the [canonical DTO/projections](HOSTED_RELAY_ARCHITECTURE.md#errors-and-production-diagnostics), deduplication, caps and sink-failure handling.
- **H12.08 — Notices:** implement [SC-05](HOSTED_RELAY_ARCHITECTURE.md#customer-notifications-and-delivery) with consent/recipient rechecks and qualified HD-13 external delivery.
- **H12.09 — Model status:** project [model monitoring history](HOSTED_RELAY_ARCHITECTURE.md#model-status-history) for currently authorized models on the server; cover a new model mid-window, a telemetry gap, a revoked grant and a status refresh with zero provider sends.
  Restricted installation tests cover JSON/SSE completion timings, customer
  isolation, collector coverage, immutable retry and slot expiry. Qualify the
  live probe feed and ongoing operation under H16.03.

**Acceptance:** `tests/{usage_reports,diagnostics,notifications}.rs` proves snapshot
reconciliation, late credits, retention, authorization and event/recipient races.
Distinguish cancellation, uncertainty, recovered attempts and unsettled completion.
Test customer DTOs for secret leakage, not just hidden UI fields. Reads and notice
delivery failures cannot trigger inference or change money/access.

#### H13 — Default web application and typed transport

**Owns:** HC-01/07/10, HD-08/10. Target: `hosted/web/{package.json,src/,tests/,README.md}`
and the Hosted schema/client compatibility checks. No extraction to another repo.

- **H13.01 — Build:** retain the independent React/TypeScript/Vite artifact; keep test adapters, fixtures and simulated sessions out of its production data path.
- **H13.02 — Transport:** typed same-origin API, sessions/CSRF, cancellation, errors and compatibility checks; no fallback origin.
- **H13.03 — Routes:** implement canonical [customer](HOSTED_RELAY_ARCHITECTURE.md#navigation-and-screen-responsibilities)/[owner](HOSTED_RELAY_ARCHITECTURE.md#owner-browser-navigation) routes, deep links and callbacks with separate authority.
- **H13.04 — Configuration:** neutral branding, existing locale registry (English default, Russian, Simplified Chinese) and themes through [Relay design](HOSTED_RELAY_WEB_DESIGN.md#visual-system).
- **H13.05 — Browser safety:** scoped memory caches and late-response guards; persist only non-sensitive preferences.
- **H13.06 — Accessibility:** keyboard/focus, mobile, reduced motion and chart tables across locales/themes.
- **H13.07 — Replace preview authority:** use separate public/customer/owner response types and paginated reads. Decode exact decimal strings at the transport boundary. Replace local top-up/key/catalog writes with operation results and scoped refetch; no optimistic balance credit, local issuance, cross-tab price publication or fallback to the combined preview snapshot.
- **H13.08 — Site consolidation:** adapt the reusable API guide, legal-page layout and image workflow. Replace production URLs, integrations and brand assets with Hosted contracts and installation settings. Verify the supported examples and the old-to-new link map before any change to the existing site.

**Acceptance:** planned `hosted/web/tests/unit/api-contract.test.ts` and `hosted/web/tests/e2e/shell.spec.ts`
cover incompatibility, missing config, deep links, session/logout races and origin
isolation. Run the production artifact against the disposable server as two customers
and an owner. Inspect response bodies, not hidden controls: customer pages never
receive owner inventory or another wallet. Unplug the API and verify no synthetic
replacement or repeat mutation. Keep existing preview tests as presentation checks.

#### H14 — Complete customer cabinet

**Owns:** HC-07/08/10. Integrate the existing `overview/`, `usage/`, `models/`,
`status/`, `keys/`, `billing/` and `notifications/` features under `hosted/web/src/features/`.
Add authenticated account/security actions in account context, not another primary
sidebar item. Use [customer screen responsibilities](HOSTED_RELAY_ARCHITECTURE.md#navigation-and-screen-responsibilities).

- **H14.01 — Overview:** rolling 24h/7d/30d to server `as_of`; keep current wallet/activity separate. A chart drill-down preserves exact UTC bounds rather than silently applying Usage's calendar-day preset.
- **H14.02 — Usage:** scoped filters, pagination, safe detail/export and H12 explanations; preserve snapshot/filter context.
- **H14.03 — Models/keys:** granted catalog/bands, one-time issuance, limits, revoke/replace and lost-response recovery.
- **H14.04 — Billing:** quote, status, receipt/refund and distinct held/settled/estimated values; redirects are not credit.
- **H14.05 — Account and setup:** verified login/session recovery, last-method guard and contextual connection instructions with placeholder secrets; no standalone Help or forced welcome dialog.
- **H14.06 — Failure states:** implement the [browser failure contract](HOSTED_RELAY_ARCHITECTURE.md#browser-failures-and-unavailable-services); safe reads cannot repeat mutations.
- **H14.07 — Journey/notices:** implement [SC-02](HOSTED_RELAY_ARCHITECTURE.md#first-customer-request-walkthrough), own notices/preferences and service conditions within existing routes.
- **H14.08 — Status:** bind `/app/status` to H07/H12's authorized rows and observation windows. New catalog identity alone produces no green intervals, latency or request counts.
- **H14.09 — Studio (when offered):** implement [protected entry and key selection](HOSTED_RELAY_ARCHITECTURE.md#entering-studio-with-a-relay-key). Exercise a signed-out deep link through login back to Studio and a signed-in visit with no extra login. An empty list links to `/app/keys` without issuing a key; a failed lookup does not show the empty state.
  Confirm issuance, dismiss the one-time result and assert the route is still `/app/keys`. Navigate to Studio manually and select the new masked key.

**Acceptance:** `hosted/web/tests/e2e/customer-{journey,failures}.spec.ts` covers funded
journeys, duplicate/lost responses, revoked authority, render/network failures,
SC-03 reconciliation and lifecycle/notice states across mobile, locales and themes.
No repeated payment/key/generation or secret-bearing support copy. For an offered
Studio, select one of A's two keys: no hold or provider send until explicit submit.
Submit a synthetic image: observe one execution, one Usage row for the selected
key and one charge to A's wallet. Reloading does not repeat it. B cannot read
A's request or submit with A's key ID. Revocation between selection and submit
produces no hold or send. With image support disabled, a direct Studio
visit cannot dispatch or charge. General chat, teams and saved conversations remain out of scope.

#### H15 — Owner web panel, scoped Desktop and explicit transfer

**Owns:** HC-03/04/07/08/09/10, HD-12. Target: existing `hosted/web/src/features/owner/`, Hosted
management handlers, affected typed Desktop wrappers and transfer tests.
Implement the [owner screen passports](HOSTED_RELAY_WEB_DESIGN.md#owner-panel)
WS-11–WS-23 in the web owner panel with H13's shared design system and distinct
owner authority. Deliver case/security/setup tasks as contextual actions, not
all-customer, key, payment, audit or settings dashboards. Desktop implements only the
[three-area operator scope](HOSTED_RELAY_ARCHITECTURE.md#desktop-hosted-scope), not
every web owner screen. Preserve the explicit Desktop-transfer flow instead of
treating web sign-in as synchronization.

- **H15.01 — Actions:** bind controls to server permission, fresh proof, revision, confirmation, audit and failure state.
- **H15.02 — Web inventory/pool:** preserve complete local lifecycle and policy semantics for sources, membership, rotation and publication; Desktop exposes only connection addition/status and explicit pool inclusion/removal.
- **H15.03 — Web access/usage:** separate logical/physical counts and trusted identity from User-Agent; no Applications launcher. Desktop receives scoped server-wide statistics/history, not customer-management screens or key administration.
- **H15.04 — Web customers/money/settings:** audited suspension/refund/adjustment and safe diagnostics. From Setup → Money, add/check/publish a merchant connection and its methods; show the resulting choices in customer Balance. Test that customers and paired Desktop devices cannot read drafts or configure checkout, and that owner reads never return a saved secret.
- **H15.05 — Transfer:** implement [SC-09](HOSTED_RELAY_ARCHITECTURE.md#from-connection-import-to-first-offer): inspect/snapshot/confirm/stage-and-drain/verify, preserving
  sources and recovery; no implicit synchronization. Transfer selected accounts/API
  sources with their effective proxies, protected proxy authentication and remapped
  bindings under the [proxy-preservation contract](HOSTED_RELAY_ARCHITECTURE.md#preserve-the-effective-proxy-during-transfer).
  Preserve explicit direct overrides and shared bindings without replacing the
  server's common proxy; unresolved proxy dependencies block activation rather
  than falling back to direct traffic.
  The [single-source Responses copy](../../hosted/server/README.md#copy-one-desktop-api-source)
  is covered by [`desktop_source_copy.rs`](../../hosted/server/tests/desktop_source_copy.rs).
  Shared-proxy mapping, batch preview and OAuth ownership handoff remain open.
- **H15.06 — Desktop console:** implement Overview, Usage, and Connections and pool with explicit active-server identity and operation-scoped pairing. Reuse revision/fresh-proof policy for allowed actions without inheriting full owner authority or overwriting local profiles.
  Implemented scoped rolling totals and in-flight counts, filtered request
  history with safe details, the connections inventory and passkey-confirmed
  pool inclusion/exclusion. Overview reads current pool readiness and measured
  generation speed without treating missing samples as zero.
  Installed-app acceptance remains open.
- **H15.07 — Web investigation:** canonical error/detail routes, nested attempts and bounded capture; no raw-payload or replay console. Desktop reads only granted safe request/error detail, not capture administration.
- **H15.08 — Web lifecycle:** implement SC-01/06/07/08 setup, readiness, guarded transitions and change/closure confirmations; do not duplicate service administration in Desktop.
- **H15.09 — Model workspace:** bind publisher-group and within-group ordering, per-model enablement and variant selection to H07/H04. Connections and Pool open the same purchase/sale editor. Test setting an API default, overriding one model, reverting to inheritance and opening the saved draft from the other entry point. The storefront changes only after publication takes effect.
  The restricted owner editor now saves group/model order with drag and keyboard
  controls; order changes need no price publication.
- **H15.10 — Owner notices:** the restricted `/admin/notifications` editor saves without sending and confirms a revision once. The customer journey proves atomic publication and recipient-only reads. Public rollout requires checkpointed fanout; Desktop pairing cannot publish.

**Acceptance:** `tests/{management_parity,transfer_recovery}.rs` and
`hosted/web/tests/e2e/{admin,diagnostics,service-lifecycle}.spec.ts` cover advertised actions,
authority/revision races, source-preserving transfer, safe counters/capture and
closure obligations. Polling/acknowledging cannot trigger inference, change money
or bypass readiness. Check mobile/keyboard and each shipped locale. Desktop acceptance
covers all three areas, explicit connection admission/removal, permission/expiry/
revocation failures and rejection of direct calls to web-only operations. Mode
switches do not upload local accounts, overwrite profiles or stop the local API;
closing Desktop does not stop server inference. Confirmed transfers retain the
existing ownership and recovery guarantees.
Web acceptance covers direct provider authorization and explicit local transfer
as separate flows, supported versus unavailable connector methods, and grouped
model controls with keyboard/mobile operation and stale-save recovery. Read-only
customer catalogs retain authorized prices/order without exposing owner controls.

Transfer acceptance also covers account/API-specific proxies, inherited local
common proxies, explicit direct overrides, shared-proxy ID remapping and protected
proxy authentication. Verify the same effective route after a server restart and
with Desktop closed. Missing/unreachable proxy dependencies must leave the target
blocked and preserve source recovery; reject secret leakage, endpoint-only proxy
deduplication and silent changes to unrelated server bindings.
Also cover per-connector transferability, ID/settings mapping, conflicts with
existing destination policies and uncertain apply acknowledgements. Reconciliation
must prevent two active OAuth refresh owners; transfer cannot publish local price
estimates as retail tariffs or overwrite unrelated server catalog settings.
Recovery after staged token rotation must use the current verified credential
generation rather than restore an obsolete source token.

#### H16 — Deployment, operations and recoverable release

**Owns:** HC-08/09/11, HD-05/07/09/10/11. Target: `hosted/deploy/`,
`hosted/server/README.md`, `src/management/diagnostics.rs`, diagnostic-session schema,
web distribution guide and operations acceptance fixtures.

- **H16.01 — Packaging:** pin compatible artifacts, document default/custom-site deployment and provide a source-build recipe that works in a fork without upstream CI credentials. The server build does not require the frontend.
- **H16.02 — Host/edge:** measured resources, private services, non-root containers, trusted origins and cancellation checks. Qualify SSE/WS at the edge for offered transports; reject unoffered operations/upgrades without an upstream send.
- **H16.03 — Visibility:** separate probe classes, bounded metrics/logs and external outage alerts with thresholds/recovery/runbooks.
  Unknown telemetry is not zero; keep source maps private and browser/server evidence distinct.
  The [owner operations view](../../hosted/server/README.md#owner-operations)
  reads saved cleanup and reconciliation passes with grouped failure/recovery history.
  The optional collector ingests bounded availability observations; independent
  live probes, scheduling and off-host outage alerts remain open.
- **H16.04 — Backup:** verified off-host pgBackRest/WAL, compatible vault/config and independently recoverable keys.
- **H16.05 — Restore:** isolated closed ingress; reconcile money, revocations, deletions and uncertainty before reopening.
  [Restore fencing](../../hosted/server/README.md#fence-an-isolated-restore) and
  the local physical backup drill cover pre-restore credentials and duplicate
  dispatch, credit and refund effects. Independently recoverable evidence,
  reconciliation of the missing interval and controlled reopening remain open.
- **H16.06 — Upgrade:** preflight, stop/drain, single migrator, readiness/smoke checks and schema-compatible rollback or roll-forward.
  The [read-only preflight](../../hosted/server/README.md#check-an-upgrade)
  checks the installed migration history against a candidate binary before downtime.
- **H16.07 — Runbooks:** key/identity/provider/storage/payment/migration failures with safe commands, stop conditions and recovery evidence.
- **H16.08 — Diagnostics:** owner-only fresh-proof/CSRF API, one bounded fenced capture, fixed expiry and persistent counters.
  Missing caps disable capture; optional sink failure cannot lose settlement/audit or cause inference.
  [Temporary diagnostics](../../hosted/server/README.md#temporary-diagnostics) implements the
  in-progress request scope, with an owner Operations control and local PostgreSQL/browser checks.
  Deployment-scale qualification remains open.
- **H16.09 — Readiness/outbound:** version-bound evidence and scoped destinations; no circular prerequisites or health gate on maintenance entry.
- **H16.10 — Lifecycle:** SC-07/08 drills preserve callbacks, refunds, exports and one active authority through closure/migration.

**Acceptance:** `tests/{operations_recovery,request_selection,readiness,outbound_policy,
service_modes,service_closure}.rs` and isolated deploy/restore drills prove cut points,
revocation, capture budgets, sink failure and retained obligations. Canary secrets
must be absent from API/log/support output. Measure actual RPO/RTO and workload;
documented objectives are not results. Deploys, restarts and paid probes need authorization.

#### H17 — End-to-end paid-product acceptance

**Owns:** HC-12 and every package's evidence. This is the public-release gate, not
another implementation shortcut. Real integrations require explicit authorization.

- **H17.01 — Customer:** qualify supported login → wallet/funding → key/model → correct charge/receipt/report → recovery.
- **H17.02 — Owner:** qualify setup/recovery, promised controls and safe investigation across authorization/retention boundaries.
- **H17.03 — Failures:** execute the cross-package matrix and [reference-derived cases](HOSTED_RELAY_ARCHITECTURE.md#selected-patterns-and-owning-packages) with Relay evidence.
- **H17.04 — Distribution:** qualify empty-host/default and custom-site installations, compatible artifacts and applicable WS-01–WS-24 surfaces. Test both the stock release and a downstream source build with a changed guide, its own branding and a test merchant. Block access to Zenith services: `/docs` must work anonymously, policy links must be the operator's and checkout must reach only the configured test merchant. With no merchant, funding stays disabled. `/app/help` stays absent.
- **H17.05 — Commercial:** resolve HD-01/02/09 with actual merchant, tariff, region/source permissions and customer terms.
- **H17.06 — Operations:** prove measured backup/recovery/capacity, alerts, sink failure, capture stop and rollback in production builds.
- **H17.07 — Sign-off:** trace SC-01–SC-09 to named tests and HD blockers; separate synthetic arithmetic from authorized live evidence.
- **H17.08 — Reopen rule:** bind sign-off to server/web/schema versions, offered operation and transport, credential/merchant scope, tariff and recovery evidence. Recheck affected gates after a change. Rehearse disabling just one offer and entering maintenance with pending holds and payments; do not roll back the journal or reopen on an old green check.

**Exit:** all first-release obligations have verified evidence or the public scope
is explicitly revised by the product owner. No paid route is activated by default
merely because a container, login page or model listing works.

#### Journey test gates

Use the cheapest layer that can actually prove the claim. Existing
[web unit tests](../../hosted/web/tests/unit) and [browser specs](../../hosted/web/tests/e2e)
prove presentation with fixtures. The [core import fixture](../../crates/relay-core/tests/host_management/import_workflow.rs)
proves metadata preservation and failure propagation using an in-memory host.
Neither proves a server vault, tenant boundary, durable money movement or provider permission.

| Run when | Test boundary and required observations |
| --- | --- |
| Changing a pure rule, projection or component | Unit/contract vectors: exact rates and rounding; unknown vs zero; rolling interval edges; catalog order/visibility; per-row validation/confirmation. Use synthetic data and a fixed clock. |
| Changing a server handler, job, migration or connector host port | Isolated migrated PostgreSQL, test vault, fixture identity/payment callbacks, counting loopback provider/proxy. Call real handlers; assert durable rows, generations, balanced postings, attempt/send counts before and after failure/restart. No real external calls. |
| Connecting a tab or changing a served action | Run the **production-built** web client against that server as anonymous, customer A, customer B and owner in separate sessions. Follow the actual route/buttons; inspect authorized network responses and resulting server state. No preview entry, mocked mutation success or shared fixture snapshot. |
| Changing pairing, renewal or transfer | Run the Desktop adapter and disposable server together with isolated stores. Interrupt both sides at each ownership handoff, revoke pairing, restart and repeat with Desktop closed. Compare source/destination generations, active refresh owners and observed proxy route. |
| Before opening or reopening paid service | H16 isolated upgrade/restore/drain drills with pending requests/payments; H17 separately authorized real sign-in, merchant funding/refund and permitted provider/client receipt. Record installation, offered variant and versions. Never point CI at customer accounts or real money. |

**Action-to-test map.** Existing filenames below are preview regressions, not proof
of the service behavior in the last column. Named server suites are **planned** under
`hosted/server/tests/`; browser journey suites are planned under `hosted/web/tests/e2e/`.

| Action and current preview coverage | Planned suite | Required pass/fail evidence |
| --- | --- | --- |
| Landing → Models/prices → Start; sign in, return or sign out (`landing-entry.spec.ts`, `sidebar.spec.ts`) | `customer_identity.rs`, `customer-journey.spec.ts`, `customer-failures.spec.ts` | Anonymous prices even with registration closed contain no private rows. Cancelled callback creates no workspace; two valid returns create one. A protected deep link rechecks identity. Back after logout or late A response in B's session reveals no A data. |
| Balance → quote, top up, return, receipt/refund; Keys → create/edit/reissue/revoke (`billing.spec.ts`, `keys.spec.ts`) | `payments_postgres.rs`, `admission_postgres.rs`, `customer-journey.spec.ts` | Quote/cancel gives zero checkout calls; confirming an expired or changed quote is rejected. A lost confirmation recovers one intent; fake redirect gives no credit, duplicate verified event gives one. Drop issuance reply: one masked key, no second secret. Reissue leaves the old key active until confirmed revoke. A revoked/expired/over-budget key has zero new sends; refund and inference cannot spend the same funds. |
| External client → Overview → Usage → detail/export (`overview-charts.spec.ts`, `usage.spec.ts`) | `first_funded_request.rs`, `execution_recovery.rs`, `usage_reports.rs`, `customer-journey.spec.ts` | Use the [receipt oracle](#one-request-statistics-fixture). Assert both outcomes, exact units and tenant isolation across chart, table and CSV, not merely a successful response or screenshot. |
| Customer Models/Status; owner Models reads Pool rules (`models.spec.ts`, `status.spec.ts`, `owner-model-rules.spec.ts`) | `pool_catalog.rs`, `usage_reports.rs`, `admin.spec.ts` | Inject a new discovered ID: owner sees it without a frontend build; listing, customer scope, effective tariff and telemetry each require their own evidence. New customer receives approved standard access, not private routes. Order changes once, no draft price on storefront, no green without observations; removal of a grant clears cached customer rows. |
| Owner setup/passkey; Connections → account/file/API/proxy (`owner-api-sources.spec.ts`, `owner-proxies.spec.ts`) | `bootstrap.rs`, `admin_auth.rs`, `provider_onboarding.rs`, `admin.spec.ts` | Anonymous/customer cannot claim ownership or import. Try wrong issuer/principal, cancelled/expired consent, repeated callback, one invalid row and one unchanged duplicate. Only confirmed eligible items persist; a failed write never reports Saved. No pool membership, grant, published offer or secret echo. |
| Paired Desktop transfer and recovery (not a web-preview operation) | `transfer_recovery.rs`, `vault_jobs.rs`, H15 Desktop/server run | Copy static API source; stage/drain transferable OAuth. Include inherited authenticated proxy, shared binding, explicit direct and unsupported method. Kill after drain and after destination commit before reply. Same operation, source retained, at most one refresh owner; bad proxy auth produces no direct fallback. Verify destination renewal/dispatch after restart with Desktop closed. |
| Pool → picker/member pencil/rotation/rules (`owner-pool-picker.spec.ts`, `owner-pool-member-editor.spec.ts`, `owner-pool-runtime.spec.ts`, `owner-pool-rotation.spec.ts`) | `management_parity.rs`, `pool_catalog.rs`, `admission_postgres.rs`, `admin.spec.ts` | Cancel/undo unsaved selection changes nothing. Save, pause, park, restore and reorder; stale revision cannot overwrite. Review paid backup, cancel, then save and reload: only the selected offer/sources change, with zero upstream sends. Observe a blocked fixture request on its member/model; the highlight clears on finish. No predicted next member. Member order is not storefront order. |
| Owner Overview → Usage/case; Notifications → customer bell (`owner-workspace.spec.ts`, `owner-notifications.spec.ts`, `notifications.spec.ts`) | `diagnostics.rs`, `notifications.rs`, `diagnostics.spec.ts`, `customer-failures.spec.ts` | Show posted charges, not deposits. Inspect a masked request/refund by reference without impersonating. Draft has zero recipients; duplicate publish/interrupted fanout gives each eligible customer one notice, later registrants none. A reading it leaves B unread and cannot change its target's state. |

Across these journeys, open/refresh/back, switch locale/theme or filters and
repeat a lost acknowledgement. Ordinary reads and presentation actions must not
make provider sends, create keys, enqueue imports, post money or publish notices.
Call the underlying API with wrong role/foreign IDs as well as checking the UI.
Unplug the backend: prices, funds and status become unavailable, not fixture
values; offer a safe read/status retry, never an automatic repeat of a mutation.
Cover keyboard-only, narrow screen, supported locales and inaccessible clipboard
on the critical paths; do not multiply every database fault by every viewport.

#### One-request statistics fixture

Use the [SC-03 worked amounts](HOSTED_RELAY_ARCHITECTURE.md#money-scenarios-and-worked-checks)
as independent expected values, not totals calculated by the UI under test:

1. A verified synthetic payment funds A with **10.000000 USD**, B with nothing.
   At 23:55 UTC A sends one text request: 40,000 ordinary input tokens at 20 USD
   per million and up to 30,000 output at 40 USD per million. No cache/tier fees.
   Assert one **2.000000** hold, available **8.000000**, one physical send.
   B cannot use A's key record, wallet, request lookup or export via B's session.
2. Final evidence is 10,000 output tokens: **50,000** normalized tokens and a
   **1.200000** charge. Post after midnight. Assert one closed hold, zero reserved,
   **8.800000** available, one logical request. Repeat the same idempotent POST,
   payment event and settlement delivery: same logical ID, no additional send,
   credit or debit. A status read is not a replay of the generated answer.
3. Pin UTC `[from,to)`, model, key and report watermark. A's chart bucket for the
   **start day**, filtered Usage totals, request receipt and CSV must agree on
   1 / 50,000 / 1.200000. The journal debit has the **next day's posting time**.
   Owner aggregates include A once; B sees no rows/charge or private metadata.
   With a second funded B request in a separate run, owner totals include both;
   A's saved report remains unchanged. A foreign export cannot be downloaded.
4. In an independent streaming case, forward substantive output but supply no
   usable provider usage, partial or final.
   Observe held/pending, advance the controlled clock to the saved deadline,
   settle the approved **0.600000** estimate, then deliver authoritative
   **0.400000** evidence twice. Assert one **0.200000** credit, **9.600000**
   available, no second request/send. A report pinned before the correction
   retains its watermark or explicitly expires; a fresh report for the same
   start window shows net **0.400000** and the later posting. An unknown token
   count is never presented as zero. Run this branch only once streaming is offered.

#### Cross-package failure and acceptance coverage

| Scenario | Primary packages | Required result |
| --- | --- | --- |
| Two callbacks register one subject | H04, H06 | One identity/workspace/wallet; no orphan credit account |
| Customer supplies a foreign resource ID | H02, H06, H09, H12, H14 | No cross-workspace data, charge, grant or existence disclosure |
| Customer A closes a workspace with a dispatched request and a pending payment; B stays active | H04, H06, H08–H10, H12, H14, H16 | A cannot create new keys, funding intents or dispatches. Close the original hold once and reconcile a later verified payment once, even if its callback repeats. Closure alone sends no refund. After restore, A's revoked key still fails; B's access and wallet are unchanged. |
| Two keys spend the same remaining balance | H04, H09 | Guarded reservation, no negative wallet or multiplied limits |
| Two tenants share one member; one loses a grant while queued | H01, H07, H09, H10 | No doubled physical capacity or private-state reuse; the denied dispatch has zero upstream sends |
| A second generation starts on an open socket after key revocation | H01, H09, H10 | Previous handshake/hold authorizes nothing; reject before send and finish the first request independently |
| Provider evidence exceeds the qualified price bound | H04, H07, H10 | Debit at most the hold; preserve uncapped evidence and stop selling the affected variant until its bound is verified |
| New model discovered; then tariff, grant or monitoring goes stale | H07, H12, H14, H15 | Inventory updates without frontend constants; public, authorized, status and runnable projections retain their own rules |
| Commit succeeds but acknowledgement is lost | H04, H08, H09, H10 | Lookup/reconcile same operation; no extra generation/credit/debit |
| Multiple events describe the same payment | H08 | One economic credit independent of distinct delivery event IDs |
| Process dies after dispatch intent | H03, H10 | Proven not-sent or uncertainty; no lease-based blind resend |
| Client disconnects after output | H10 | Supported cancellation/evidence policy, not automatic free execution |
| Late usage differs from estimate | H04, H10, H12 | Lower amount credited once; higher expense not extra customer debit |
| Source is removed during refresh/request | H03, H07, H15 | Fence preserved; no secret resurrection or broken history |
| Continuation is foreign/expired/partially saved | H11 | Clear rejection; no cross-user replay or silent extra retention |
| Browser signs out before an old response arrives | H13, H14 | Cache/UI cannot expose previous authenticated data |
| Detail expires while money evidence is retained | H04, H12 | Honest detail-unavailable state; financial totals remain auditable |
| Refund/chargeback arrives after spend | H04, H08 | Explicit obligation/restriction/compensation; no destroyed history |
| Restore point predates revocation or a payment | H03, H08, H16 | Reconcile/invalidate affected state before ingress; never trust old DB alone |
| Backend/web versions differ | H02, H13, H16 | Qualified compatibility or clear refusal, not misinterpreted money/schema |
| Single host and colocated monitoring are lost | H16 | External alert plus independently usable backup/key recovery |
| Confirmed cancellation versus unknown disconnected outcome | H10, H12, H14, H15 | Distinct explanation/counters; no unsupported refund, outage alert or blind resend |
| Failed attempt recovers within a successful logical request | H10, H12, H15 | Nested attempt evidence, no extra failed request or duplicated customer charge |
| Response completes but settlement remains unresolved | H04, H10, H12, H14, H15 | Successful execution and pending accounting both visible; status lookup does not regenerate |
| Upstream error contains secrets, markup or customer content | H12, H14, H15, H16 | Allowlisted safe evidence only in API/UI/logs/support output; no raw-message fallback |
| Optional diagnostics sink drops events or fills storage | H03, H10, H12, H16 | Partial/unavailable coverage and bounded health signal; durable money/audit safeguards remain intact |
| Capture races, expires, loses authority or restarts | H03, H05, H16 | Single scoped allowance, fixed deadline and recoverable counters; otherwise capture stops, no implicit extension |
| Browser render/read fails after a mutation may have committed | H08, H09, H13, H14 | Safe fallback and same-operation status recovery; no invented zero balance or repeated effect |
| Refund approval races inference spending | H04, H08, H09 | Separate atomic refund encumbrance; same funds cannot be spent/refunded twice |
| Notice delivery is duplicated, unknown or no longer consented | H03, H06, H12, H14 | One logical notice, bounded delivery/recheck, no money/access mutation or unverified recipient |
| Readiness expires or config changes before activation | H02, H05, H15, H16 | Revision-checked activation rejects stale evidence; no frontend bypass |
| A scheduled price/removal job is late or runs twice | H04, H07, H09, H12 | One effective revision; old holds protected, inconsistent new admissions blocked |
| Maintenance/closure races queued work and payment callbacks | H02, H08, H09, H10, H16 | New effects fenced, accepted obligations reconciled, no blind replay or discarded money |
| Owner moves host/domain or retires the installation | H03, H05, H08, H12, H16, H17 | One active authority, reviewed identity migration, retained obligations/export/revocation |
| One caller floods login, keys, queue or integration URLs | H05, H06, H09, H16 | Bounded scoped work and outbound authority, no victim enumeration, cross-tenant access or global budget multiplication |

For each row, expand test IDs, exact fixtures, injected cut point, expected states,
money postings, public error and operator signal in the owning package before code.
This matrix defines the minimum review coverage.

#### Scenario evidence checklist

For each scenario, the owning package attaches its exact step/test IDs and expected
customer/owner text to the canonical walkthrough. These are planned acceptance
artifacts, not files claimed to exist in the current repository.

| Scenario | Principal acceptance artifacts | What must be proved |
| --- | --- | --- |
| SC-01 | `tests/bootstrap.rs`, `tests/readiness.rs`, `hosted/web/tests/e2e/service-lifecycle.spec.ts` | Default closed, recoverable owner setup, current evidence and explicit public activation |
| SC-02 | `tests/first_funded_request.rs`, `tests/customer_identity.rs`, `tests/admission_postgres.rs`, `hosted/web/tests/e2e/customer-journey.spec.ts` | One verified identity/wallet/key journey, honest paid request and safe lost-response recovery |
| SC-03 | `tests/accounting_postgres.rs`, `tests/payments_postgres.rs`, `tests/execution_recovery.rs` | Exact worked amounts, balanced journal and once-only reserve/charge/refund/correction |
| SC-04 | `tests/diagnostics.rs`, `tests/diagnostic_capture.rs`, `hosted/web/tests/e2e/diagnostics.spec.ts` | Known versus unknown cause, safe investigation and bounded capture without replay |
| SC-05 | `tests/notifications.rs`, `hosted/web/tests/e2e/customer-failures.spec.ts` | Recipient isolation, consent, deduplication, expiry and no financial side effects |
| SC-06 | `tests/pool_catalog.rs`, `tests/admission_postgres.rs`, `hosted/web/tests/e2e/service-lifecycle.spec.ts` | Notice/effective-time behavior, saved tariffs and emergency-disable distinction |
| SC-07 | `tests/service_modes.rs`, `tests/operations_recovery.rs`, isolated restore drill | Admission fence, surviving obligations, trustworthy recovery and independent outage signal |
| SC-08 | `tests/service_closure.rs`, `tests/payments_postgres.rs`, isolated migration drill | Single active authority, funds/exports/late events and separately verified teardown |
| SC-09 | `tests/provider_onboarding.rs`, `tests/transfer_recovery.rs`, `tests/pool_catalog.rs`, `hosted/web/tests/e2e/admin.spec.ts` | Direct onboarding and paired transfer stay distinct; preserve proxy/ownership, park first, explicitly admit and publish, prove visibility and observed status separately |
