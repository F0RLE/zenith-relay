# Hosted Relay roadmap

Planned Hosted implementation and acceptance. The
[Hosted architecture](HOSTED_RELAY_ARCHITECTURE.md) owns target contracts;
the [Desktop roadmap](../project/ROADMAP.md) owns current-runtime work. Development and
release checks are in [CONTRIBUTING.md](../../CONTRIBUTING.md).

## Hosted multi-user Relay

The first public Hosted release is a complete prepaid model-token service backed
by operator-managed account/API-provider capacity. It has
owner-configured Google/Telegram/OIDC customer login, one identity/wallet across
channels, scoped keys, direct inference and local-pool control parity. Desktop is
an operator client; the Hosted panel has no local Applications section.
The [Hosted target contract](HOSTED_RELAY_ARCHITECTURE.md)
owns product decisions, launch defaults and safety gates.

Use its [recommended first-production stack](HOSTED_RELAY_ARCHITECTURE.md#recommended-first-production-stack)
and [empty-host acceptance sequence](HOSTED_RELAY_ARCHITECTURE.md#from-an-empty-host-to-paid-service)
as the baseline. Dependencies and deployment artifacts are part of the planned
implementation. Use the shared core for the new Hosted runtime.

### Delivery stages

Use the [package dependency map](#package-dependency-map) as the single delivery
sequence: H00 specification, H01 core qualification, H02–H12 runtime and accounting,
H13–H15 interfaces, H16 operations, H17 paid acceptance. Each package below owns
its artifacts, failure cases and checks. Complete affected
[specification gates](HOSTED_RELAY_ARCHITECTURE.md#specification-before-implementation)
before code; package prerequisites define ordering.

Internal stages do not reduce the first-public-release scope. One qualified
account family is enough initially; additional Claude/other account connectors
are future work, not promised support inferred from an API adapter.

Excel/BPS deep acceptance and additional subscription connectors are not a
prerequisite for a Hosted release that does not offer those credential classes.
No shared paid route is inferred from a working personal account.

### Customer website implementation slices

H13 owns the web shell and typed transport, H14 the customer cabinet, and H17
end-to-end acceptance; use their [package definitions](#hosted-implementation-work-packages). The
[website contract](HOSTED_RELAY_ARCHITECTURE.md#customer-website) owns routes and
data meanings; [web design](HOSTED_RELAY_WEB_DESIGN.md) owns presentation.

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
→ exact charge → report → failure/recovery. Public release requires full acceptance.

#### H00 — Specification and decision coverage

**Owns:** HC-01–HC-12. Keep contracts in architecture and presentation in web design.

- **H00.01 — Scope:** confirm journeys, exclusions and default/custom frontend scope.
- **H00.02 — Decisions:** assign owners and evidence to unresolved HD-01–HD-13.
- **H00.03 — Dictionaries:** complete affected fields, APIs, states and permissions in their existing owners.
- **H00.04 — Threat/failure coverage:** map authorization, race, outage and recovery invariants to tests.
- **H00.05 — Change control:** update affected contracts, consumers and tests together; retain reuse provenance/licenses.
- **H00.06 — Scenarios:** check SC-01–SC-08 against those contracts; name blockers instead of inventing facts.

**Acceptance:** consistent links, identifiers, dependencies and expected checks.
Design review does not establish runtime or commercial qualification.

#### H01 — Shared-core qualification and reuse boundary

**Owns:** HC-01/04/06, HD-12. Existing anchors are `GatewayRuntime`,
`gateway::router`, `gateway::network_router`, core adapters/usage and Desktop
vault/refresh/configuration flows. Do not rewrite the four codecs.

- **H01.01 — Integration:** identify core entry points, cancellation/lease/refresh ownership and restart boundaries.
- **H01.02 — Auth context:** carry server-created tenant/key/request identity, not caller-supplied attribution.
- **H01.03 — Evidence:** capture terminal, usage and attempt certainty before conversion for each offered protocol.
- **H01.04 — Retry:** verify pre-execution proof, ownership and output visibility; adapt connector failure tests.
- **H01.05 — Reuse:** make the smallest shared interface/vault change and check Desktop/server callers.
- **H01.06 — Parity:** resolve or exclude unsupported mutations, including weekly quota wake.

**Acceptance:** existing core tests and a Hosted-context fixture prove isolation
without Desktop regressions. Qualify account connectors separately from codecs.

#### H02 — Hosted composition, database and contract foundations

**Owns:** HC-01/02/07/11, HD-04/08/10. Target: `relay-hosted/{Cargo.toml,src/,
migrations/,openapi.yaml,tests/support/}`; existing builds remain independent.

- **H02.01 — Process:** startup validation, graceful admission stop and bounded shutdown; no automatic legacy migration.
- **H02.02 — Config:** validate typed settings, origins, secrets and finite limits before admission.
- **H02.03 — Schema:** tenant keys, operation uniqueness and append-only migrations scoped to each package.
- **H02.04 — DB:** bounded SQLx pool, short transactions, separate runtime/migrator roles and safe retries.
- **H02.05 — API:** typed routes/errors, correlation, pagination and compatibility; domain packages own endpoint schemas.
- **H02.06 — Harness:** disposable PostgreSQL and deterministic clock/provider/payment fixtures.
- **H02.07 — Service control:** persist mode/admission epoch; boot in closed `setup`, fence transitions and qualification scope.

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

**Acceptance:** `tests/accounting_postgres.rs` proves no concurrent overdraft,
balanced/replayable postings, idempotent close/refund and commit-ack recovery.
Unknown differs from zero, overflow rejects, and refund reserves remain unavailable
to inference until a proven release commits.

#### H05 — Owner provisioning, passkeys and Desktop authority

**Owns:** HC-03/10, HD-03/08/12. Target: `src/auth.rs`, `src/auth/`,
owner/device schema and `tests/admin_auth.rs`.

- **H05.01 — First owner:** server-local, installation-bound, single-use enrollment; atomic credential/owner assignment.
- **H05.02 — WebAuthn:** verified library-backed RP/origin, discoverable-credential and user-verification contracts.
- **H05.03 — Sessions:** protected cookies, CSRF, expiry/authorization epoch and fresh-proof action matrix.
- **H05.04 — Recovery:** second/last-key guards and audited server-local recovery; no weaker public reset.
- **H05.05 — Desktop:** bounded owner-approved pairing/scopes/revocation and browser step-up, not an admin bypass.

**Acceptance:** `tests/admin_auth.rs` rejects wrong origin/RP, replay, foreign/stale
authority, CSRF/fixation and setup races. Qualify real authenticators separately.

#### H06 — Customer registration, linking and recovery

**Owns:** HC-03, HD-03/08 and conditional HD-13. Target: `src/identity.rs`, `src/identity/`, customer
auth handlers and `tests/customer_identity.rs`; use H04 wallet initialization.

- **H06.01 — Integrations:** verified Google/Telegram/OIDC adapters with issuer, redirect, state/nonce/PKCE validation.
- **H06.02 — Registration:** atomically create one person/workspace/wallet per verified subject.
- **H06.03 — Linking:** prove both identities; prevent reassignment and removal of the last login method.
- **H06.04 — Sessions:** revocable cookies, CSRF and outage/recovery flows; no owner/device authority.
- **H06.05 — Suspension/deletion:** fence all entry points, retain required money evidence and restore tombstones.
- **H06.06 — Abuse/recipients:** bounded attempts without enumeration/lockout; enforce service mode and qualification scope.
  External recipients need separate consent, proof and revocation under HD-13.

**Acceptance:** `tests/customer_identity.rs` covers all adapters, callback/link races,
last-login protection, redirects/CSRF, suspension, recipient changes and scope spoofing.
No second wallet, email-based merge or customer leakage. Authorize live login separately.

#### H07 — Credential inventory, pools, catalog and route grants

**Owns:** HC-04/09, HD-01/12. Target: `src/management.rs`, `src/management/`,
`src/catalog.rs`, `src/catalog/`, capacity migrations and `tests/pool_catalog.rs`.

- **H07.01 — Inventory:** scoped account/API sources, secret generations, refresh and health; saving is not pool admission.
- **H07.02 — Lifecycle:** separate add/reauthenticate/park/enable/disable/drain/trash/restore/remove; fence dispatch and ownership.
- **H07.03 — Parity:** reuse local validators for controls, proxies, automations and request/publication policies.
- **H07.04 — Catalog:** separate granted customer routes from bounded public descriptors/tariffs at `GET /public/models`.
  Preserve freshness/privacy; prices do not belong in `/public/config`, and aliases add no capacity.
- **H07.05 — Qualification:** require HD-01 and finite billable bounds for paid routes; reuse core discovery/validation.
- **H07.06 — Races:** revision-check refresh/removal, activation, tariffs and draining; reads cannot activate or probe providers.
- **H07.07 — Publication:** implement [SC-06](HOSTED_RELAY_ARCHITECTURE.md#price-and-model-change-walkthrough), preserving holds and fencing failed/superseded activation.

**Acceptance:** `tests/pool_catalog.rs` covers lifecycle/revision/grant isolation,
alias capacity, unknown quota and controlled-clock publication/notice races.
Qualify Desktop/server families together; disclose unsupported mutations.

#### H08 — Payments, receipts, refunds and external reconciliation

**Owns:** HC-05/09/12, HD-02/04/09. Target: `src/payments.rs`, `src/payments/`,
payment migrations, webhook routes and `tests/payments_postgres.rs`.

- **H08.01 — Adapter:** qualify the actual merchant contract, currencies, origins, signatures and idempotency; no guessed FX/provider.
- **H08.02 — Intent:** authorize and persist an immutable quote/operation before the external call; recover timeout/expiry.
- **H08.03 — Event:** verify signature, merchant and intent, then durably handle duplicates/reordering before acknowledgement.
- **H08.04 — Credit/receipt:** one posting per economic effect, not per event ID; redirects cannot credit money.
- **H08.05 — Refund/dispute:** implement the HD-02/04/09 states, refundable remainder, compensation and audit.
- **H08.06 — Reconcile:** resolve missed webhooks/acknowledgements by operation ID without resending uncertain effects.
- **H08.07 — Lifecycle:** reserve refund funds against concurrent spend; maintenance still processes existing payments/reconciliation.
  Notices follow committed effects and cannot alter money.

**Acceptance:** `tests/payments_postgres.rs` covers signatures, amount/currency,
duplicate/reordered credit, commit loss, concurrent refunds, spent-funds reversals,
late success and delivery outage. Live merchant/money acceptance remains separate.

#### H09 — Keys, limits, sizing and admission

**Owns:** HC-03/05/06/07, HD-06/07/08. Target: `src/access.rs`, `src/access/`,
admission parts of `src/inference/` and `tests/admission_postgres.rs`.

- **H09.01 — Keys:** scoped one-time issuance, verifier storage, expiry/rotation/revocation; no secret replay.
- **H09.02 — Policies:** enforce workspace/key precedence and units without multiplying budgets; recheck before dispatch.
- **H09.03 — Sizing:** snapshot finite post-expansion exposure and eligible tariff bands; reject unknown/unbounded requests.
  H11 must qualify persisted continuation expansion before resume is offered.
- **H09.04 — Idempotency:** authenticated scoped digests/default snapshots; conflict on changed requests, never redispatch uncertain/terminal duplicates.
- **H09.05 — Transaction:** atomically claim/check/reserve/persist; recover lost acknowledgement through the same operation.
- **H09.06 — Queue:** bound/cancel/expire work and atomically revalidate holds; no silent tariff increase beyond approved spend.
- **H09.07 — Fencing/fairness:** service mode, authorization epoch, due policy and budgets apply at reserve and dispatch through the shared scheduler.

**Acceptance:** `tests/admission_postgres.rs` covers concurrent keys, foreign/revoked
IDs, exact balance, unknown/zero price, overflow, deadlines, digest conflicts and
lost acknowledgements. HTTP/SSE/SDK duplicates create no extra hold or generation.

#### H10 — Execution, settlement and uncertainty

**Owns:** HC-05/06/09, HD-06/07. Target: `src/inference.rs`, `src/inference/`,
accounting settlement/reconciliation workers and `tests/execution_recovery.rs`.

- **H10.01 — Handoff:** persist fenced dispatch through H01 with authoritative identity; no SQL locks across provider I/O.
- **H10.02 — Observation:** retain pre-conversion terminal/usage evidence and safe protocol errors; integrate H11 checkpoints.
- **H10.03 — Streaming:** bound backpressure/duration and handle disconnect/cancel; HTTP success is not completion or retry proof.
- **H10.04 — Close:** settle once at the reserved tariff or enter deadline-bounded reconciliation under the specified estimate/waiver policy.
- **H10.05 — Late evidence:** uniquely credit lower customer amounts; higher expense stays with the operator, never reopen/resend.
- **H10.06 — Restart:** recover the same request/fence/deadline across send, stream, terminal, commit and acknowledgement cut points.

**Acceptance:** `tests/execution_recovery.rs` uses PostgreSQL and controlled upstream
to prove cut points, partial usage, cancellation, duplicate terminals and bounded
hold closure. Real client/provider qualification remains separate.

#### H11 — Continuations and durable completed turns

**Owns:** HC-06/09, HD-05/06. Target: `src/continuations.rs`,
`src/continuations/`, checkpoint schema and `tests/continuation_recovery.rs`.

- **H11.01 — Eligibility:** qualify native reference, full history or permitted encrypted payload per route; reject unsupported resume.
- **H11.02 — Scope:** bind lookup to tenant/key/provider/member/model/protocol/adapter/authority, expiry and turn version.
- **H11.03 — Checkpoint:** commit complete turns/tools before resumable success; use CAS/fenced advancement and branch ownership.
- **H11.04 — Payload:** opt-in encrypted/versioned storage with scoped size/TTL limits, not usage-screen or replay access.
- **H11.05 — Recovery:** restore only committed compatible authorized state; apply revocation, expiry and cleanup.

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
- **H12.03 — Projections:** version/rebuild aggregates and late credits idempotently; reconcile to journal, not guessed profit.
- **H12.04 — API/export:** bounded scoped cursors and formula-safe CSV; separate current wallet/activity from historical filters.
- **H12.05 — Cleanup:** reviewed retention, reconciliation exclusions, tombstones and backup expiry before production purge.
- **H12.06 — Support:** bounded sanitized labels/correlation and audited lookups, not raw IP/body/header archives.
- **H12.07 — Diagnostics:** implement the [canonical DTO/projections](HOSTED_RELAY_ARCHITECTURE.md#errors-and-production-diagnostics), deduplication, caps and sink-failure handling.
- **H12.08 — Notices:** implement [SC-05](HOSTED_RELAY_ARCHITECTURE.md#customer-notifications-and-delivery) with consent/recipient rechecks and qualified HD-13 external delivery.

**Acceptance:** `tests/{usage_reports,diagnostics,notifications}.rs` proves snapshot
reconciliation, late credits, retention, authorization and event/recipient races.
Distinguish cancellation, uncertainty, recovered attempts and unsettled completion.
Test customer DTOs for secret leakage, not just hidden UI fields. Reads and notice
delivery failures cannot trigger inference or change money/access.

#### H13 — Default web application and typed transport

**Owns:** HC-01/07/10, HD-08/10. Target: `web/{package.json,src/,tests/,README.md}`
and the Hosted schema/client compatibility checks. No extraction to another repo.

- **H13.01 — Build:** independent React/TypeScript/Vite static artifact; no Tauri, runtime Node or second money/auth backend.
- **H13.02 — Transport:** typed same-origin API, sessions/CSRF, cancellation, errors and compatibility checks; no fallback origin.
- **H13.03 — Routes:** implement canonical [customer](HOSTED_RELAY_ARCHITECTURE.md#navigation-and-screen-responsibilities)/[owner](HOSTED_RELAY_ARCHITECTURE.md#owner-browser-navigation) routes, deep links and callbacks with separate authority.
- **H13.04 — Configuration:** neutral branding, RU/EN and themes through [Relay design](HOSTED_RELAY_WEB_DESIGN.md#visual-system), not frontend forks or imported reference screens.
- **H13.05 — Browser safety:** scoped memory caches and late-response guards; persist only non-sensitive preferences.
- **H13.06 — Accessibility:** keyboard/focus, mobile, reduced motion and chart tables across locales/themes.

**Acceptance:** `web/tests/api-contract.test.ts` and `web/tests/e2e/shell.spec.ts`
cover incompatibility, missing config, deep links, session/logout races and origin
isolation. Review WS-01/02/11/24 and both shells against the design handoff;
synthetic fixtures and private catalog data never supply public prices.

#### H14 — Complete customer cabinet

**Owns:** HC-07/08/10. Target feature directories under `web/src/features/`:
`overview/`, `usage/`, `models/`, `keys/`, `billing/`, `settings/`, `help/`.
Implement [WS-03–WS-10](HOSTED_RELAY_WEB_DESIGN.md#customer-cabinet) with the shared
design/interaction system; screen descriptions cannot weaken domain policy.

- **H14.01 — Overview:** separate current wallet/activity from historical UTC metrics and coverage.
- **H14.02 — Usage:** scoped filters, pagination, safe detail/export and H12 explanations; preserve snapshot/filter context.
- **H14.03 — Models/keys:** granted catalog/bands, one-time issuance, limits, revoke/replace and lost-response recovery.
- **H14.04 — Billing:** quote, status, receipt/refund and distinct held/settled/estimated values; redirects are not credit.
- **H14.05 — Settings/help:** verified login/session recovery, last-method guard and safe supported-client instructions.
- **H14.06 — Failure states:** implement the [browser failure contract](HOSTED_RELAY_ARCHITECTURE.md#browser-failures-and-unavailable-services); safe reads cannot repeat mutations.
- **H14.07 — Journey/notices:** implement [SC-02](HOSTED_RELAY_ARCHITECTURE.md#first-customer-request-walkthrough), own notices/preferences and service conditions within existing routes.

**Acceptance:** `web/tests/e2e/customer-{journey,failures}.spec.ts` covers funded
journeys, duplicate/lost responses, revoked authority, render/network failures,
SC-03 reconciliation and lifecycle/notice states across mobile, locales and themes.
No repeated payment/key/generation or secret-bearing support copy. Chat, image UI,
teams and saved conversations remain outside scope.

#### H15 — Operator surfaces, Desktop parity and explicit transfer

**Owns:** HC-03/04/07/08/09/10, HD-12. Target: `web/src/features/admin/`, Hosted
management handlers, affected typed Desktop wrappers and transfer tests.
Implement the [owner screen passports](HOSTED_RELAY_WEB_DESIGN.md#owner-panel)
WS-11–WS-23 with H13's shared design system and distinct owner authority; preserve
the explicit Desktop-transfer flow instead of treating web sign-in as synchronization.

- **H15.01 — Actions:** bind controls to server permission, fresh proof, revision, confirmation, audit and failure state.
- **H15.02 — Inventory/pool:** preserve local lifecycle and policy semantics for sources, membership, rotation and publication.
- **H15.03 — Access/usage:** separate logical/physical counts and trusted identity from User-Agent; no Applications launcher.
- **H15.04 — Customers/money/settings:** audited suspension/refund/adjustment and safe diagnostics; expose uncertainty, not prompt bodies.
- **H15.05 — Transfer:** inspect/snapshot/confirm/stage-and-drain/verify, preserving sources and recovery; no implicit synchronization.
- **H15.06 — Compatibility:** paired Desktop shares owner capabilities/revisions/fresh-proof policy without overwriting local profiles.
- **H15.07 — Investigation:** canonical error/detail routes, nested attempts and bounded capture; no raw-payload or replay console.
- **H15.08 — Lifecycle:** implement SC-01/06/07/08 setup, readiness, guarded transitions and change/closure confirmations.

**Acceptance:** `tests/{management_parity,transfer_recovery}.rs` and
`web/tests/e2e/{admin,diagnostics,service-lifecycle}.spec.ts` cover advertised actions,
authority/revision races, source-preserving transfer, safe counters/capture and
closure obligations. Polling/acknowledging cannot trigger inference, change money
or bypass readiness. Check mobile/keyboard and RU/EN states.

#### H16 — Deployment, operations and recoverable release

**Owns:** HC-08/09/11, HD-05/07/09/10/11. Target: `relay-hosted/deploy/`,
`relay-hosted/README.md`, `src/management/diagnostics.rs`, diagnostic-session schema,
web distribution guide and operations acceptance fixtures.

- **H16.01 — Packaging:** pinned compatible artifacts and default/custom-site recipes; no production frontend build dependency.
- **H16.02 — Host/edge:** measured resources, private services, non-root containers, trusted origins and Caddy SSE/WS/cancellation checks.
- **H16.03 — Visibility:** separate probe classes, bounded metrics/logs and external outage alerts with thresholds/recovery/runbooks.
  Unknown telemetry is not zero; keep source maps private and browser/server evidence distinct.
- **H16.04 — Backup:** verified off-host pgBackRest/WAL, compatible vault/config and independently recoverable keys.
- **H16.05 — Restore:** isolated closed ingress; reconcile money, revocations, deletions and uncertainty before reopening.
- **H16.06 — Upgrade:** preflight, stop/drain, single migrator, readiness/smoke checks and schema-compatible rollback or roll-forward.
- **H16.07 — Runbooks:** key/identity/provider/storage/payment/migration failures with safe commands, stop conditions and recovery evidence.
- **H16.08 — Diagnostics:** owner-only fresh-proof/CSRF API, one bounded fenced capture, fixed expiry and persistent counters.
  Missing caps disable capture; optional sink failure cannot lose settlement/audit or cause inference.
- **H16.09 — Readiness/outbound:** version-bound evidence and scoped destinations; no circular prerequisites or health gate on maintenance entry.
- **H16.10 — Lifecycle:** SC-07/08 drills preserve callbacks, refunds, exports and one active authority through closure/migration.

**Acceptance:** `tests/{operations_recovery,diagnostic_capture,readiness,outbound_policy,
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
- **H17.04 — Distribution:** qualify empty-host/default and custom-site installations, compatible artifacts and rendered WS-01–WS-24.
- **H17.05 — Commercial:** resolve HD-01/02/09 with actual merchant, tariff, region/source permissions and customer terms.
- **H17.06 — Operations:** prove measured backup/recovery/capacity, alerts, sink failure, capture stop and rollback in production builds.
- **H17.07 — Sign-off:** trace SC-01–SC-08 to named tests and HD blockers; separate synthetic arithmetic from authorized live evidence.

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
This matrix defines the minimum review coverage.

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
