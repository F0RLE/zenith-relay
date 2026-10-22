# Zenith Relay roadmap

Open work, not shipped features or a progress log. Current contracts are in
[PLANNING.md](PLANNING.md); development/release checks are in
[CONTRIBUTING.md](../../CONTRIBUTING.md).

## Hosted multi-user Relay

The first public Hosted release is a complete prepaid model-token service backed
by operator-managed account/API-provider capacity, not an admin-panel MVP. It has
owner-configured Google/Telegram/OIDC customer login, one identity/wallet across
channels, scoped keys, direct inference and local-pool control parity. Desktop is
an operator client; the Hosted panel has no local Applications section.
The [Hosted target contract](HOSTED_RELAY_ARCHITECTURE.md)
owns product decisions, launch defaults and safety gates; it is not shipped behavior.

Use its [recommended first-production stack](HOSTED_RELAY_ARCHITECTURE.md#recommended-first-production-stack)
and [empty-host acceptance sequence](HOSTED_RELAY_ARCHITECTURE.md#from-an-empty-host-to-paid-service)
as the baseline. Technology selection does not mean Hosted dependencies or deployment
artifacts already exist. The retired personal-server runtime is not a Hosted foundation.

### Delivery stages

These are the high-level sequence. The [implementation work packages](#hosted-implementation-work-packages)
decompose them into design gates, concrete artifacts, failure cases and acceptance.
Complete the affected [specification checklist](HOSTED_RELAY_ARCHITECTURE.md#specification-before-implementation)
before code; broad stage labels alone are not implementation instructions.

1. **Core acceptance:** qualify the existing native/converted routes, shared
   request budget, terminal outcomes, ownership, and attempt accounting needed
   by Hosted. Do not rebuild the existing four codecs or route-binding model.
2. **Hosted foundation:** implement the documented owner/customer permissions,
   retention and USD tariff policy with PostgreSQL identities/workspaces, audit,
   vault references, scoped idempotency and continuation ownership/checkpoints.
   Use SQLx in the Hosted composition, controlled append-only migrations, bounded
   connection pools and durable worker ownership; do not hold database transactions
   across upstream streams or introduce Redis as a mandatory dependency.
   The first administrator panel uses [passkey-only sign-in](HOSTED_RELAY_ARCHITECTURE.md#passkey-only-administrator-access),
   with explicit owner provisioning, revocable sessions, and server-local recovery.
   Add owner-configured Google, Telegram and generic OIDC customer sign-in, secure
   linking/recovery and suspension without granting installation administration.
3. **Pool/catalog:** revisioned management, route validation, grants, scoped
   model/context projection, and explicit OAuth `stage_and_drain` transfer.
   Deliver credential inventory/parking and the existing local pool's controls,
   with honest active-model/request counts. New account connectors must qualify
   on Desktop and server together; generic protocol support is not account login.
   Qualify automation parity, including weekly quota wake and secondary-window
   validation, against the current local-pool contracts.
4. **Accounting:** journal, guarded reservations, settlement, reconciliation,
   payment idempotency, refunds, and uncertainty. Prove real-PostgreSQL races
   and crash recovery before paid traffic.
5. **Inference:** tenant/key admission through `relay-core`, SDK-shaped errors,
   usage attribution, and committed continuation recovery. Enable only
   commercially permitted routes with supported billable usage and finite bounds.
6. **Customer/operator surfaces:** complete Overview, Connections/inventory, Pool,
   Access without Applications, Usage, customers/payments and settings. Customers
   get models/prices, top-up/receipts, keys, own usage and recovery. Attribute usage
   by authenticated user/key with a separate untrusted User-Agent label. Website
   and any Telegram bot share the same account API; bot UI is not Telegram login.
   Deliver the neutrally branded landing page/cabinet as `web/` inside `zenith-relay`,
   with its own build/tests/artifact and an alternative custom-site integration path
   against the same customer API. Do not create a separate repository for launch.
   Reuse React/TypeScript/Vite components through typed Hosted API transport, with
   separately built web assets and no browser dependency on Tauri or a Node backend.
   Follow the [customer website contract](HOSTED_RELAY_ARCHITECTURE.md#customer-website)
   for page scope, graphs, financial semantics, privacy and acceptance.
   Revisioned local/hosted transfer never synchronizes secrets implicitly.
7. **Operations and paid acceptance:** version-pinned Docker/TLS, private durable
   stores, liveness/readiness/inference checks, bounded admission, alerts, retention,
   migrations and isolated restore drills. Prove the target RPO/RTO and real client/
   provider behavior. Verify the actual merchant/payment channel, launch region,
   financial retention and commercial permissions before enabling paid routes.
   Deliver the Linux/Compose/Caddy baseline, private Prometheus/Grafana/Alertmanager,
   external outage checks and pgBackRest off-host recovery with coordinated vault/key
   handling. Publish verified setup/update/recovery procedures; test SSE/WebSocket
   proxy behavior and single-host failure recovery, not only container startup.

Internal stages do not reduce the first-public-release scope. One qualified
account family is enough initially; additional Claude/other account connectors
are future work, not promised support inferred from an API adapter.

Excel/BPS deep acceptance and additional subscription connectors are not a
prerequisite for a Hosted release that does not offer those credential classes.
No shared paid route is inferred from a working personal account.

### Customer website implementation slices

These are internal delivery slices of the complete customer cabinet, not reduced
public-release scope. The [website specification](HOSTED_RELAY_ARCHITECTURE.md#customer-website)
owns screen and metric definitions; customer state and billing rules remain
server-owned rather than duplicated in frontend implementations.

1. **Web application and public contract:** add the default landing page/cabinet
   under `web/` in the existing Relay repository, with its own package manifest,
   commands, tests and build artifact. Preserve Desktop and Git history;
   no repository extraction is required. Publish versioned Hosted customer API/schema
   and synthetic examples. Initially build backend/web artifacts from the same
   tested release; document compatibility and default-versus-custom deployment.
   Follow the [web distribution contract](HOSTED_RELAY_ARCHITECTURE.md#one-repository-and-deployment-choice)
   for ownership, neutral configuration and community contribution boundaries.
   Review coupled API/client changes together and run both sides' affected checks.
   Create the [original Relay design](HOSTED_RELAY_ARCHITECTURE.md#original-relay-design),
   not an imported/re-skinned frontend. Reuse identified utilities/dependencies with
   provenance and license notices. Configure branding and service endpoints; keep
   credentials out of source and public build artifacts.
2. **Shell and identity:** a separately deployable Relay web entry, configured
   same-origin API transport, customer session boundary, navigation, locales/themes,
   original design tokens/components, configurable neutral branding/landing content,
   responsive layouts and explicit loading/empty/error states. Domain/brand are
   deployment configuration, not another frontend fork. Isolated mock previews are
   not live features; reference-project screenshots are not design specifications.
3. **Overview and usage:** server-owned aggregates, current wallet/activity strips,
   consistent UTC filters, requests/tokens/cost/latency charts, model/key breakdowns,
   paginated history, safe detail and bounded exports. Prove snapshot/coverage and
   request-charge reconciliation before presenting graphs as financial truth.
4. **Access and money:** authorized model/pricing catalog, one-time key issuance,
   limits/revocation, connection help, payment intent/status, receipts and journal.
   Complete login linking/session recovery and pending/refund/suspension flows;
   never simulate credited balances or fulfilled paid requests in customer mode.
5. **Customer and distribution acceptance:** synthetic multi-workspace isolation, financial/usage
   edge cases, cache/sign-out safety, delayed response races, both locales/themes,
   mobile/keyboard/chart-table parity and deep links. Verify neutral-brand/domain
   setup from released artifacts, independent-client operation without the default
   website, version compatibility and frontend rollback without state loss. Document
   local development, contribution/security reporting and the release workflow.
   Then perform separately authorized real login/payment/provider acceptance before
   enabling paid traffic.

### Hosted implementation work packages

The [web design specification](HOSTED_RELAY_WEB_DESIGN.md) defines WS-01–WS-24,
visual/interaction baselines and the design-review sequence. H00 reviews this
presentation contract before H13/H14/H15 implement it. Complete screen descriptions
and mockups do not close runtime, accessibility, security or paid-release gates.

The packages below are planned work, not a completion log. Keep their identifiers
stable when decomposing them further. Architecture owns target names and semantics;
this plan owns dependencies, implementation order and evidence. Do not create a
second schema/API definition in task descriptions or treat a planned path as an
existing implementation.

For every subtask, record: scenario/invariant; prerequisite package and unresolved
decision; exact files and reused entry points; schema/API/state changes; reads and
writes; permissions and sensitive data; transactions and external effects; failure,
retry and recovery; UI/configuration/operations impact; named checks and expected
results. Before implementation, all fields affected by that subtask must have a
concrete answer or an explicit blocking decision. "Add billing", "handle errors"
and "test authorization" are not sufficient task definitions.

Documentation acceptance checks links, names, transition coverage, schema/API
consistency and requirement-to-test traceability. Implementation acceptance proves
the behavior with actual code and the appropriate database/client/runtime tests.
Paid acceptance additionally requires separately authorized real integrations and
recovery evidence. Internal completion of one package never narrows public scope.

#### Package dependency map

Paths below are future artifacts unless marked existing. The
[implementation contract map](HOSTED_RELAY_ARCHITECTURE.md#implementation-contract-map)
owns target names/types, table inventory, transaction rules, API names and open
decisions. Use `Hxx.nn` for subtasks and `HC-xx`/`HD-xx` for contract/decision
references in their acceptance descriptions. These are not dates, estimates or
claims that the package has started.

| Package | Result | Prerequisites |
| --- | --- | --- |
| H00 | Reviewed design/decision coverage | Existing Hosted contract and current-code boundary |
| H01 | Verified shared-core integration/reuse map | H00 |
| H02 | Hosted composition, configuration, schema/migration and API foundations | H00, H01 |
| H03 | Vault consistency, durable jobs and domain outbox foundation | H02, HD-05 |
| H04 | Tariffs, ledger, wallet and reservation primitives | H02, H03, HD-04 |
| H05 | Installation owner, passkeys and scoped Desktop pairing | H02, H03, HD-03 |
| H06 | Customer identity, sessions, linking and recovery | H04, H05, HD-03 |
| H07 | Inventory, pools, catalog, grants and qualified routes | H01, H03, H04, H05 |
| H08 | Top-up, verified events, refunds and payment reconciliation | H04, H06, HD-02 |
| H09 | Keys, limits, request sizing and scoped idempotency admission | H04, H06, H07, HD-06 |
| H10 | Core execution, settlement, cancellation and uncertainty | H03, H09 |
| H11 | Committed continuation and restart recovery | H03, H10 |
| H12 | Usage/diagnostic/reporting/notice/export/retention projections | H08, H10; H11 for resume reporting; HD-13 only for optional external notices |
| H13 | Default web shell, typed API and branding | H02 contract; H05/H06 for authenticated acceptance; H07 for public model/pricing acceptance |
| H14 | Complete customer cabinet | H08, H09, H12, H13 |
| H15 | Complete operator surfaces and Desktop parity/transfer | H05, H07, H08, H12, H13 |
| H16 | Release packaging, operations, backups and recovery drills | H02/H03 foundation; H10–H15 for full acceptance |
| H17 | Paid-product acceptance and supported deployment matrix | All prior packages and applicable HD decisions |

Infrastructure/runbook design and web scaffolding can proceed against frozen
contracts before the runtime is finished. Do not call mocked screens live features
or interpret parallelizable documentation as authorization to run external services.
The first internal end-to-end proof is identity → verified funding → key → execution
→ exact charge → report → failure/recovery. It is not a reduced public release.

#### H00 — Specification and decision coverage

**Owns:** HC-01–HC-12; updates remain in this architecture/roadmap pair. No new
implementation, production configuration or parallel architecture document.
The web design document owns presentation only; update its affected screen
passports when canonical routes, permissions or data meanings change.

- **H00.01 — Scope:** enumerate customer/owner/operator journeys, first-release
  obligations and exclusions; distinguish default/custom frontend deployment paths.
- **H00.02 — Decisions:** assign owners/evidence to HD-01–HD-13. Separate a design
  decision, deployment value and externally verified fact; never fill missing
  commercial permission or prices with a plausible-looking assumption.
- **H00.03 — Dictionaries:** finish each package's field/API/state/permission/config
  passport before its implementation. Include null/default/range and delete behavior,
  not just entity names. Record consumers and a canonical location for each contract.
- **H00.04 — Threat/failure coverage:** walk unauthorized access, duplicate actions,
  races, disconnect, dependency outage, disk full, interrupted commit and restore.
  Map each invariant to a named test and an observable expected outcome.
- **H00.05 — Change control:** a changed contract updates affected schemas/examples,
  UI meanings, tests and runbooks together; preserve the implemented/proposed boundary.
  Map the [selected reference patterns](HOSTED_RELAY_ARCHITECTURE.md#reference-derived-implementation-decisions)
  to their owning packages, adaptations and named tests. Record inspected revision/
  files and contract differences; third-party source reuse has a separate provenance/
  license review. No reference frontend is an approved visual specification.
- **H00.06 — Scenario passports:** walk [SC-01–SC-08](HOSTED_RELAY_ARCHITECTURE.md#step-by-step-scenario-map)
  one step at a time. For each action record actor/preconditions, typed API inputs,
  permission, transaction/records, external effect, saved state/revision, visible
  RU/EN result, timeout/retry/restart behavior and acceptance test. An unresolved
  merchant/legal/measured value has a named HD blocker, never a guessed default.

**Design exit:** every downstream package has scope, artifacts, dependencies and
acceptance; unresolved details have a specific blocker, not "handle later".
**Checks:** document links/anchors, identifier coverage, duplicate vocabulary and
cross-layer consistency. This gate is not runtime or commercial qualification.

#### H01 — Shared-core qualification and reuse boundary

**Owns:** HC-01/04/06, HD-12. Existing anchors are `GatewayRuntime`,
`gateway::router`, `gateway::network_router`, core adapters/usage and Desktop
vault/refresh/configuration flows. Do not rewrite the four codecs.

- **H01.01 — Integration map:** identify exact construction/execution/usage/cleanup
  entry points and tests. Document what each receives/returns, who owns cancellation,
  physical leases and provider refresh, and what survives restart today.
- **H01.02 — Auth context:** define the server-created tenant/key/request context
  crossing core admission. A static pool key or client JSON must not
  become the shared identity of all Hosted customers.
- **H01.03 — Evidence:** specify observation points before protocol conversion for
  terminal outcome, normalized usage and attempt certainty; cover streaming and
  nonstreaming, tools, context/tier/cache semantics and late observations.
- **H01.04 — Retry:** map every existing retry/fallback path to pre-execution proof,
  ownership and output visibility. Unsafe uncertainty cannot become a Hosted retry.
  Adapt the selected connector cases for quota/capacity/rate-limit classification,
  cancelled ownership waits and supported transport failures to Relay's own tests;
  upstream tests are examples, not proof of provider semantics or Hosted acceptance.
- **H01.05 — Reuse/refactor:** select the smallest shared interface change, including
  vault reuse/extraction under HD-05. List Desktop/server callers before changing it.
- **H01.06 — Parity:** resolve or explicitly exclude unsupported owner mutations,
  including the documented weekly quota wake mismatch, before advertising parity.

**Acceptance:** focused existing core/server tests plus a Hosted-context synthetic
fixture prove attribution/isolation without changing local behavior. Retain exact
route-family evidence; a protocol adapter does not qualify an account connector.

#### H02 — Hosted composition, database and contract foundations

**Owns:** HC-01/02/07/11, HD-04/08/10. Target: `relay-hosted/{Cargo.toml,src/,
migrations/,openapi.yaml,tests/support/}`; existing builds remain independent.

- **H02.01 — Process:** specify/start the `zenith-relay-hosted` composition, lifecycle
  phases, startup validation, graceful admission stop and bounded shutdown.
  Initialization is not public owner signup or automatic personal-server migration.
- **H02.02 — Config:** expand every target setting into type/unit/default/range,
  secret source, precedence and reload rule. Fail before admission on incompatible
  schema, invalid origin, missing key or unbounded mandatory limits.
- **H02.03 — Schema spine:** settle the full relational dependency graph, composite
  tenant keys, operation uniqueness and migration order. Create only schema needed
  by the current package; later packages add their own append-only domain migrations.
- **H02.04 — DB boundary:** bounded SQLx pool, short transactions, runtime/migrator
  roles, migration exclusion, explicit scope and safe error classification. Specify
  transaction retry limits without retrying external effects.
- **H02.05 — API:** freeze route inventory, object/error envelopes, exact JSON types,
  protocol-vs-control errors, correlation IDs, pagination and compatibility version.
  Complete per-endpoint schemas in the domain package, not untyped handler stubs.
- **H02.06 — Harness:** disposable real PostgreSQL, migration runner and deterministic
  synthetic clock/provider/payment fixtures. No customer's database or credentials.
- **H02.07 — Service-control foundation:** define the canonical mode/admission-epoch
  schema and reads, protected qualification scope and revisioned transition primitive.
  New installations start closed in `setup`; restart cannot reset to `live` from
  configuration. H05/H15 attach owner authority, H06/H08/H09 enforce entry gates and
  H16 supplies readiness guards. No unprotected transition endpoint in the scaffold.

**Acceptance artifacts:** `tests/bootstrap.rs`, `tests/schema_postgres.rs` and
`tests/api_contract.rs`: fresh database, upgrade fixture, concurrent migration
attempt, unsupported schema, invalid secret/config, transaction rollback and tenant
context reuse. Record that no transaction/DB connection spans a provider stream.
`tests/service_modes.rs` proves default-closed boot, persisted mode, lost transition
acknowledgement and stale revisions before downstream packages expose public effects.

#### H03 — Vault generations and durable jobs

**Owns:** HC-09/11, HD-05. Target: `src/vault.rs`, `src/jobs.rs`, `src/jobs/`,
their migrations and `tests/vault_jobs.rs`.

- **H03.01 — Secret format:** qualify the Desktop vault primitive at
  `src-tauri/src/local_pool/store/vault.rs`;
  specify reference/version/type, encryption-key ID, bounds, file permissions and
  crash-durable generation manifest. No plaintext material in PostgreSQL/logs.
- **H03.02 — Commit protocol:** durable new ciphertext → transactional live reference
  and fence → eligible old-version cleanup. Spell out orphan handling and recovery
  when filesystem and DB operations fail in either order.
- **H03.03 — Rotation/deletion:** key/digest rotation overlap, refresh ownership,
  source removal and revoked-generation restore behavior. Missing key fails closed,
  not an empty inventory or newly generated replacement key.
- **H03.04 — Jobs:** exact payload schema per kind, unique dedupe identity, claim/CAS,
  lease/fence, heartbeat, retryable errors, backoff ceiling, deadline and exhausted
  state. External uncertainty enters reconciliation, never generic resend.
- **H03.05 — Operations:** safe queue/lease lag diagnostics, cleanup visibility and
  vault-generation references in backup manifests. Archive failure must be observable.
- **H03.06 — Domain outbox:** define typed domain-event envelopes and same-transaction
  outbox append with unique event identity, bounded fanout, checkpoints and cleanup.
  Reuse the job runner; no broker or side-effecting callback inside a money commit.
  H12 supplies customer-notice projection/delivery without moving business ownership.

**Acceptance:** wrong/missing key, corrupted/truncated file, disk-full write, crash
before/after reference update, duplicate/stale worker, expired lease and restart.
Old authority cannot write after revocation; losing a job does not lose money work.
Outbox replay after crash/commit-ack loss retains one logical event and cannot repeat
the source payment, refund or inference effect.

#### H04 — Exact tariffs, journal and wallet primitives

**Owns:** HC-05/09, HD-04. Target: `src/accounting.rs`, `src/accounting/`,
accounting migrations and `tests/accounting_postgres.rs`.

- **H04.01 — Price schema:** fully define component/context/tier boundaries, revision
  publication, rates, zero-price evidence, checked arithmetic and one final rounding.
  Include synthetic examples for overlap, large counts, fractional charge and overflow.
- **H04.02 — Ledger:** complete accounts/postings for every money operation, supported
  currency, uniqueness and database enforcement of balanced transactions. Specify
  reconstructible available/reserved projections and mismatch handling.
- **H04.03 — Wallet creation:** transactional initializer used by H06 registration;
  no separate lazy wallet creation that races identity callbacks.
- **H04.04 — Reserve/close:** implement guarded reserve, close, release, late credit
  and safe same-operation lookup after lost commit acknowledgement. Preserve tariff,
  sizing/policy version and immutable closure evidence.
- **H04.05 — Reversals:** settle the policy for partial/full refunds, chargeback after
  spend, restricted accounts and operator adjustment before implementing those paths.
  Every operation has actor/reason/reference and a customer-visible meaning.
- **H04.06 — Contention:** enumerate lock order and races among multiple keys sharing
  a wallet, top-up, suspension, tariff edit and competing closers.
- **H04.07 — Worked money cases:** implement the synthetic
  [SC-03 checks](HOSTED_RELAY_ARCHITECTURE.md#money-scenarios-and-worked-checks)
  in integer microUSD, including hold/release, bounded estimate, lower/higher late
  evidence and duplicate payment credit. Add separate refund-reserved postings and
  public inference/refund/total-reserved reconciliation. Unknown is never zero;
  fixture amounts are not tariffs. HD-04 resolves spent-funds reversals before launch.

**Acceptance:** real PostgreSQL concurrent spends never overdraw; duplicate closure
does not change totals; all postings balance; rollback leaves no partial hold;
deadlock/timeout retry and commit-ack loss resolve one operation. Zero/unknown are
distinct, overflow rejects, journal replay rebuilds matching projections.
Run the same worked cases with repeated close/refund operations and concurrent
spend; refund funds are unavailable to inference until a proven release commits.

#### H05 — Owner provisioning, passkeys and Desktop authority

**Owns:** HC-03/10, HD-03/08/12. Target: `src/auth.rs`, `src/auth/`,
owner/device schema and `tests/admin_auth.rs`.

- **H05.01 — First owner:** exact server-local setup command/input and one-use
  enrollment ceremony; installation binding, expiry, restart/concurrent consumption
  and atomic verified credential/owner assignment. No first-public-user ownership.
- **H05.02 — WebAuthn:** library-backed options/verification DTOs, RP ID/origins,
  discoverable credential and user-verification requirements, supported counter/backup
  behavior. Revalidate official contracts, not handwritten crypto assumptions.
- **H05.03 — Sessions:** exact cookie names/attributes, CSRF transport, idle/absolute
  expiry, authorization epoch and fresh-verification action matrix.
- **H05.04 — Recovery:** second credential, last-credential guard, lost-all-keys
  server-local recovery, revocation and audited re-enrollment. No weaker public reset.
- **H05.05 — Desktop:** owner-approved bounded device pairing, scopes, revocation and
  browser step-up for sensitive actions. Pairing cannot become a permanent bypass
  of passkey-only human admin sign-in.

**Acceptance:** wrong origin/RP, missing verification, replayed/expired challenge,
foreign credential, stale role, CSRF, session fixation, revoked device and setup race
all fail safely. Real authenticator acceptance is separate from synthetic fixtures.

#### H06 — Customer registration, linking and recovery

**Owns:** HC-03, HD-03/08 and conditional HD-13. Target: `src/identity.rs`, `src/identity/`, customer
auth handlers and `tests/customer_identity.rs`; use H04 wallet initialization.

- **H06.01 — Integrations:** separate verified Google, Telegram and configured OIDC
  adapters with exact issuer/client/redirect/state/nonce/PKCE behavior and origin
  allowlists. A forwarded profile, bot message or matching email is not proof.
- **H06.02 — Register:** one verified subject → one person/workspace/wallet; specify
  duplicate callback and simultaneous sign-in behavior transactionally.
- **H06.03 — Link/unlink:** fresh current proof plus proof of new identity; prevent
  cross-user reassignment and removal of the last usable login method.
- **H06.04 — Session/recovery:** revocable customer cookies, CSRF, provider outage,
  returning user, session expiry and another linked method. Customer auth cannot
  create owner/device permissions.
- **H06.05 — Suspension/deletion:** block every linked entry point and key issuance;
  audit explicit cancellation separately. Retain required money evidence while
  deleting eligible personal data and preparing restore tombstones.
- **H06.06 — Abuse and recipient proof:** bound login/registration/link attempts,
  challenge lifetime/storage and verification messages without permanent victim
  lockout or user enumeration. Enforce service mode and verified qualification scope
  at registration. Optional notification-recipient linking needs its own consent,
  single-use proof, destination limits and revocation; login claims alone do not
  authorize messages. No external channel implementation before HD-13 qualification.

**Acceptance:** `tests/customer_identity.rs` covers all three adapter paths,
concurrent callbacks, linking collision, last-method removal, provider disable,
CSRF/open redirect, suspension and recovery. Verify no second wallet and no email
merge. Live login acceptance is an independently authorized launch gate.
Cover repeated registration attempts, qualification-scope spoofing, recipient change
while delivery is queued and unverified/foreign recipient IDs without customer leakage.

#### H07 — Credential inventory, pools, catalog and route grants

**Owns:** HC-04/09, HD-01/12. Target: `src/management.rs`, `src/management/`,
`src/catalog.rs`, `src/catalog/`, capacity migrations and `tests/pool_catalog.rs`.

- **H07.01 — Inventory:** complete schema/actions for account/API sources, labels,
  secret references, authorization generation, refresh and health. Explicit scope
  determines installation-shared versus private/granted access.
- **H07.02 — Lifecycle:** specify add, reauthenticate, park, enable, disable, drain,
  trash, restore and remove separately. Fence dispatch/refresh before destructive
  changes; external reauthentication must not silently create a different owner.
- **H07.03 — Parity:** map every existing local-pool control to its shared validator,
  Hosted method/DTO, permission, revision and test. Include order/rotation, proxies,
  quota automation, context/reasoning/tier, tool/timezone rules and price publication.
- **H07.04 — Catalog:** project exact usable routes/models/protocols under workspace
  grants/key narrowing and published tariff. Capability, entitlement, permission,
  quota and price remain independent evidence; an alias is not new capacity.
  Define the bounded `GET /public/models` publication separately for landing-page
  prices: public qualified descriptors and complete tariff bands/revisions only,
  with freshness and no customer grants/private capacity. Unpublished/disabled or
  stale data cannot masquerade as publicly purchasable access. Test anonymous versus
  authenticated catalog isolation; do not add prices to `/public/config`.
- **H07.05 — Qualification:** persist HD-01 evidence and finite usage/bounds for each
  paid route. Unqualified routes remain unavailable for sale. Shared core discovery
  and runtime validation remain authoritative rather than a separate model registry.
- **H07.06 — Mutation races:** expected configuration revision and runtime activation
  state; concurrent refresh/removal, tariff changes and draining requests. Dashboard
  reads cannot activate providers or trigger discovery probes.
- **H07.07 — Published changes:** implement [SC-06](HOSTED_RELAY_ARCHITECTURE.md#price-and-model-change-walkthrough)
  using immutable tariffs/config revisions, affected-scope preview, notice window,
  atomic publication/outbox and fenced due activation. Specify cancellations,
  superseded changes and emergency disable without hidden price increases. Preserve
  existing holds; block affected new admissions when a due revision cannot activate.

**Acceptance:** `tests/pool_catalog.rs` covers inventory without membership, all
lifecycles, stale revision, foreign IDs, grant revocation, alias slot sharing,
inactive/unknown quota and failure to activate a changed configuration. Qualify
Desktop/server account families together; document unsupported mutations honestly.
Use controlled-clock tests for scheduled changes, missed/duplicate activation jobs,
new versus held/queued requests, notice cancellation and incompatible route revisions.

#### H08 — Payments, receipts, refunds and external reconciliation

**Owns:** HC-05/09/12, HD-02/04/09. Target: `src/payments.rs`, `src/payments/`,
payment migrations, webhook routes and `tests/payments_postgres.rs`.

- **H08.01 — Adapter contract:** select the actual merchant/provider and define
  provider IDs, intent/event fields, currency quote, redirect/callback origins,
  signature verification and idempotency. Revalidate official provider documentation
  before implementation; no assumed processor, FX rate or availability.
- **H08.02 — Intent:** authorize workspace, validate requested top-up, quote immutable
  amount/currency/credit, persist local operation, call provider outside locks and
  bind its reference. Specify timeout and expired/abandoned payment recovery.
- **H08.03 — Event:** verify signature/merchant and match server intent before storing
  allowed evidence. Duplicate, reordered and delayed events drive explicit states;
  HTTP acknowledgement follows durable processing/queueing policy, not wishful credit.
- **H08.04 — Credit/receipt:** one journal credit and receipt reference per verified
  economic effect. Multiple distinct event IDs for one payment cannot duplicate it.
  Customer redirect/polling shows pending until authoritative credit exists.
- **H08.05 — Refund/dispute:** request versus approved external refund, refundable
  remainder, in-progress reservation, partial refund and chargeback after spend.
  Specify accounting compensation, restrictions, notification and audit per state.
- **H08.06 — Reconcile:** provider queries resolve lost acknowledgement and missed
  webhook; retries use durable operation IDs and cannot resend unknown effects blindly.
- **H08.07 — Refund and lifecycle safety:** specify requested/approved/reserved/
  externally unknown/final refund behavior using the canonical separate encumbrance.
  Bind amount/currency/provider reference and the original refundable remainder;
  guard concurrent spend/refund. Service mode blocks new funding intents but not
  verified existing-payment events or required reconciliation. Publish notices from
  committed effects; delivery/read state cannot change money. HD-02/04/09 own disputes.

**Acceptance:** `tests/payments_postgres.rs` proves valid/invalid signature, wrong
merchant/amount/currency, replay, reordered events, event-ID diversity for the same
payment, simultaneous credit, failed commit, partial refund and spent-funds reversal.
Synthetic tests do not establish actual merchant onboarding or successful live money.
Test refund acknowledgement loss, late success after maintenance/closure, multiple
refund approvals against one balance and an outbox/channel outage after credit.

#### H09 — Keys, limits, sizing and admission

**Owns:** HC-03/05/06/07, HD-06/07/08. Target: `src/access.rs`, `src/access/`,
admission parts of `src/inference/` and `tests/admission_postgres.rs`.

- **H09.01 — Keys:** exact key encoding/public prefix/verifier, one-time issue,
  listing, rename, expiry, replacement and revoke. Bind assigned user separately
  from untrusted labels; never return stored secrets in a list or retry response.
- **H09.02 — Policies:** spell out every workspace/key operation/model/limit field,
  precedence and units. Multiple keys cannot multiply workspace concurrency/spend.
  Recheck suspension/revocation before dispatch, not only when queueing.
  Publish the same effective scope/units/reset semantics to key forms and detail;
  distinguish spending budgets, request/token rate, concurrency and wallet funds.
- **H09.03 — Sizing:** after continuation/bridge expansion, calculate the supported
  input bound and finite output/reasoning/cache/media/tool exposure. Snapshot the
  calculation and most expensive eligible context band. Unknown/unbounded rejects.
  H11 later qualifies persisted continuation expansion against this same sizing
  contract; no resume route is advertised from a single-turn admission test.
- **H09.04 — Idempotency:** exact canonicalization/default-snapshot rules, scoped
  key/request digests, rotation and retention window. Authenticate before lookup;
  different request conflicts, uncertain/terminal duplicate never dispatches again.
- **H09.05 — Admission transaction:** combine operation claim, scope/limit checks,
  bounded wallet reserve and durable request. No independent balance check followed
  by unconditional debit; lost commit acknowledgement reads the same operation.
- **H09.06 — Queue:** bounded workspace/global queue, cancellation/expiry, physical
  handoff and hold release. Route/bound change revalidates/re-reserves atomically
  before send; no running with an obsolete cheaper hold.
  Re-reservation cannot silently replace the saved tariff or exceed the approved
  spend bound. A scheduled change that invalidates the original contract follows
  SC-06 cancellation/release, not an unannounced higher-price reservation.
- **H09.07 — Admission fencing and fairness:** check service mode/qualification,
  current authorization epoch, due model policy and key/workspace budget before
  reserve and dispatch. Qualify many keys from one workspace against other customers,
  long/short streams and revoke/maintenance races. Reuse shared scheduler ownership;
  no second web-side queue or arbitrary caller-supplied workspace attribution.

**Acceptance:** `tests/admission_postgres.rs` covers concurrent keys/requests, revoked
or foreign IDs, exact-balance boundary, absent/zero price, overflow, queue deadline,
digest conflict, duplicated HTTP/SSE/SDK requests and dropped commit acknowledgement.
Assert both response semantics and absence of an extra hold/generation.

#### H10 — Execution, settlement and uncertainty

**Owns:** HC-05/06/09, HD-06/07. Target: `src/inference.rs`, `src/inference/`,
accounting settlement/reconciliation workers and `tests/execution_recovery.rs`.

- **H10.01 — Handoff:** use H01's core integration with authoritative tenant/key/
  request context. Persist fenced dispatch intent/attempt before send; do not hold
  SQL locks/connections while waiting for provider output.
- **H10.02 — Observation:** capture original upstream terminal/usage evidence before
  conversion. Specify logical and physical state transitions, cumulative usage
  merge, partial components and normalized safe errors for every offered protocol.
  Define the terminal-checkpoint integration point for H11; resumable success is
  gated on that package rather than introducing a second execution engine.
- **H10.03 — Streaming:** qualify first-byte, terminal event, backpressure, bounded
  duration, client disconnect and cancellation. HTTP success/heartbeats alone do
  not establish completion or substantive output; no unsafe automatic fallback.
- **H10.04 — Close:** normal supported usage closes once under the reserved tariff.
  Missing usage enters the fixed reconciliation deadline; implement documented
  partial/waiver/bounded-estimate policies without fabricated hidden components.
- **H10.05 — Late evidence:** resolve uncertain outcome where evidence permits;
  lower customer amount yields a unique compensating credit, higher expense remains
  operator exposure. Never reopen a closed hold or rerun an uncertain generation.
- **H10.06 — Restart:** enumerate before-send, after-send, mid-stream, terminal-before-
  commit and close-before-ack crash cases. Recovery owns the same request/fence and
  original deadline; pending output is not silently replayed or resubmitted.

**Acceptance:** `tests/execution_recovery.rs` with real PostgreSQL and controlled
synthetic upstream proves each cut point, partial/unknown usage, cancellation,
retry ownership, duplicate final events, accounting consistency and eventual bounded
hold closure. Real client/provider execution remains a separate qualification.

#### H11 — Continuations and durable completed turns

**Owns:** HC-06/09, HD-05/06. Target: `src/continuations.rs`,
`src/continuations/`, checkpoint schema and `tests/continuation_recovery.rs`.

- **H11.01 — Eligibility:** for each offered route, specify whether it uses native
  provider reference, compatible full client history or permitted encrypted payload.
  Reject unsupported resume; metadata alone cannot recreate signed/native content.
- **H11.02 — Scope:** fix lookup digest, tenant/key/provider/member/model/protocol/
  adapter/authorization binding, expiry and turn version; no foreign ID enumeration.
- **H11.03 — Checkpoint:** commit a complete turn before advertising terminal success;
  define completed tool calls versus incomplete arguments, pending IDs and branch
  children. Turn advancement is CAS/fenced, not last-writer-wins.
- **H11.04 — Payload:** explicit opt-in, schema/key version, encrypted reference,
  per-session/workspace/global size and TTL; no usage-screen or response-replay access.
- **H11.05 — Recovery:** restore only committed compatible unexpired authorized state;
  process key/grant/source revocation, identity changes, cleanup and upgrade mismatch.

**Acceptance:** `tests/continuation_recovery.rs` covers concurrent turns, missing
payload, incomplete tool arguments, wrong scope, expired/revoked state, graceful and
abrupt restart and compatible/incompatible adapter versions. Off means no payload
retention; exceeding limits cannot secretly enable more storage or a different route.

#### H12 — Usage, reports, export and retention

**Owns:** HC-07/08, HD-07/09 and conditional HD-13. Target: `src/usage.rs`, `src/usage/`,
`src/usage/diagnostics.rs`, reporting/retention jobs, schema,
`tests/usage_reports.rs` and `tests/diagnostics.rs`.
Customer notices additionally target `src/notifications.rs`, `src/notifications/`,
their schema/routes and `tests/notifications.rs`, reusing H03 outbox/jobs.

- **H12.01 — Evidence separation:** minimal logical/settlement anchor, expiring request
  detail, physical attempts, provider expense, audit and money journal have explicit
  readers/retention. Expiring detail cannot delete a financial reference.
- **H12.02 — Metrics:** implement the canonical outcome/token/charge/latency definitions.
  Specify SQL/filter/group/bucket population, UTC boundaries, coverage watermark and
  empty/unknown/stale behavior; retries do not multiply logical request totals.
- **H12.03 — Projections:** exact daily dimensions, idempotent rebuild/checkpoints,
  late settlement/request-linked credit correction and aggregation version. Payment
  refunds remain wallet activity, not negative model usage. Reconcile charges to
  journal; show incomplete expense coverage instead of confirmed invented profit.
- **H12.04 — API/export:** stable scoped cursor, bounded range/rows/bytes/runtime,
  safe detail, formula-safe CSV and authorized download. Separate current wallet/
  active/queued snapshot from historical report filters and expired detail.
- **H12.05 — Cleanup:** exact deletion/anonymization/tombstone operations, eligibility
  exclusions for open reconciliation and financial retention, job retry/cutoff and
  backup expiry. No production purge before the reviewed financial policy exists.
- **H12.06 — Abuse/support:** sanitized bounded User-Agent, no raw IP/body/header
  archive, safe correlation codes and audited privileged lookups. Low-cardinality
  telemetry cannot become a second uncontrolled customer-data store.
- **H12.07 — Diagnostic contract:** implement the canonical
  [errors and production diagnostics](HOSTED_RELAY_ARCHITECTURE.md#errors-and-production-diagnostics)
  DTO, code-to-message/severity/evidence/next-action mapping and separate customer/
  owner projections. Define each field's type, nullability, length, source and
  visibility, stable scoped filters/cursors, event deduplication and matching indexes.
  Reuse core `ErrorOrigin`/codes and domain-owned execution/accounting evidence;
  source attribution is not proof of cause. Specify `diagnostic_events` expiry,
  ingest/storage caps, coverage persistence and fallback when a sink is unavailable.
  No arbitrary raw error text, independent financial truth or implied safe replay.
- **H12.08 — Notices:** implement [SC-05](HOSTED_RELAY_ARCHITECTURE.md#customer-notifications-and-delivery):
  recipient-scoped in-app list/read state, event mapping, stable low-balance episode,
  preferences/consent, bounded fanout, current-recipient recheck, expiry and cleanup.
  Optional external delivery uses qualified HD-13 contracts and distinguishes sent,
  failed, unknown and suppressed outcomes; no exactly-once claim without evidence.
  Security/financial/lifecycle truth stays in its owning domain, never in notice state.

**Acceptance:** `tests/usage_reports.rs` proves one-snapshot card/chart/table/export
agreement, boundary dates, missing coverage, late credits, unknown expense, retained
aggregate with expired detail, changing authorization and restart during cleanup.
Report reads cannot call upstream models, change balances or rebuild the runtime.
`tests/diagnostics.rs` additionally covers unknown origin/code, malformed or
secret-bearing upstream evidence, foreign request/attempt IDs, duplicate events,
pre-auth scope, retained financial truth after detail expiry and lost telemetry.
Validate the exact customer DTO, not only fields hidden by the owner UI. Confirmed
cancellation, uncertain disconnect, recovered attempt, incomplete response and
completed execution with unsettled money have distinct fixtures and counters.
`tests/notifications.rs` proves duplicate/reordered events, lost delivery acknowledgement,
withdrawn consent, changed recipient, foreign IDs, threshold recovery, stale/superseded
notices and retention. A failed/suppressed notice cannot alter funds or public access.

#### H13 — Default web application and typed transport

**Owns:** HC-01/07/10, HD-08/10. Target: `web/{package.json,src/,tests/,README.md}`
and the Hosted schema/client compatibility checks. No extraction to another repo.

- **H13.01 — Build:** independent React/TypeScript/Vite application, manifest,
  scripts, tests and static artifact. No Tauri imports, second money/auth backend,
  runtime Node requirement or obligation to build web when building the backend.
- **H13.02 — API transport:** schema-backed DTOs, same-origin paths, sessions/CSRF,
  cancellation, request correlation and errors; check API version/capabilities before
  authenticated navigation. Never fall back to an unconfigured origin.
- **H13.03 — Routes:** `/`, `/login`, `/app/overview`, `/app/usage`, `/app/models`,
  `/app/keys`, `/app/billing`, `/app/settings`, `/app/help` and separate `/admin` entry.
  Keep these names aligned with the canonical [navigation table](HOSTED_RELAY_ARCHITECTURE.md#navigation-and-screen-responsibilities).
  Define protected deep links, auth callback handoff and unknown-route behavior.
  Use the separate [owner browser routes](HOSTED_RELAY_ARCHITECTURE.md#owner-browser-navigation),
  including passkey-only `/admin/login`; do not treat browser `/admin/access` as a
  new unprefixed control API or exchange customer sessions for owner authority.
- **H13.04 — Configuration:** useful neutral branding, typed logo/theme/copy/legal
  links, RU/EN and light/dark. Domain and branding changes use configuration, not
  a private frontend fork or hardcoded deployment endpoint.
  Follow the agreed [Relay web design](HOSTED_RELAY_WEB_DESIGN.md#visual-system),
  using Relay's own launcher palette, navigation and usage/detail composition;
  replace account/pool identity with the customer's masked API-key identity.
  Reuse browser-safe presentation patterns without importing Tauri or owner data.
  External reference projects contribute functional scenarios only; do not import
  their screens, CSS/themes, branded assets or page components. Keep suitable
  existing utilities and licensed primitives.
- **H13.05 — Browser safety:** no sensitive localStorage/IndexedDB/service-worker
  caching; scoped in-memory caches, request cancellation and late-response guards
  on sign-out/account switch. Only non-sensitive presentation preferences persist.
- **H13.06 — Accessibility:** keyboard/focus/errors, mobile navigation, reduced
  motion and chart tables; test labels and disclosure in both locales/themes.

**Acceptance:** `web/tests/api-contract.test.ts` and `web/tests/e2e/shell.spec.ts`
cover incompatible API, deep-link refresh, missing config, session expiry, delayed
foreign response, logout clearing and no requests to unconfigured origins. Synthetic
preview fixtures are test-only and visibly separated from real customer mode.
Review the original visual system at desktop/mobile sizes in both locales/themes,
including keyboard and exceptional states; preserve utility/dependency notices.
Use the design document's [handoff matrix](HOSTED_RELAY_WEB_DESIGN.md#design-handoff-and-acceptance)
for WS-01/02/11/24, shared components and both shells; do not ship anonymous prices
from a private `/models` call or invent public numbers when publication is absent.

#### H14 — Complete customer cabinet

**Owns:** HC-07/08/10. Target feature directories under `web/src/features/`:
`overview/`, `usage/`, `models/`, `keys/`, `billing/`, `settings/`, `help/`.
Implement [WS-03–WS-10](HOSTED_RELAY_WEB_DESIGN.md#customer-cabinet) with the shared
design/interaction system; screen descriptions cannot weaken domain policy.

- **H14.01 — Overview:** current available/reserved wallet and current activity
  independent of historical UTC filters; graphs and comparisons use canonical
  request/token/charge/latency meaning, coverage and unknown/stale presentation.
- **H14.02 — Usage:** model/key filters, time series, stable paginated history, safe
  details and bounded exports. Empty retained period, expired detail, partial usage,
  unsettled charge and unknown expense each have distinct text/state. Apply H12's
  safe explanation, stable code/ID, evidence-based next action and explicit support
  copy; never expose the owner timeline or promise a refund before journal posting.
  Keep authorized time/model/key context when moving between chart, table and export;
  historical filters never change the current wallet or live-count meaning.
- **H14.03 — Models/keys:** authorized catalog/prices/bands, connection examples,
  one-time copy/reveal, scope/limit form, expiry, revoke and replacement. A lost
  issuance response has a safe recovery flow, not silent extra key creation.
- **H14.04 — Billing:** top-up quote/confirmation, pending/credited/failed/expired
  status, receipt and refund request. A redirect is not proof of funds. Disclose
  reserved money, estimated settlement and compensating credits without double totals.
- **H14.05 — Settings/help:** linked sign-in methods, sessions, recovery, suspension,
  deletion request and no-last-login guard; instructions for actual supported clients
  and safe support correlation IDs, not customer prompts/secrets.
- **H14.06 — State inventory:** for every page/action, enumerate loading, empty,
  partial, stale, denied, unavailable, success, duplicate and interrupted submission.
  Every retry control must distinguish a safe read from a new chargeable operation.
  Add shell/feature error boundaries and API/network/version fallbacks following
  the [browser failure contract](HOSTED_RELAY_ARCHITECTURE.md#browser-failures-and-unavailable-services).
  Keep unavailable balances distinct from zero; support copy has safe route/build/
  request references, not query/form/exception dumps or automatic telemetry upload.
- **H14.07 — Guided journey and notices:** implement [SC-02](HOSTED_RELAY_ARCHITECTURE.md#first-customer-request-walkthrough)
  with first-use Help/Overview, safe configuration examples, explicit paid-test
  distinction and lost-key-issue recovery. Show own notices/preferences, current
  service condition and scheduled changes inside existing routes. Explain available,
  inference-reserved, refund-reserved, charged and pending values without false totals.

**Acceptance:** `web/tests/e2e/customer-journey.spec.ts` and
`web/tests/e2e/customer-failures.spec.ts` cover the full synthetic funded journey,
duplicate clicks/callbacks, disconnected response, revoked session/key, capacity
failure, money/graph/export reconciliation, mobile/keyboard and both locales/themes.
Cover render failure, unavailable backend, deliberate navigation abort and lost
mutation acknowledgement. Read refresh/error-boundary recovery cannot repeat a
payment, key issue or inference; copied summaries contain no secrets/customer content.
Browser chat, image-generation UI, teams or saved conversations are not inferred additions.
Extend customer-journey/failure tests with notice opt-out, stale public status,
maintenance/closure access, qualification restrictions and the exact SC-03 examples.

#### H15 — Operator surfaces, Desktop parity and explicit transfer

**Owns:** HC-03/04/07/08/09/10, HD-12. Target: `web/src/features/admin/`, Hosted
management handlers, affected typed Desktop wrappers and transfer tests.
Implement the [owner screen passports](HOSTED_RELAY_WEB_DESIGN.md#owner-panel)
WS-11–WS-23 with H13's shared design system and distinct owner authority; preserve
the explicit Desktop-transfer flow instead of treating web sign-in as synchronization.

- **H15.01 — Action matrix:** map every owner control to method/DTO/permission,
  freshness requirement, expected revision, confirmation, audit and failure state.
  No generic edit-all form or hidden frontend-only authorization.
- **H15.02 — Inventory/pool:** overview, connections/accounts/API sources, parked
  inventory, membership/rotation, models/prices, quota/context/tool policies,
  refresh/reauthentication and lifecycle actions preserve local semantics.
- **H15.03 — Access/usage:** direct API information, keys/limits and activity with
  separate logical/physical counts; authenticated customer/key attribution plus
  untrusted bounded User-Agent. No local Applications/launcher page in Hosted.
- **H15.04 — Customers/money/settings:** suspension, safe diagnostics, audited
  adjustments/refunds and integration/security/operations settings. Display expense
  uncertainty honestly; administrative screens never expose normal prompt bodies.
- **H15.05 — Transfer:** inspect/snapshot/confirm/stage-and-drain/verify sequence,
  revision conflict, source-preservation guarantee and explicit secret export/import.
  Interrupted transfer is recoverable and cannot become hidden bidirectional sync.
- **H15.06 — Compatibility:** paired Desktop actions use the same owner policy,
  capabilities/revisions and fresh-proof checks; preserve Desktop local profiles.
- **H15.07 — Errors and investigation:** build `/admin/usage?tab=errors` and
  `/admin/usage/requests/{request_id}` from the canonical diagnostic contract.
  Use H13's original design to connect availability/concurrency, error trends and
  authorized request detail; no copied reference dashboards or raw payload views.
  Deliver scoped filters, separate logical/attempt/cancellation/accounting counters,
  observed timeline, evidence completeness, current financial projection, safe
  next action and support copy. Keep recovered attempts nested and unknown causes
  explicit. `/admin/operations` separates real job/component incidents from inference
  failures and exposes H16's bounded capture controls; no generic replay/debug console.
- **H15.08 — Setup and service lifecycle:** add `/admin/operations/setup`, guarded
  mode transitions, qualified check actions and staged change/closure confirmations.
  Show missing/failed/expired readiness with the next safe action, affected-customer
  counts and real observed revision. The UI cannot waive mandatory evidence, run a
  paid test on page load or mark an unknown refund completed. Use SC-01/06/07/08.

**Acceptance:** `tests/management_parity.rs`, `tests/transfer_recovery.rs` and
`web/tests/e2e/admin.spec.ts` cover every advertised action, unauthorized customer,
stale revision, expired fresh proof, late refresh after removal, interrupted transfer,
source preservation and accurate active counts. Screens cannot report unsupported
mutations as successful, and polling cannot cause inference/discovery side effects.
`web/tests/e2e/diagnostics.spec.ts` exercises owner/customer isolation, RU/EN wording,
logical-versus-attempt counts, expired/partial detail, completed-but-unsettled work,
mobile/keyboard access, safe support copy and bounded filters. Opening, refreshing
or acknowledging a problem cannot call upstream or alter money/access. Capture UI
shows scope, expiry, consumed budget and revoked/stopped state from the server.
`web/tests/e2e/service-lifecycle.spec.ts` covers stale checklist activation, mode races,
clear paid-check confirmation, change notices, missing outage contact and closure
with outstanding funds. Customer sessions cannot access owner setup/control APIs.

#### H16 — Deployment, operations and recoverable release

**Owns:** HC-08/09/11, HD-05/07/09/10/11. Target: `relay-hosted/deploy/`,
`relay-hosted/README.md`, `src/management/diagnostics.rs`, diagnostic-session schema,
web distribution guide and operations acceptance fixtures.

- **H16.01 — Packaging:** pinned backend/web/DB/edge/monitoring artifacts, compatibility
  manifest and both default/custom site recipes. Runtime works without default assets;
  no `latest`, Git submodule or production frontend compilation requirement.
- **H16.02 — Host/edge:** minimum measured resources, file ownership, private DB/backend/
  metrics, restricted non-root containers, trusted proxy/origin configuration, secret
  files, DNS/TLS and API-before-SPA routing. Test SSE/WS and cancellations through Caddy.
- **H16.03 — Visibility:** separate liveness/readiness/inference probes, bounded safe
  metrics/logs, external outage check and actionable alert/runbook pairs. Define
  settlement lag/oldest hold, disk/DB, stale jobs, refresh and missing archive alerts.
  For each alert fix observation window, threshold, severity, grouping, recovery,
  cooldown, notification destination and runbook; acknowledgement is not resolution.
  Distinguish cancellation/recovered-attempt noise from sustained degradation, and
  unavailable telemetry from a measured zero-error period. Keep build/source-map
  artifacts private; browser-only failures are not invented backend observations.
- **H16.04 — Backup:** pgBackRest/WAL delivery plus compatible vault/config manifest,
  independently recoverable keys and off-host storage; monitor completion, not job
  start. Payload TTL and deletion/revocation policy apply to restored material.
- **H16.05 — Restore:** isolated host with ingress/inference/payment effects closed;
  select compatible restore set, verify integrity/references, reconstruct/reconcile
  wallet state, replay revocations/deletions, resolve uncertain requests and expired
  holds, then controlled acceptance before reopening.
- **H16.06 — Upgrade:** preflight versions/space/backup, stop admission, drain/flush,
  one migration runner, startup/readiness, smoke acceptance and explicit roll-forward
  or compatible rollback. Old binary plus newer schema is not a rollback plan.
- **H16.07 — Runbooks:** lost admin/vault key, identity outage, expired upstream
  account, quota exhaustion, stuck stream/hold, webhook outage, disk full, corrupt
  backup and migration failure. Specify safe diagnostic command, expected output,
  stop condition and recovery evidence; never ask for customer prompts/secrets.
- **H16.08 — Temporary diagnostics:** implement the canonical owner-only
  diagnostic-session list/start/stop API, validated configuration and audited state
  machine using existing H03/H05 primitives. Require fresh passkey/CSRF, bounded
  server-resolved scope and a durable idempotent start. Qualify finite event/byte/
  rate/storage limits and record approved deployment values; missing capture caps
  disable capture. Enforce original expiry, one active session, fenced budget
  reservation, revocation, stop, cleanup and restart without resetting counters.
  Verify normal error reporting remains usable without extended capture; a full
  optional sink cannot lose required settlement/audit evidence or cause inference.
- **H16.09 — Readiness and boundary qualification:** build version-bound readiness
  checks/results, dependency invalidation and explicit activation gates. Freeze the
  mode-specific guard matrix: test prerequisites do not require their own completed
  test, and entering maintenance cannot depend on healthy capacity. Freeze the
  measured abuse/notification/check budgets and purpose-scoped outbound policy.
  `tests/readiness.rs` and `tests/outbound_policy.rs` cover expired evidence, changed
  artifacts/credentials/origins, destination/redirect changes and private-provider
  exceptions without generic fetch authority. Verify independent outage contact.
- **H16.10 — Lifecycle drills:** expand SC-07/08 into executable runbooks with actor,
  exact command/action, safe output, deadline, stop rule and recovery evidence.
  `tests/service_modes.rs` and `tests/service_closure.rs` cover paused admission,
  active-request drain, late callbacks, pending refunds, history/export access,
  closure and single-active-authority host migration. Never delete required keys/
  financial evidence just because the public website has been disabled.

**Acceptance:** `tests/operations_recovery.rs` plus isolated deploy/restore drills
verify private ingress, resource bounds, matched artifact versions, full-host loss,
wrong/missing keys, expired/revoked state and webhook/request reconciliation. Measure
RPO/RTO and workload; do not convert the documented objectives into claimed results.
`tests/diagnostic_capture.rs` uses a controlled clock and real PostgreSQL to test
concurrent start/append/stop, lost commit acknowledgement, stale owner proof, forged
scope, expired/revoked session, process restart and event/byte/rate exhaustion.
Operations fixtures also exercise log/storage failure, repeated alert grouping and
recovery without recursive logging storms. Assert synthetic canary secrets and
payload markers are absent from API, logs and support output; UI masking is not enough.
No authorization to deploy, restart a user's service or run paid probes is implied.

#### H17 — End-to-end paid-product acceptance

**Owns:** HC-12 and every package's evidence. This is the public-release gate, not
another implementation shortcut. Real integrations require explicit authorization.

- **H17.01 — Customer:** fresh and returning users complete each supported login
  path, one identity/wallet, real supported funding, key, enabled model, correct
  charge, receipt/report and recovery. Unknown data is never fabricated success.
- **H17.02 — Owner:** establish/recover owner, configure integrations/capacity/prices,
  operate every promised local-pool control, suspend/revoke and investigate safely.
  Trace a synthetic failure from safe customer reference to owner evidence and
  accounting status; prove unknown cause, wrong-workspace lookup and expired detail
  cannot become invented certainty or leaked content.
- **H17.03 — Failure matrix:** run concurrency, duplicate payment/request, lost
  acknowledgement, cancellation, quota/provider outage, restart and restore with
  expected balances, outcome certainty and absence of unauthorized execution.
  Include every [reference-derived required case](HOSTED_RELAY_ARCHITECTURE.md#selected-patterns-and-owning-packages):
  stale/missing-cache admission, delayed settlement, quota/capacity separation,
  cancelled/obsolete ownership, chart reconciliation and forbidden-content handling.
  Attach Relay test evidence, not successful runs from the source projects.
- **H17.04 — Distribution:** one empty-host default installation and one supported
  custom-site integration; no hidden external dependency, secret/code coupling
  or second wallet. Qualify published artifact/client compatibility pairs.
  Trace WS-01–WS-24 to the [design evidence matrix](HOSTED_RELAY_WEB_DESIGN.md#screen-to-package-and-evidence-matrix),
  including RU/EN, light/dark, responsive/focus and exceptional states. Textual design
  approval and token contrast checks do not replace rendered UI/accessibility tests.
- **H17.05 — Commercial:** verify HD-01/02/09 decisions, actual tariffs, permitted
  regions/source classes, merchant/receipt/refund configuration and customer notice.
  Working personal credentials or synthetic payments do not satisfy this gate.
- **H17.06 — Operations:** demonstrate backup/restore, measured capacity/recovery,
  alerts, operator response and rollback consequences. Record safe evidence in test/
  release artifacts; do not turn this roadmap into a progress transcript.
  Demonstrate alert/recovery delivery, sink failure and capture auto-stop in the
  production build; no development-mode payload logging is a launch prerequisite.
- **H17.07 — Scenario sign-off:** trace every SC-01–SC-08 step from UI/API through
  authority, durable records and external effects to reports/recovery. Attach named
  test evidence and unresolved HD blockers. Verify synthetic arithmetic separately
  from authorized real merchant/provider tests. A passed mock or written runbook is
  never evidence of a live integration, safe restore or legal/commercial approval.

**Exit:** all first-release obligations have verified evidence or the public scope
is explicitly revised by the product owner. No paid route is activated by default
merely because a container, login page or model listing works.

#### Cross-package failure and acceptance coverage

| Scenario | Primary packages | Required result |
| --- | --- | --- |
| Two callbacks register one subject | H04, H06 | One identity/workspace/wallet; no orphan credit account |
| Customer supplies a foreign resource ID | H02, H06, H09, H12, H14 | No cross-workspace data, charge, grant or existence disclosure |
| Two keys spend the same remaining balance | H04, H09 | Guarded reservation, no negative wallet or multiplied limits |
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
This is a minimum review matrix, not a claim that unknown failures cannot occur.

#### Scenario evidence checklist

For each scenario, the owning package attaches its exact step/test IDs and expected
customer/owner text to the canonical walkthrough. These are planned acceptance
artifacts, not files claimed to exist in the current repository.

| Scenario | Principal acceptance artifacts | What must be proved |
| --- | --- | --- |
| SC-01 | `tests/bootstrap.rs`, `tests/readiness.rs`, `web/tests/e2e/service-lifecycle.spec.ts` | Default closed, recoverable owner setup, current evidence and explicit public activation |
| SC-02 | `tests/customer_identity.rs`, `tests/admission_postgres.rs`, `web/tests/e2e/customer-journey.spec.ts` | One verified identity/wallet/key journey, honest paid request and safe lost-response recovery |
| SC-03 | `tests/accounting_postgres.rs`, `tests/payments_postgres.rs`, `tests/execution_recovery.rs` | Exact worked amounts, balanced journal and once-only reserve/charge/refund/correction |
| SC-04 | `tests/diagnostics.rs`, `tests/diagnostic_capture.rs`, `web/tests/e2e/diagnostics.spec.ts` | Known versus unknown cause, safe investigation and bounded capture without replay |
| SC-05 | `tests/notifications.rs`, `web/tests/e2e/customer-failures.spec.ts` | Recipient isolation, consent, deduplication, expiry and no financial side effects |
| SC-06 | `tests/pool_catalog.rs`, `tests/admission_postgres.rs`, `web/tests/e2e/service-lifecycle.spec.ts` | Notice/effective-time behavior, saved tariffs and emergency-disable distinction |
| SC-07 | `tests/service_modes.rs`, `tests/operations_recovery.rs`, isolated restore drill | Admission fence, surviving obligations, trustworthy recovery and independent outage signal |
| SC-08 | `tests/service_closure.rs`, `tests/payments_postgres.rs`, isolated migration drill | Single active authority, funds/exports/late events and separately verified teardown |

## Local runtime P0 — finish and accept the current runtime

### Model, provider, and harness compatibility

The shared typed pipeline, four native paths, multiple source/model bindings,
and native-upstream selection are already implementation contracts. Remaining
work is qualification and specific missing semantics, not another adapter engine.

| Work | Acceptance |
| --- | --- |
| Catalog provenance and refresh | Source failures preserve valid data; changed identity/price/capability is traceable; generated exports update atomically without private inventory or manual model rows. |
| Endpoint feature evidence | Advertised protocol does not grant unsupported continuation, tools, reasoning, cache, media, or usage semantics. Pins and explicit exclusions survive reload. |
| Conversation compatibility | Each advertised source/client/version passes text → call → result → next answer in JSON and SSE, including its supported reasoning/media/schema features. |
| Client startup context | Establish a lossless foreign representation for standalone Responses results without `call_id` and interleaved system/developer instructions, including Messages-to-Gemini history, before claiming full client parity; do not invent calls, erase context, or hoist instructions. |
| Native fidelity | Ordered schemas, precise numbers, repeated permitted headers, opaque state, and native extensions survive without lossy conversion. |
| Accounting and errors | One logical outcome plus retained attempts; partial/unknown usage remains explicit; the failing stage and origin are safe and understandable. |
| Client profiles | ChatGPT, Claude Desktop, and OpenCode expose only usable models/efforts/context for that route; attachment and recovery preserve user settings. |

Use the [official contracts](../../.agents/skills/relay-protocol-adapters/references/contracts.md)
and [conversation matrix](../../.agents/skills/relay-protocol-adapters/references/acceptance.md).
Synthetic coverage, a working endpoint, and live client acceptance are different
evidence. Discovery must not generate paid probes across a catalog.

### Client and provider acceptance

BPS multi-tool batching has a bounded Relay-local contract in PLANNING and
synthetic roundtrip coverage for JSON/SSE, discovery, tool restrictions,
disabled parallel policy, typed results, and opaque reasoning. Remaining live
qualification must verify model adoption of the envelope, client acceptance of
reversible call IDs and result grouping, and the complete tool-results/next-turn
cycle on each intended harness. Measure cache reuse, TTFT and end-to-end latency
separately. Local protocol support is not evidence of concurrent client execution
or a measured speedup, and a first response with several calls is insufficient.

Request-stage diagnostics distinguish selected-attempt preparation, upstream
headers/collection, normalized first stream output, and response/tool readiness.
Qualify their coverage on the intended HTTP/SSE/discovery paths after installation;
client receipt and between-request harness delays require client-side evidence.
Cold-preparation qualification should compare request building, its initial
schema-validation subset, catalog preparation, and attachment work. A synthetic
validator benchmark does not identify a live cold-start bottleneck by itself.
Use context-size and existing cache-prefix diagnostics before removing any work;
token reuse alone does not establish the cause of TTFT or safe history reduction.

| Area | Cases still requiring permitted live qualification |
| --- | --- |
| Provider presets | Exact endpoint/region/key scope, Moonshot USD/CNY statistics, Kimi Code discovery/tools; quota readers only after their units/reset contract is verified. |
| Lifecycle | Add, refresh, disable, trash/restore, removal, credential replacement, failed save/rollback, and restart on Desktop and server. |
| Outcomes | Completed, incomplete, refusal, malformed terminal, interruption, late usage, cancellation, and safe pre-output retries. Check state, not only displayed text. |
| Tools/state | Parallel calls, fragmented arguments, namespace/custom identity, Gemini optional IDs, signed reasoning, full-history and ID-based continuations, missing state after restart. |
| Native extensions | Gemini continuation-token resumes with unchanged contents and owner; provider cache references/hosted tools; client-executed Responses discovery and PDF inputs/results on supported pairs. |
| Authentication | OAuth renewal, same-token reauthentication, delayed 401/invalid grant, routing-cookie expiry; old turn/WS state must not cross authorization incarnations. |
| Transport | Compressed input, compaction, HTTP SSE versus reusable WS, socket loss, and unchanged owner/budget rules. Measure first output and usage without claiming quality from transport choice. |
| Limits | `turn_rate_limit_exceeded` keeps its delay without account-wide rotation; provider quota/access/model blocks keep their own scope. |
| Inventory | Endpoint failure does not erase source inventory or grant a different route. |

### Excel / Basis Points acceptance

- PKCE callback, cancellation, proxy, contextual Help, separate native/BPS
  connections, import/export/transfer, and concurrent login replacement.
- Compact versus full versus local-discovery catalogs with matched real client
  tasks: cold-start input, total lookup+answer usage, tool-selection accuracy,
  latency, context floor added by the native application, and session caching.
  Synthetic byte reduction is not a token/cost or quality guarantee.
  Include explicit retries after a completed lookup and failed answer or exhausted
  dispatch budget: verify history-scoped preloading in JSON/SSE, one usage event
  per physical attempt at budget exhaustion, and total sends/input, not only
  cache hits.
- Repeated and overlapping catalog lookups: verify earlier-result references,
  unchanged schema validation, and total input across all generations. Concurrent
  identical images should share uploads within the existing account/credential
  scope; include failure, cancellation and cache-capacity cases.
- Warm discovery-to-tool-less continuations, including client compaction through
  ordinary Responses: measure observed cached input and end-to-end latency
  against the cold path at matched model/effort/account. Check restart/expiry,
  changed instructions/context, zero current tool authority, and retained
  history. Stable prefix tests alone do not establish a live performance gain.
- Actual entitlements, effort settings, full tool cycles, inline/file-ID tool
  images, collaboration attribution, and no privilege increase from history.
- Early text, terminal-only tool release, malformed/missing completion,
  heartbeat timeout, cancellation, and first/last output timing.
- Explicit local structured-output validation, refusals, tool turns, invalid
  schemas/output, and no automatic regeneration of a completed answer.
- Next-request 401 recovery, model-scoped access blocks, shared aliases,
  two-account rotation, quota refresh, removal, and operator disable/drain.
- Native Free versus Free BPS behavior after import, downgrade, restart, and
  transfer; timezone rewriting leaves ordinary text/API-source bodies intact.
- Quality comparisons only with authorized isolated routes and matched tasks,
  models, and effort. Keep inconclusive observations separate. Local solvers
  do not establish live account quality or eliminate degradation.

### Concurrency and load

- Verify HTTP/2 refusal behavior against the installed executable: no hidden
  reqwest resend inside a recorded dispatch; preserve Relay's retry eligibility.
- Finish native Responses WS multiplexing: independent lanes/FIFO, bounded
  queues, scoped errors, continuation forks, retry/usage and disconnect ownership.
- Qualify mixed HTTP/SSE/WS/image admission on an authorized live pool. Local
  fixtures cover repeated mixed waves, lifecycle churn, bounded queue retention,
  and scheduler lock/reservation timings; they do not establish process-memory
  stability, provider latency, or long-running live fairness.
- Complete delayed-result, token-ownership, concurrent-refresh, rollback, and
  deleted/re-added account cases on Desktop and server.

### Server acceptance

1. HTTPS pairing with separate management/request credentials and vault key.
2. Explicit permitted connection transfer and redacted management state.
3. Request/quota/usage/timing parity with Desktop open and closed.
4. Restart, upgrade, interrupted migration, backup/restore, and a request from
   the restored runtime.
5. No secrets or prompt/response bodies in diagnostics/support exports;
   credential-bearing account exports use `no-store`.

## Local server P1 — make server publication explicit

Finish one desired-state workflow: preview a revisioned diff, validate routes
and references, apply/rollback atomically, report the active revision, and
recover after a lost management connection. Portable settings, credentials,
local leases, and server-owned changes remain separate. Credential transfer
needs explicit confirmation; no implicit two-way synchronization.

## P1 — measured routing and performance work

- Measure startup, page reads, saves, pool switches, history sizes, first-output
  latency, refresh work, and lock waits. Optimize the measured path, not a
  guessed protocol or assumed cache-hit benefit.
- Reduce the many mutable attempt flags passed through
  `gateway/execution/request/drive.rs` into cohesive typed execution state.
  Preserve one request identity, budget, lease, and recorded outcome across
  HTTP, account-only, and WS paths; add parity cases before extracting helpers.
- Group unrelated `GatewayRuntime` model/session/provider state into internal
  owners with explicit lock boundaries. Do not duplicate continuation stores
  or replace locks with a new actor/cache layer without measurement.
- Exercise policy-only updates without losing listeners, active streams,
  leases, affinity, or authorization provenance. Measure restart persistence
  needs for hard deadlines separately from disposable health hints.

## Maintenance

- Upgrade Tauri/GTK as a tested platform change, not incidental cleanup.
- Keep profile/history recovery, Windows paths, partial cleanup, transactional
  bulk mutations, and concurrent policy edits covered by their owning checks.
- Keep current contracts, future work, user Help, and release history in their
  existing documents. Do not append investigation transcripts or completed
  implementation steps to this roadmap.

## Future connectors

- Qualify supported SIWC plan/quota telemetry before adding a quota reader;
  opaque token metadata and native Codex endpoints are not a compatible API.
- SIWC local acceptance: authorize a real grant with user consent, verify catalog,
  text → tool call → result → next answer, refresh/restart/revocation and denied
  plan permission. Qualify the Tauri WebView flow on supported platforms, including
  MFA/social-login restrictions, proxy, cancel/reopen and copy-link browser recovery;
  the provider documents system-browser sign-in, not embedded compatibility.
  Local tests do not establish live provider acceptance. Native
  upstream WebSockets, compaction, Hosted use and token transfer without a fresh
  sign-in remain unqualified. Test Zenith export/import and reauthorization on a
  second host separately; do not infer live support from local round-trip tests.

Add subscription providers individually, with permission and evidence for
authentication, renewal/revocation, protected storage, entitlements, quota units,
usage, models, execution, and recovery. A generic API codec is not a subscription
connector. Kimi Code subscription and MiniMax M Plan readers remain unqualified
until their exact cloud contracts are verified.

## Demand-gated future work

- Distributed scheduling only after measured demand and shared admission,
  member leases, durable ownership, and idempotent delivery exist.
- Additional aliases/groups/profiles need scoped identities, collision checks,
  independent ordering, enablement, and price fields; presentation cannot alter
  protocol or scheduler policy.
