# Hosted Relay Architecture

**Target service.** An operator supplies permitted provider capacity and sets
retail prices. Customers fund a wallet, call models from their own applications
with Relay keys and see what each request cost. The same installation can offer
[Image Studio](#entering-studio-with-a-relay-key) against that wallet.
The [server](../../hosted/server/README.md) currently boots in closed `setup`,
binds its first owner locally, serves public configuration, and supports routine
owner passkey sign-in, a service-mode overview and server-side logout. A separate
private storage role owns encrypted, revision-fenced connection and proxy secrets.
A private [text worker](../../hosted/server/README.md#first-funded-text-request)
uses the [shared core](../../crates/relay-core/src/gateway/hosted.rs) and a PostgreSQL
wallet for bounded JSON or separately verified native text SSE. The disposable
funded JSON journey links the approved customer's key, Usage receipt and Balance
entry. Public paid inference is closed; the full dashboards remain
[development previews](../../hosted/web/README.md).

## Step-by-step scenario map

These scenarios follow one installation from setup to closure. The
[roadmap](HOSTED_RELAY_ROADMAP.md#delivery-stages) gives the order in which to build and test it.

| Scenario | Action and result | Owning packages |
| --- | --- | --- |
| SC-01 | **Install and establish ownership.** [Local setup](#owner-setup-and-readiness) initializes PostgreSQL and the vault and enrolls the first owner with a passkey. The owner configures customer sign-in, payment verification and recovery. The installation starts in `setup`; registration, funding and inference are closed to the public. | H02, H03, H05, H07, H08, H15–H17 |
| SC-09 | **Turn a connection into an offer.** In [Connections](#from-connection-import-to-first-offer), the owner adds a supported account or API source directly, or confirms a Desktop transfer with its effective proxy. A new connection stays outside the pool until admitted in **Participants**. **Model rules** sets the enabled models, audience, display order and retail prices; **Models** shows what was published. The owner tests the offer with allowlisted identities in `qualification` and opens `live` only after the [launch checks](#delivery-and-acceptance) pass. | H03–H05, H07, H09, H12, H15; H16/H17 for public opening |
| SC-02 | **Go from the landing page to a working client.** A visitor can inspect public prices without signing in. [Verified sign-in](#first-customer-request-walkthrough) creates or reopens one personal workspace and wallet. The customer chooses a granted model, tops up in **Balance** and issues a key in **Keys**; funding and issuance can happen in either order. Only a verified payment credits a top-up. The external client takes the inference base URL, Relay key and model ID; the cabinet does not send the generation. | H04, H06–H10, H12–H14 |
| SC-03 | **Execute and account for a request.** [Admission](#request-lifecycle) authenticates the key, checks the route and reserves a bounded amount at the saved tariff. The shared core leases physical capacity and needs a server acknowledgement before each provider send. Accounting [settles or releases the hold once](#money-scenarios-and-worked-checks); a lost response does not authorize another generation. **Usage** groups the net charge and observed tokens by request start, while **Balance** shows when the journal entry posted. | H01, H04, H07–H10, H12, H14 |
| SC-04 | **Explain a result or investigate a failure.** From **Overview**, a customer opens the matching **Usage** interval, inspects a request or exports the same filtered totals. The owner can use its [safe reference and attempt timeline](#errors-and-production-diagnostics) to distinguish a provider failure, missing evidence and pending settlement. Neither view can read prompts or replay the request. **Status** shows model observations, not a diagnosis of one charge. | H12, H14, H15, H16 |
| SC-06 | **Change the offer.** The owner [publishes a new revision](#price-and-model-change-walkthrough) in Pool rules. Ordinary increases and planned removals follow the notice period; new admissions use the effective revision. Existing holds keep their saved tariff and historical requests are not repriced. | H04, H07, H09, H12, H14, H15 |
| SC-05 | **Notify customers.** An owner may also draft a [service notice](#customer-notifications-and-delivery) in **Notifications**. Saving a draft sends nothing. Confirmed publication creates one in-app notice per eligible recipient; reading it changes only that customer's read state, not the tariff, balance or access. | H03, H06, H08, H12, H14–H16 |
| SC-07 | **Maintain and recover.** In [maintenance](#maintenance-and-recovery-walkthrough), the server refuses new dispatches and funding intents but drains or reconciles accepted work and still handles payment callbacks. After an isolated restore, reconcile money and reapply revocations before reopening. Closing Desktop is not a server shutdown. | H03, H05, H08, H10, H15, H16 |
| SC-08 | **Migrate or close the installation.** [Migration](#service-closure-and-migration-walkthrough) leaves only one host authorized to dispatch, renew credentials or act on money. Permanent closure stops sales, accounts for remaining funds and preserves scoped receipts, exports and support under the reviewed retention policy. Changing mode does not erase obligations. | H03, H04, H05, H08, H12, H15–H17 |

The [screen inventory](HOSTED_RELAY_WEB_DESIGN.md#screen-inventory) specifies what
each tab can do. The [implementation contract map](#implementation-contract-map)
owns the API, data and state transitions. The [scenario evidence checklist](HOSTED_RELAY_ROADMAP.md#scenario-evidence-checklist)
defines how to verify the outcomes above, including failure and recovery.

## Specification before implementation

Define each contract once, covering ownership, failure policy and customer
meaning. Link to its owning section from other documents.

This document owns the target contracts; [the detailed delivery plan](HOSTED_RELAY_ROADMAP.md#hosted-implementation-work-packages)
owns ordered work packages, dependencies and acceptance. `PLANNING.md` continues to
describe implemented behavior.

Before implementing a work package, its specification must identify:

1. User/owner scenario, scope, explicit exclusions and dependency identifiers.
2. Existing code to reuse, new module/file names, callers and authority boundaries.
3. Every persistent entity and field: type, meaning, required/null/default behavior,
   key, uniqueness, index, tenant reference, sensitivity, retention and deletion.
4. Writes/readers, transaction boundaries, lock order, version checks, deduplication
   and recovery after interruption before/after each external effect or commit.
5. States and allowed transitions, actor/guard, terminal evidence, deadlines and
   the distinction between execution, accounting and customer presentation.
6. API method/path, authentication, permission, request/response fields, errors,
   pagination, idempotency, limits, compatibility and sanitized synthetic examples.
7. UI routes, controls, disclosure, loading/empty/stale/error/denied states,
   localization, accessibility and browser caching/logout behavior.
8. Configuration names, validation, secret delivery, deployment/version compatibility,
   migration, backup, rollback/roll-forward and operator recovery steps.
9. Audit and metric events, safe support diagnostics, threat/abuse cases, monitoring
   and how to detect a partial failure without exposing customer content.
10. Named positive, negative, concurrency, crash and compatibility checks, expected
    observable results and the evidence needed for acceptance.

Apply the list to the slice being delivered. The
[first in-process proof](HOSTED_RELAY_ROADMAP.md#first-engineering-task) has no
database, public API or money movement; it needs a trust boundary and observable
send counts, not a production schema or merchant. A slice that *does* introduce
persistence, an API or money must resolve those contracts before writing them.
Use the [decision register](#design-decisions-that-must-not-be-guessed) for
unresolved facts. A design decision and a passing implementation test are separate gates.

Trace each scenario from the screen action through server authorization, the
transaction and any external effect, to the result and recovery check. If one of
these disagrees with the contract, correct the owning section and its test plan
together. Design approval does not establish provider permission or prove a restore.

## Decision

Build a multi-user service around `relay-core`, initially as one modular backend
and deployment unit with bounded workers. PostgreSQL owns durable hosted state;
provider secrets use an encrypted vault with keys outside the database.

Keep the source in the existing `zenith-relay` repository. The default landing page
and customer cabinet are a separate `hosted/web/` application with their own build/tests
and optional deployment artifact; operators can instead connect their own site.
See [application boundaries and deployment choice](#one-repository-and-deployment-choice).

Reuse the existing protocol, routing, scheduler, and stream semantics. Do not
fork the codec engine or scatter Hosted-mode conditions through Desktop.
Use the planned `hosted/server/` composition defined in the implementation map.
The retired personal-server runtime is removed; Hosted is a separate implementation.

### First release and commercial model

The first public release is a complete paid product, not an administrator-panel
demo. The operator supplies account/API-provider capacity; customers buy model
token usage through Relay keys, not provider credentials or ownership of an
account. Provider quota limits available supply; it is not a fungible customer
token balance. Customers see model tariffs, their money balance, limits, usage,
and charges. Never promise unlimited capacity or sell unverified quota estimates.

ChatGPT is the target initial account family, not a claim of permission to
sell that capacity. If it cannot qualify, an API-source-only offer requires
an explicit change of launch scope. Claude and other account families are
subsequent connectors, introduced into Desktop and server together through the
shared lifecycle. A generic Messages endpoint is not a Claude subscription
connector. Each family must qualify its own sign-in/import, token renewal,
entitlements, quota, execution, usage, recovery, and commercial permission.
Customer Google/Telegram sign-in is separate from these upstream credentials.

The target product is multi-provider, not a ChatGPT-only pool. The initial
qualified family limits launch support, not the domain model. Keep provider
account family, issuing client/sign-in method, API source and model publisher
distinct. Additional families reuse shared lifecycle and scheduling contracts;
they do not masquerade as Codex/SIWC/BPS connections or introduce a second engine.
Quota is a shared concept for subscription accounts; its units, scope, windows
and observation methods are provider-specific. Missing quota telemetry means
unknown, not no quota or unlimited capacity. Additional/manual quota resets and
reset credits are optional capabilities, separate from normal quota renewal.
Authentication methods and credential renewal follow each provider's qualified
connector contract, not a common ChatGPT credential shape. Subscription expiry
and transfer support also remain connector-qualified.

Public release includes customer registration/sign-in, account recovery, model
catalog and prices, payment/top-up, scoped request keys, working inference,
usage/receipts, owner management, support diagnostics, and tested operations.
Implementation milestones can be private and incremental; none alone satisfies
this release definition. Customer BYOK charging, account resale, subscription
bundles, and a marketplace are not part of this first offer.

## Product boundaries

- Website and Telegram are clients of one identity/account API, not separate
  balances, user registries, payment truth, or key logic.
- Applications call inference directly with scoped keys. Desktop is a
  [limited operator console](#desktop-hosted-scope), not a required inference hop
  or a second full owner panel.
- Capacity is explicitly platform-shared, workspace-private, or shared by grant.
  Importing a credential does not make it public or commercially usable.
- Provider/product permission, region, credential type, and supported billable
  usage must be reviewed before offering a shared paid route. Working personal
  subscriptions or BYOK keys are not permission to pool/resell them.
- Relay owns its database, credentials and inventory. Neither Desktop's local
  store nor the production Zenith Gateway/Control API is the Hosted backend or a
  source of test customers, provider credentials or money.
- No identity spoofing or sharing concealment.

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
These modules can share a deployment.

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

### Owner and customer permissions

The installation owner controls platform capacity and commercial settings.
Start with explicit owner and customer roles; do not invent a partially enforced
staff role. Delegated operators require a separate action-permission matrix before
being enabled. Workspace ownership never confers installation ownership.

| Action | Installation owner | Customer |
| --- | --- | --- |
| Add, inspect, reauthenticate, disable, park, trash, restore, or remove platform connections | Yes, audited; secret-bearing actions require fresh verification | No |
| Edit pool membership/order, routing, proxies, model enablement, reasoning/context/tier rules and prices | Yes, using the shared revisioned validation | No |
| View active model/member work and platform-wide redacted usage | Yes | Only own requests, limits and usage; no upstream inventory |
| Issue/revoke request keys | Audited restriction or emergency revocation of a customer's key; no issuing, revealing or replacing secrets on the customer's behalf. Use a separate customer session for own keys | Own workspace only |
| Manage identity methods, payments, tariffs, limits and security | Yes; privileged changes require fresh verification | Own linked identities, sessions and account settings only |
| Credit, refund, or adjust a wallet | Through the accounting journal with actor/reason/reference | Request a top-up/refund; never choose a journal credit |
| Read/export provider secrets | Explicit confirmed operation, fresh verification, no-store and audit | Never |

Suspension blocks new requests and key issuance without deleting financial
evidence. Revocation is checked at admission; cancellation of already dispatched
work is explicit and cannot imply zero provider expense. Customer deletion follows
the retention policy, not a cascading deletion of requests or money records.

### Operator interface and customer cabinet

The Hosted web owner panel preserves the current local-pool controls and
semantics, not a simplified replacement. Desktop exposes only the
[limited Hosted scope](#desktop-hosted-scope), not every owner action below.
The server-side interface has no local application launcher/profile attachment.
Preserve Desktop Computer-mode application integrations and LAN access.

| Area | First-release responsibilities |
| --- | --- |
| Overview | Rolling platform statistics including posted customer charges; drill into Usage or a specific blocker. |
| Connections | Accounts, API sources, proxies, authentication, observations and trash/recovery. Edit an API's purchase rates and selling prices together. Saving a source keeps it in inventory. |
| Pool | Participants owns admission, member settings, active work and rotation. Model rules owns pool enablement, display order and offer publication; it uses the same price editor as Connections. |
| Models | Inspect the effective storefront catalog and prices; return to the corresponding rule to make changes. |
| Usage | Investigate a logical request, attempts, timings and settlement. Physical expense is separate from the customer's charge. |
| Notifications | Draft, review and publish an in-app service notice; inspect delivery. |

Setup, owner security, paired devices, support cases, refunds and audit have
[contextual entry points](#owner-browser-navigation). These are privileged
operations, not additional everyday destinations.

Parking means removing pool membership while retaining the connection and its
protected credentials; disabling, draining, trashing and permanent removal remain
different actions. Drain fences new dispatch/refresh owners before destructive
changes; in-flight outcomes must not resurrect a removed connection. Price edits
create a new tariff revision and never rewrite already reserved requests.

Active counts come from current runtime ownership, not requests started in the
last minute. Display logical requests separately from physical attempts/queued
work; show timestamp and stale/unavailable states rather than a fabricated zero.
Read-only dashboard refresh must not probe providers or rebuild the runtime.

The customer cabinet contains sign-in/linked identities, available models and
prices, balance/top-up/receipts, API keys and connection instructions, own usage,
sessions and support. It does not expose operator pool controls. A user-labelled
request means the authenticated key's assigned user, not a name supplied in JSON.

### Desktop Hosted scope

Desktop Hosted is a compact operator console for observing a server and supplying
provider connections to its pool. It is not a customer cabinet or a duplicate of
the full web administration panel. Its planned first-release scope has three areas:

| Area | Desktop responsibility |
| --- | --- |
| Overview | Server-wide request/token/error statistics, generation speed and pool availability; distinguish active logical requests from physical attempts and show stale/unavailable data honestly. |
| Usage | Server-wide request history with period/model/status filters and safe request/error details; no separate customer pages or customer-account administration. |
| Connections and pool | Add supported provider-account/API connections to the server, inspect their status, and explicitly include them in or remove them from the server pool. |

Here an account means a provider connection, not a Hosted customer identity.
Saving a connection does not silently admit it to the pool. Removing membership
retains the saved connection and its protected credentials; it is not deletion.
Advanced routing/model policy, prices and other pool configuration remain in the
web owner panel rather than expanding these basic Desktop controls.

Customer creation/list/detail/administration, customer balances and keys, payments,
refunds, tariffs, installation/security settings and service administration are
web-owner-only surfaces. Desktop has no per-customer cabinet or management flow.
This boundary does not remove the customer's own key/balance controls from the
separate web customer cabinet.

**Connection and authority.** The operator selects a server and completes explicit
owner-approved device pairing. Show the active server identity before reads or
mutations. Grant only the approved overview/usage/connection/pool operations,
with bounded lifetime and revocation; owner identity is not a blanket admin grant
to the paired device. The backend enforces the allowlist on every request, including
direct calls to excluded customer, key, money and settings endpoints. Preserve
revision checks, fresh verification and audit for permitted mutations; hiding
navigation alone does not establish the boundary.

**Local separation and transfer.** Switching modes neither uploads local accounts
nor replaces local profiles, and does not stop the local API. Adding a local
connection to the server requires selection, a source/destination preview and
explicit confirmation under the [transfer contract](#local-and-hosted-pool-transfer).
Preserve credential ownership, recovery and unsupported-family checks; do not
invent automatic synchronization or source deletion. Inference runs on the server
and remains available when Desktop is closed.

**Implemented boundary.** An owner can pair Desktop with one installation for
`connections.read`. The public invitation and the returned approval code are
not login credentials. Native code keeps the device secret in the OS credential
store, and sends it only to the reviewed origin on an explicit inventory read.
The server checks the grant, installation, owner authority and expiry on each
page. **Forget** is local; **Revoke** ends the grant on the server. See the
[pairing procedure](../../hosted/server/README.md#pair-a-desktop).
Aggregate reads, membership changes and confirmed credential transfer remain
separate work. The development web demo exercises none of these authorities.

## Reference-derived implementation decisions

Implement the selected patterns and failure cases in Relay's own frontend and
runtime. Qualify money, security and protocol behavior against Relay's contracts
and official provider specifications; upstream implementations supply reference
patterns and tests.

### Pinned reference inventory

Use these immutable snapshots for reproducible comparison. The source locations
identify what was inspected, not files approved for copying. Re-check a changed
revision and its applicable notices before using any additional source.

| Reference snapshot | Inspected source areas | Selected value and license review inputs |
| --- | --- | --- |
| [New API](https://github.com/QuantumNous/new-api/tree/603a5ceb55908cc24b509aca2caa6fc956c9958c) | `web/src/features/dashboard/`, `wallet/`, `pricing/`; `service/billing_session.go`; `model/quota_reserve_test.go` | Customer journey, usage dimensions and reservation failure cases. Review AGPLv3 `LICENSE` and the UI attribution requirements in `NOTICE`; no frontend import. |
| [Sub2API](https://github.com/Wei-Shaw/sub2api/tree/8f2cbb53d9d1c40e1865b07ac460571f7d4c48de) | `frontend/src/views/admin/ops/`; `backend/internal/service/ops_errors.go`, `billing_inflight_reservation.go` and its tests | Account operations, error drill-down and long-running reservation lifecycle. `LICENSE`/README declare LGPL-3.0-or-later; also review the README commercial-operation disclaimer before any code reuse. |
| [LiteLLM](https://github.com/BerriAI/litellm/tree/8d2e7dca263fe91b5c28900ac4f322a5c47a4124) | `litellm/proxy/management_endpoints/key_management_endpoints.py`; `ui/litellm-dashboard/src/app/(dashboard)/budgets/` | Key policy and budget usability. Root `LICENSE` distinguishes MIT material from the separately licensed `enterprise/` tree; do not assume a documented feature is MIT-licensed. |
| [Bifrost](https://github.com/maximhq/bifrost/tree/2ec9294bbcc7454e3e445ce36eea93c04edaeba5) | `ui/app/workspace/agent-logs/views/agentLogDetailsSheet.tsx`; `plugins/logging/contentpolicy_test.go`; `framework/logstore/contenthidden_test.go` | Correlated request detail and tests separating content retention from visibility. Review root Apache-2.0 `LICENSE` and applicable dependency notices; no imported UI or logging defaults. |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI/tree/3f0cef010e4ad20279500e6b0f18060b6f3f37cc) | `sdk/cliproxy/auth/conductor_cancellation_test.go`; `internal/runtime/executor/codex_executor_retry_test.go` | Cancellation, quota/capacity classification and transport-specific test scenarios. Review MIT `LICENSE` and file/dependency notices; not a replacement engine or provider specification. |

### Selected patterns and owning packages

1. **Complete customer workflow — New API; H12/H13/H14.** Adopt the functional
   progression from overview and authorized prices to a scoped key, usage detail
   and billing history. Preserve one time/model/key filter context between charts,
   tables and bounded exports. Current wallet/active counts remain independent of
   historical filters; missing coverage, pending charge and empty data are distinct.
   Use `/app/billing` for money and the account context for security. Implement this with
   the [original Relay design](#original-relay-design), not source page components.
2. **Operational drill-down — Sub2API; H07/H12/H15.** Connect current model/account
   availability and concurrency to scoped error trends, request/attempt detail and
   permitted operator actions. Saved, parked, disabled, draining and removed remain
   separate states. Account quota exhaustion, model capacity and transient limits
   must not collapse into one broken-account label. Display stale/unknown evidence
   honestly; opening a dashboard never probes or refreshes credentials implicitly.
3. **Understandable key policy — LiteLLM; H09/H14/H15.** Present key name, assigned
   user, expiry, permitted operations/models and effective limits together. Explain
   units, scope and reset semantics; distinguish a period spending budget, request/
   token rate limit, concurrency ceiling and current wallet funds. Keys cannot
   multiply their workspace budget. Server-side admission owns enforcement, and a
   UI estimate or cached counter cannot authorize spend. Enterprise team hierarchies
   and additional roles are not introduced by this comparison.
4. **Correlated, privacy-safe diagnostics — Bifrost; H12/H14/H15/H16.** Follow one
   authorized request reference from customer explanation to owner-only event/
   attempt timeline, component health and current settlement state. Distinguish
   execution outcome from accounting outcome. Metrics, logs and reports derive from
   the same identifiers and evidence; absent events are not an invented root cause.
   Hidden content is not deleted content. Follow our [data policy](#initial-data-policy)
   and [diagnostic contract](#errors-and-production-diagnostics), not a reference's
   default payload retention or client-controlled logging switches.
5. **Failure-oriented connector qualification — CLIProxyAPI; H01/H07/H10/H11.**
   Adapt the scenarios, not assumed provider behavior: quota versus capacity,
   cancellation while waiting for ownership, refresh/removal races, transport
   failure before/after send, streaming terminal evidence and continuation identity.
   Qualify only advertised transports/capabilities. Run them against Relay's shared
   core and synthetic fixtures; ambiguous execution or visible output never becomes
   permission to retry on a new account. Use official contracts where available
   and separately identified observed evidence for account-specific transports.

The following checks make the adopted patterns executable requirements. Extend
the existing package tests.

| Required case | Owning packages | Expected proof |
| --- | --- | --- |
| Parallel keys and a stale/missing cache | H04, H09 | One durable workspace balance/budget; a cache snapshot or unavailable optional cache cannot bypass the required PostgreSQL reservation or permit a duplicate debit. |
| Long stream, cancellation and delayed settlement | H03, H09, H10 | Lease/timeout expiry alone cannot free money while execution/accounting remains unresolved; duplicate completion closes the same operation at most once. |
| Quota exhaustion versus capacity/rate limit | H01, H07, H10 | Correct source/model classification without unjustified credential disablement; alternate dispatch still needs proven pre-execution rejection/not-sent and no client output. |
| Cancelled ownership wait or late refresh | H01, H07 | No leaked gate, stale credential resurrection or new dispatch by a revoked owner; unrelated eligible work remains live. |
| Chart/detail/export and wallet reconciliation | H12, H14 | Same authorized dimensions and coverage; changing history filters cannot change the current wallet, and unknown/partial usage is never shown as zero-cost evidence. |
| Error detail with forbidden content | H12, H14, H15, H16 | Synthetic secrets/markup/customer content cannot appear in routine logs, UI, exports or support copy; authorization and expiry apply to every detail lookup. |

### Reuse and acceptance boundary

For each selected pattern, the owning package records: the pinned reference and
relevant files, the problem it solves, Relay's adaptation, contract differences,
affected callers/data boundaries and named acceptance tests. H00 reviews this
mapping; H17 verifies the resulting behavior.

Default to Relay-owned implementation of these requirements, reusing our core and
existing suitable utilities. No imported gateway, Go/Python sidecar, mandatory
Redis, external analytics store or additional billing service is introduced by
this reference inventory. Keep the recommended first-production stack unchanged.
Before adapting an upstream fix, establish whether its failure is possible in
Relay and add the applicable regression case. Language/runtime-specific workarounds
are not portable requirements; Rust does not by itself prevent incorrect async
ordering, duplicate accounting or unsafe retries. Record an already-excluded case
as such rather than adding redundant machinery. Source comparisons do not establish
a better scheduler, lower latency or higher provider cache-hit rate.
In particular, an optional reference implementation's fail-open in-flight cache
reservation is not our durable financial admission contract.

If an isolated non-visual source reuse is later proposed, identify its exact files/
revision, dependencies and rights holders; review applicable licenses, retain
notices, and handle it as third-party material under [CONTRIBUTING.md](../CONTRIBUTING.md#license-and-contributor-agreement).
Do not describe it as newly authored/assigned Relay code. Upstream access and
commercial permissions remain separate HD-01/HD-02/HD-09 launch decisions; an open
source license or working connector does not close those decisions.

## Customer website

This section specifies the planned first-release customer website. A customer registers,
funds access, obtains connection credentials, understands usage and manages their
account without installing Desktop. Operator controls have their own interface.
Implementation slices are tracked in the [Hosted roadmap](HOSTED_RELAY_ROADMAP.md#customer-website-implementation-slices).

### Frontend application boundary

Build the default Relay site and cabinet as the `hosted/web/` application
inside the existing `zenith-relay` repository, against the Hosted API. It is the
reference client for [operator installations](#public-source-and-operator-installations).
Keep the Desktop application independent.
Reuse transport-independent Relay formatting, i18n, accessible interaction and
chart utilities where suitable; do not inherit Desktop page layouts/styles or
import Tauri commands or local account state. The backend must also support an
operator's own website without deploying the default web application.

### Original Relay design

The detailed [web design specification](HOSTED_RELAY_WEB_DESIGN.md) owns visual
tokens, responsive layouts, screen passports and interaction acceptance. This
architecture owns routes, permissions and data semantics.

Design the Hosted website and operator web surfaces as Relay's own product.
Adapt reusable first-party guides and Studio interactions to Relay's typography,
spacing, navigation and interface copy. Preserve existing
local-pool capabilities and meanings.
Do not redesign the existing Desktop application as part of this web work.

Third-party reference projects inform functional requirements, failure scenarios and technical
patterns, not visual deliverables. Do not copy their screens, CSS/themes, branded
assets, page components or marketing copy, including by reproducing screenshots.
An original design does not require replacing suitable accessible primitives,
chart libraries or existing Relay utilities; review their licenses and retain
applicable notices. New styling never transfers third-party copyright to Relay.

H13 defines the original web design tokens, components and interaction states;
H14/H15 use them for customer/operator surfaces. Review desktop/mobile, both
locales/themes, keyboard use and loading/empty/error/pending states before visual
acceptance. Functional contracts below define information and permitted actions.
Branding configuration customizes Relay's design.

### One repository and deployment choice

The upstream repository contains the Desktop app, `relay-core`, the
existing `hosted/web/` frontend and the planned `hosted/server/` crate. The web
package already has its own manifest, development/build commands and tests. Separate
builds and deployment do not require separate repositories. A normal backend build
or installation must not require building or running the default website.

Keep the customer frontend replaceable, not embedded as a mandatory part of the
server executable. It is our maintained, community-contributable reference client;
operators can use it or implement their own website/cabinet against the same API.
Shared repository ownership lets API and client changes be reviewed and tested in
one change, without allowing frontend imports of backend business logic or secrets.

| Owner | Responsibility | Must not own |
| --- | --- | --- |
| Backend/core modules in `zenith-relay` | Shared engine, Hosted backend, customer API/schema, identity, catalog, accounting and backend operations | Customer HTML/theme implementation or an obligation to run the default frontend |
| `hosted/web/` application | Default landing, API guides, policy pages, optional Image Studio, customer and owner UI, typed API client and independent artifact | Database access, wallet calculation, provider secrets, authentication verification or a second backend |
| Installation configuration | Chosen frontend, domains, branding, legal/support links, registered login/payment integrations and pinned compatible artifacts | Changes to shared authorization, money or request-execution rules |
| Operator's custom website | Alternative customer presentation using the documented Hosted customer API | Direct access to the database, vault, operator management credentials or private engine interfaces |

An operator using a release or building a fork has the same two frontend deployment choices:

1. **Default website:** install the backend and a pinned compatible web artifact
   built from `hosted/web/`, configure the domain/branding and integrations, then use the
   included landing page and complete cabinet. The published artifact works without source edits.
2. **Custom website:** install the same backend without the default web artifact;
   connect the operator's frontend through the published customer contracts. The
   backend has no runtime/package dependency on the web application. Uninstalling or
   replacing frontend assets does not delete accounts, balances, keys or history,
   stop reconciliation workers or change inference routing.

An operator can also retain an existing landing page and link to the default
cabinet; that is composition of these paths, not another identity/wallet system.
Custom navigation may differ, but authorization, charge disclosure, recovery and
other required customer capabilities do not become optional. The full cabinet
specification below remains the acceptance baseline for the default distribution.

### Neutral branding and configuration

The community distribution uses neutral Relay presentation. Installation-specific
branding and integration settings belong in documented, validated configuration:

- Product/display name, title, logo/favicon, theme tokens, default locale and
  landing-page copy/assets; provide useful neutral defaults rather than an empty UI.
- Public hostname/base paths and support, help, status, privacy and terms links;
  required commercial/legal details are completed by the operator before launch.
- Presentation of available login/payment methods from backend capabilities;
  a branding setting cannot enable an unconfigured integration.

Prices, enabled models, grants, money, payment status and usage remain server-owned
data, not editable website constants. Public runtime configuration contains no
client secrets, merchant keys, management credentials or customer data. Use typed
configuration and static assets, not arbitrary operator-provided executable HTML/
JavaScript. Validate link schemes and escape rendered text. A custom implementation
can own its presentation code without turning the backend into a theme/plugin host.

Use the explicitly configured same-origin customer API and integration endpoints.
Missing or incompatible configuration fails clearly, without an automatic fallback
to another deployment. Preserve provenance, license and attribution notices when
reusing code; configurable branding does not remove those obligations.

<a id="public-web-and-private-operator-deployment"></a>

### Public source and operator installations

Hosted Relay is a public, self-hostable system. Operators can run an upstream
release or fork the repository, change the web client or server, and build and
run their own service under the [project license](../../LICENSE). A downstream
change does not need an upstream merge to ship. The Zenith-branded service is
one such installation, not a dependency of others.

Each operator owns their domain, customer accounts, provider connections, payment
integration and data. The public checkout must build without another site's source
or private settings. A fresh installation starts with its own owner and stores;
it does not sign in, process payments or route inference through Zenith.

| Surface | What ships in public Relay | What belongs to the operator |
| --- | --- | --- |
| API connection guide (`/docs`) | Adapt the integration steps and examples from `zenith-site/apps/docs`: choose an offered protocol/model, create a Relay key in **Keys**, configure the external client, find the request in **Usage**. Examples use placeholders and this installation's inference URL. | Domain and support destination. A guide must not advertise the old site's models, protocols or prices as this installation's offer. |
| Policies and support (`/legal`) | Reuse the document layout and navigation for privacy, terms, payments/refunds and support. These pages are readable before sign-in and checkout. | Publish reviewed text or links for the actual seller. The public template is not a policy; missing required disclosures block paid opening. |
| Image Studio (`/studio`, when offered) | Adapt the image workflow from `zenith-site/apps/studio` as a customer client. Offer Generate only for an authorized, priced image operation; expose editing, uploads or masks only when supported. | Enable the qualified offer. Studio uses the same workspace, wallet, limits and Usage as external clients; it has no separate balance or merchant. |
| Brand and payments | Neutral assets, [payment method builder](#payment-method-builder), server adapter contract and checkout. | Own branding, legal copy and merchant accounts. Credentials are write-only or supplied by deployment secret reference; never include them in source, API responses or public configuration. |

Port reviewed, redistributable material, not the existing site's production
login/payment services, `/web/` bridge, API hosts or customer data. No route
falls back to Zenith production. Serving the new site does not migrate existing
users, balances or receipts. Keep the current site and inbound links working
until the operator verifies equivalent destinations and approves a separate cutover.

### Entering Studio with a Relay key

Public information (`/`, `/docs`, `/legal`, public `/models`) requires neither
sign-in nor a key. Studio and any other browser tool that uses a Relay key follow
the same entry sequence:

1. Open the tool. Without a valid customer session, go directly to `/login` and
   return to the validated tool path after sign-in. With a valid session, skip login.
2. Choose one of the customer's eligible keys by name and masked prefix. If the
   account has no keys, replace the picker with **Create key**, linking to
   `/app/keys`. The cabinet requires sign-in, not a pre-existing key. Only explicit
   confirmation there issues a key. The customer stays in **Keys** after creation.
   When they return to Studio themselves, they select the key without pasting its secret.
   A failed key lookup is not an empty list. Creating a key cannot restore a missing model grant.
3. Choose an eligible model, review its rate and explicitly submit. Only submission
   can reserve funds or dispatch. The Studio handler requires the customer session
   and CSRF protection, resolves the selected nonsecret key ID within that workspace
   and enforces its live scope and limits through normal Hosted admission. A
   revoked, expired or foreign key cannot dispatch; never substitute another key.
   Public inference endpoints still require their normal inference credential.

Studio is optional per installation; its inclusion at the Zenith-operated launch
remains open. Before enabling it, qualify the Studio handler
under HD-08, the image tariff and reservation bound under HD-06, and the receipt
in Usage against the selected key. After a lost response, check execution and
charges by request reference; do not promise to recover the image or generate it
again automatically. Keep prompts, uploads and results out of browser persistence
and diagnostics.

### API compatibility and fast deployment

The Hosted backend owns a versioned customer API/schema for session, catalog, keys,
wallet, payments, usage and safe reports. Publish that contract with supported
versions and synthetic examples so another frontend can integrate without importing
backend code. It is a dedicated Hosted contract. The default web
client uses that same public customer contract, without privileged shortcuts or
private endpoints available only to the default frontend.

Initially publish separately built backend and web artifacts from the same tested
Relay release/revision; separate packaging does not require a second release cadence.
Record their API compatibility and qualify supported mixed-version pairs before
allowing independent upgrades or rollback. Detect incompatible contracts/capabilities
with an actionable error; never substitute fabricated data or silently change monetary meanings. Backend
updates need regression coverage against the supported default web versions and
customer-contract tests applicable to a custom client. Customer API/schema changes
run the relevant backend and web checks together; path-filtered CI must not skip
client compatibility merely because only backend files changed.

Supply an installation recipe that pins a supported backend/frontend pair and
offers the default web artifact as an optional service/static deployment. Keep
backend/data/worker services independent of this choice. Use separately built,
versioned static or container artifacts from the same repository, not a Git
submodule or a requirement to compile the frontend from source on the server.

The recommended production topology remains same-origin behind Caddy: route the
registered API paths before default/custom web assets. Independently deployed
applications do not require cross-origin browser requests. Preserve the existing
session, CSRF, origin and passkey boundaries; do not enable wildcard credentialed CORS or shared
parent-domain cookies to make a custom site work. An actually cross-origin setup
needs its own explicit supported authentication/origin contract and acceptance,
not an arbitrary API URL entered into an untrusted webpage.

### Community workflow and web acceptance

Make `hosted/web/` a clear contribution area in the existing repository: app-specific
README/quick start, local development with synthetic fixtures, configuration
reference, compatibility information and tests. Reuse the repository's contribution,
security-reporting and release policies.
Community pull requests can improve layouts, accessibility, translations, charts
and client integrations; coupled API changes are reviewed in the same pull request.
Preserve the existing license and applicable attribution notices. A shared source
repository never weakens backend trust or accounting boundaries.

Use normal reviewed changes and pinned releases, not live installation of arbitrary
community scripts/themes. No new marketplace or plugin system is required. Before
the default website is accepted, demonstrate that:

- A clean installation obtains a usable landing page/cabinet from released artifacts
  with documented operator settings, without source edits.
- The configured domain/brand works without unconfigured URLs, login registrations,
  tracking, support destinations or payment assumptions leaking into the deployment.
- The backend works with a minimal independent test client when the default website
  is absent; replacing assets preserves wallet, key, session and usage ownership.
- Both integration paths enforce the same customer isolation and financial contract;
  a frontend change cannot broaden grants, create credits or reveal provider inventory.
- Supported release pairs, incompatible-version errors, upgrades and frontend rollback
  are tested without rolling back database state or weakening backend policy.

Develop `hosted/web/` inside Relay with the [original Relay design](#original-relay-design).
Review provenance for first-party ports and reusable dependencies; do not import
third-party reference-project pages/assets. Review source/assets, examples,
build configuration and release artifacts for secrets, customer data and
unconfigured external integrations before publication.

### Deployment origin

The operator configures the installation's HTTPS origin through
`server.public_origin`; no particular domain is part of the product contract.
The default website and a custom website use the same Hosted API and authorization
rules. An operator can set branding and origin through configuration alone.

The initial topology serves customer routes, starting at `/app/overview`, and the
Hosted API from the configured origin with distinct registered paths. A separate
API hostname is not required. Configure customer-login callbacks and allowed
origins for this deployment, and validate the administrator WebAuthn RP ID/origin
relationship explicitly. Use host-only session cookies without parent-domain sharing.

Qualify DNS/TLS, callback URLs, passkeys and deep links before launch. Origin/RP ID
changes follow the documented revalidation procedure; neither the engine nor the
reusable frontend contains a hardcoded deployment hostname.

### Navigation and screen responsibilities

Public entry explains the token-based service, enabled login methods, published
prices and connection requirements, with privacy/terms/support links. A login
button is shown only for a configured method. Public model information contains
no upstream inventory and does not promise access beyond the customer's grants.
After sign-in, return to the validated customer destination, or open Overview
in the personal workspace if there was none.

Public `/` explains the service and has a second screen at `/#faq` / `/#features`,
whose answers are visible without expanding individual items; `/login` owns customer sign-in.
The quiet Models and pricing link targets the **public browser page `/models`**
in the paid product (the [current landing](../../hosted/web/src/features/landing/LandingPage.tsx)
links to the preview `/app/models`). The planned public page reuses the read-only
catalog layout, without the cabinet shell. It uses `GET /public/models`
under the Hosted API prefix, never the authenticated customer model projection or
frontend constants. Publish only deliberately public qualified model descriptors,
supported operations/protocols and complete retail tariff/band revisions, including
effective times. This public projection is bounded and read-only, exposes no
customer grants, provider inventory, physical capacity or credentials, and is not
a promise that a signed-in customer can use every listed model. Customer access
and actual readiness remain separately checked. Missing/stale publication data
is visibly unavailable, not a free or fabricated price. H07 owns its field contract.

Planned browser routes:

| Section | Target route | Content and primary actions |
| --- | --- | --- |
| Overview | `/app/overview` | Usage and posted spend for a rolling period; current activity and important notices. Balance and key shortcuts belong in the sidebar. |
| Usage | `/app/usage` | Filtered logical request history and aggregates, safe request detail, settlement state and bounded CSV export. |
| Models and prices | `/app/models` | Read-only authorized catalog, supported protocols/capabilities and current published tariffs. |
| Model status | `/app/status` | Authorized model rows and [observed history](#model-status-history); catalog presence is not a health observation. |
| API keys | `/app/keys` | Create, label, restrict, rotate and revoke own keys; show masked metadata and last use, never a stored full secret. |
| Balance and payments | `/app/billing` | Available/reserved funds, top-up, payment status/receipts, wallet journal and refund/support entrypoint. |
| Account and security (account context, not a sidebar item) | `/app/overview?panel=account` | Verified sign-in methods, sessions, revocation and recovery once H06 is connected; language and theme stay in the shell. |

Keep the six primary destinations and their order in the [web route registry](../../hosted/web/src/features/app/routes.ts).
The public guide, policies and optional Studio are [site destinations](#public-source-and-operator-installations),
not more cabinet tabs.
Keys and Billing already have preview screens, not working server mutations.
Connection instructions belong at key issuance and support in context; `/app/help`
is not a destination. Account recovery must remain reachable without restoring a Help or Settings sidebar entry.
Browser route names do not rename the Hosted API's `/wallet` and `/account` resources.

Use one desktop navigation and a compact mobile equivalent. Balance and Keys
sit above the rail's footer divider; sign-out is below it. The header holds the
page context, bell, language and theme, not another wallet or key action.
Start with the personal workspace;
do not advertise team management or organization switching before its permissions
exist. Operator navigation stays separate and cannot be exposed by a customer-side
role toggle. There is no Connections/account inventory, pool editor, provider-cost
report, Applications launcher or Prometheus/Grafana access in the customer cabinet.

### Owner browser navigation

These are browser routes, distinct from control API paths under `/api/hosted/v1`.
`/admin` redirects a verified owner to `/admin/overview`, otherwise to `/admin/login`;
it cannot exchange a customer session for owner authority. Initial owner creation
remains the server-local ceremony. A public browser page cannot claim installation
ownership. H13/H15 and the [design screen inventory](HOSTED_RELAY_WEB_DESIGN.md#screen-inventory)
use this table instead of introducing competing route names.

| Destination | Target browser route | Responsibility |
| --- | --- | --- |
| Owner sign-in | `/admin/login` | Existing-owner passkey ceremony and safe recovery guidance; no customer-provider buttons |
| Overview | `/admin/overview` | Rolling statistics, posted customer charges and a link for an actionable setup, service or money issue |
| Connections | `/admin/connections` | Accounts, API providers, proxies and trash; save inventory or start an explicit admission |
| Pool | `/admin/pools` | Participants: membership, current work and rotation. Model rules: grouped state and order controls, the shared price editor and offer publication |
| Models and tariffs | `/admin/models` | Read-only effective catalog, not another tariff editor; an owner edit link opens the relevant Pool rule |
| Usage / errors | `/admin/usage`, `/admin/usage/requests/{request_id}` | Logical request list, Errors tab, nested attempts and safe observed timeline |
| Notifications | `/admin/notifications` | Owner drafts and confirms in-app service notices; publication is separate from saving or delivery |
| Access (context) | `/admin/access` | Connections → Paired devices; authorized support case → restrict/revoke its affected key. No global key-issuance console |
| Customer case (context) | `/admin/customers` | From a permitted request/payment or an exact customer support reference in Usage. Inspect or restrict that workspace; no browse-all directory or impersonation |
| Payment case (context) | `/admin/payments` | Overview → refunds or payments needing attention, or exact payment reference from support. Reconcile, review a refund or record an audited adjustment |
| Operations / setup (context) | `/admin/operations`, `/admin/operations/setup` | Overview → service mode/readiness, incident or recovery task; reachable even when there are no requests |
| Settings (context) | `/admin/settings` | Setup step → corresponding configuration; signed-in owner context → security/recovery. No general preferences dashboard |
| Audit (context) | `/admin/audit` | From the changed object, operation or incident; inspect attributable history, never edit it |

Primary navigation stays Overview, Connections, Pool, Models, Usage, Notifications.
Other rows are protected task destinations for the paid service (e.g. setup,
refund review or access recovery), not extra sidebar pages to scaffold now.
Tabs/sheets use validated browser state and opaque authorized IDs; they do not
grant direct access to API families or secrets. An exact-reference lookup is
authorized and audited; it never searches across installations or grants the
requester's authority to the owner. H05/H07/H08/H09/H15 own these permissions.

### Overview and chart layout

The [Overview design](HOSTED_RELAY_WEB_DESIGN.md#ws-03--overview) owns composition.
Current wallet/activity is separate from historical filters. Reports retain
unsettled requests and incomplete usage; chart/table navigation preserves filters.

Overview defaults to 24h and offers rolling 7d and 30d windows ending at server `as_of`;
owner Overview also offers All time over retained coverage, with bounded bucket
count. Carry exact UTC `[from, to)` into Usage on drill-down. An explicit Today
filter in request history starts at midnight UTC; it is not the 24h chart.
Server buckets and accessible tables share these bounds. If only calendar-day
aggregates survive, mark unsupported edge intervals partial/unavailable rather
than prorating totals. Coverage is authoritative: request detail expires under the
[data policy](#initial-data-policy), while older daily aggregates may remain.
Disable unavailable drill-downs and explain the retention boundary. A custom range
must not imply that deleted request detail, latency distributions or key/model
dimensions are reconstructable from a coarser aggregate.

Charts and their accessible tables share one filter/snapshot contract. Filters
include period, authorized model, own key and logical outcome where supported;
retain historical key labels for revoked keys without revealing their secrets.
Optional previous-period comparison uses an equal complete UTC window and is
unavailable for incomplete coverage or a zero baseline, not an invented percentage.
Show an explicit group for smaller series if the chart is truncated; table totals
must still reconcile. Viewing or refreshing a chart never issues model requests.

### Metric and money semantics

| Metric | Customer-visible definition |
| --- | --- |
| Requests | Unique admitted logical requests, not physical provider attempts or duplicate idempotency hits. Pre-dispatch admission rejections are separately labeled activity, never extra generations. |
| Outcomes | Canonical execution outcome; HTTP 200 or an open stream alone does not prove completion. Separate active, completed, incomplete, failed, cancelled and uncertain states. |
| Completion rate | Completed logical requests divided by terminal requests with known outcomes; disclose the denominator, exclude active/uncertain work and show unavailable for an empty denominator. |
| Tokens | Server-normalized components with their completeness/overlap semantics. Never add cached input twice or reasoning again when already included in output; unknown is not zero. |
| Usage cost | Net posted customer debits attributable to the selected logical requests, including request-linked compensating entries; exclude holds, top-ups, unrelated wallet adjustments and provider expense. |
| Available / reserved | Authoritative current wallet snapshot. Reserved funds are temporarily unavailable, not an additional posted charge; pending top-ups are not available money. |
| Latency | Server-computed observed timing with a named population and sample count. TTFT is separate from total duration; do not average per-bucket percentiles or invent unretained timing data. |
| Active / queued | Current tenant-scoped ownership snapshot with an observation time, not requests created during the last chart bucket. |

Usage reports attribute requests, tokens and request-linked charges to logical
request start time. All views disclose the reporting range and data `as_of`;
late usage/settlement or a compensating entry can revise a historical bucket.
Wallet history instead uses journal posting time. Explain that these time bases
differ, so a day's wallet movement is not necessarily that day's generation cost.
Settlement state is independent of execution outcome: failed/uncertain execution
does not imply free usage. Display final, estimated, pending and waived/released
states according to the accounting contract, with an explanation of the basis.

Use server-supplied normalized totals and charge breakdowns, not browser billing
or the Desktop API-equivalent estimate. Carry money as exact integer microUSD
encoded without JavaScript-number precision loss, with currency and tariff revision.
Follow the [rounding/display policy](#initial-tariff-and-wallet-policy), including
six-decimal small charges. A headline can be compact only if the exact value is
available. Missing price/usage, expired detail and a genuinely empty bucket are
different states; draw zero only for a confirmed, fully covered empty bucket.

### Usage, model and key interactions

Usage rows show time, public model, masked key label, protocol, outcome, latency,
available token components, customer charge and settlement state. Use server-side
filtering and stable cursor pagination; do not download the entire history to the
browser. Safe detail exposes the logical request ID, supported usage/timing, applied
tariff breakdown and actionable error/retry guidance. It must not expose upstream
account identity, provider secrets, physical routing diagnostics or raw error bodies.
Sanitized User-Agent is a client label, never authenticated user attribution. Do not
promise prompt/answer history; request bodies are not the usage archive.

CSV export respects the same workspace, filters, snapshot and retention, with
server-enforced row/size bounds and explicit truncation/coverage. Include currency,
UTC timestamps and usage/settlement completeness; neutralize spreadsheet-formula
injection in user-controlled labels. Export no credentials or raw content. Older
aggregate export is labeled as aggregate data, not reconstructed request history.
The short-lived request report supports direct CSV from its bounded snapshot
with no saved export artifact or provider call. Queued jobs cover larger and
retained aggregate exports.

The model catalog comes from the Hosted authorized projection. Show public model identity,
supported protocol/capabilities, current price per million tokens and relevant
context/cache/service-tier bands. Distinguish no grant, temporary capacity trouble,
disabled model and missing price without leaking pool capacity or account details.
New tariffs affect new reservations; past request detail shows its snapshotted price.

Retail comparisons are optional server publications, not browser arithmetic.
Match the reference model, component, tier, context band, currency, unit and
effective period. Without current comparable evidence, show only the retail
price; never infer a discount from the model name or a Desktop estimate.

Key creation shows the full secret once after confirmation, only in transient page
memory with an explicit copy action. Losing it requires rotation, not reveal. The
key list contains prefix/label, scopes, limits, expiry, revocation and last use.
Requested permissions cannot exceed workspace grants; limits cannot multiply the
wallet or workspace budget. Confirm revocation and explain that it blocks new
admission without promising cancellation/refund of already dispatched requests.
Connection examples contain placeholders unless the user explicitly copies the
just-issued secret; never put keys in links, analytics, browser storage or logs.

### Customer journeys and exceptional states

Use the canonical [first-request](#first-customer-request-walkthrough),
[money](#money-scenarios-and-worked-checks), [identity](#identity-and-account-channels)
and [diagnostic](#errors-and-production-diagnostics) flows. The dashboard cannot
auto-run paid tests, resend uncertain generations or infer credit from redirects.
Deleting a key/account does not erase financial obligations.

Every page has loading, empty, partial, stale, unavailable and forbidden states.
Never substitute demo data or zero balances after a fetch failure. Keep last-known
safe data visibly stale where appropriate, with observation time and manual retry.
Use bounded polling only while visible, with backoff and server retry guidance;
stop on sign-out or workspace/session changes. Cancel obsolete filter requests so
late responses cannot overwrite another range or show a previous account's data.
Do not introduce a separate realtime service just for graphs.

### First customer request walkthrough

SC-02 is the default cabinet journey. The same server-side rules apply to a custom
site; no privileged default-frontend shortcut is allowed.

| Step | Customer action and server effect | Visible result and recovery |
| --- | --- | --- |
| 1. Visit `/` | Read the four FAQ answers or inspect anonymous prices at `/models`. Start/Open cabinet targets `/app/overview`; only a missing customer session requires `/login`. | Seeing a public price creates no account, grants no access and spends nothing. Empty publication, closed registration and service interruption have distinct explanations. |
| 2. Sign in and choose | Use a configured login method. A verified callback creates or restores the personal workspace and wallet. In **Models**, compare the granted offers and copy the exact model ID/protocol. | Cancel returns safely; repeat callback returns the same account. With no usable offer, explain the restriction or point to Status/support; do not suggest topping up to unlock a missing grant. |
| 3. Fund in **Balance** | Select amount/method, review the server quote, confirm once and complete the provider's checkout. Return to the same payment detail. | Pending is not spendable. A confirmed journal credit changes available funds once and supplies the receipt. A lost return can be recovered from payment history. |
| 4. Create in **Keys** | Name the inference key; optionally narrow supported limits/expiry. Issue and explicitly copy/store the secret from the one-time result. | Only the masked record survives dismissal. Creation and funding can occur in either order; neither sends inference. Lost issuance is recovered by operation, not a second automatic key. |
| 5. Configure the external client | In the client's custom provider settings enter the public **inference** base URL for that protocol, the Relay key as its API credential, and the model ID. Keys keeps the nonsecret setup fields reachable later. | Copyable examples use placeholders; never paste an upstream credential or use the management API origin/path as the inference base. If the client cannot speak an offered protocol, stop here rather than promising compatibility. |
| 6. Use the model | The user sends a real request from that client, not from the landing or cabinet. An admitted generation gets one logical ID and bounded hold. | Read the response in the client. For proven rejection correct funds in Balance, key in Keys, or model in Models. After a timeout with possible execution use the original ID/status or matching idempotency claim; do not promise an answer replay. |
| 7. Reconcile in **Overview → Usage** | Select a rolling window/model, open a bucket, then its request. Compare execution, tokens, saved tariff and posted charge or pending hold. Export that filtered snapshot if needed. | Link a posted charge to the wallet entry. Usage groups by request start; Balance lists postings by posting time. **Status** answers whether the model has observations, not whether this particular request was billed. |
| 8. Return or retire a client | Subsequent visits open Overview or an authorized deep link. Use Usage's safe request reference for support; replace a key, switch the client, then separately revoke the old key. | Past requests/receipts remain readable. Revoking the key blocks new use; browser sign-out does not revoke client keys or cancel existing work. |
| 9. Close the personal workspace | Use the verified request/support process in **Account and security**. Explain how to obtain receipts and request a refund of eligible unused funds before access ends. Once closure is confirmed, block new key issuance, funding intents and dispatch for that workspace only. | Already accepted requests and payments reconcile under their original references. Closure is neither a refund nor instant erasure: [retention and restore](#initial-data-policy) preserve required records without reactivating keys. Other customers and the installation keep operating. |

If key issuance committed but the response was lost, identify the resulting key
by the same operation without disclosing its secret; explicitly revoke it and issue
a replacement only on a new confirmed action. A page reload is not that action.
The Relay key authenticates the client; an `Idempotency-Key` identifies one
logical request within its scope. Repeating the same prompt with the same Relay
key alone is a new request. If the client did not supply an idempotency key and lost the response,
check Usage before deciding whether to submit a **new** request.
Enter the cabinet directly. An empty Overview can point to Funding and Keys;
key issuance provides the base URL and a placeholder configuration. No welcome
dialog, automatic top-up or first-use generation. Telegram can link to this
panel and support without owning a second customer-management interface; keep
legacy fulfillment available until existing obligations have been reconciled.
Explain "available", "reserved", "charged" and "pending" in
each supported locale beside the relevant values, with the [money examples](#money-scenarios-and-worked-checks).

### Customer notifications and delivery

SC-05 uses the customer header bell plus contextual
banners. Financial history remains authoritative;
a notice or notification-provider acknowledgement never changes money or proves
that a person read a message. Operator outage alerts continue to use the documented
monitoring stack and independently reachable destination, not this customer feed.

Launch default: in-app notices are available to every customer. External customer
delivery is optional, explicit opt-in, and limited to a verified recipient through
an operator-enabled qualified channel. Do not infer email permission from a login
claim or Telegram messaging authority from sign-in. No arbitrary customer webhook
URL, automatic marketing subscription or automatic support-data upload.

| Event | Recipient and trigger | Content/action and deduplication |
| --- | --- | --- |
| Balance low | Own workspace, after a committed balance change crosses its configured threshold | Available/held distinction and Billing link; one notice per low-balance episode, rearm only after confirmed recovery above the threshold |
| Payment confirmed or needs attention | Intent owner, after the authoritative transition | Safe payment reference and committed/pending/failed meaning; unique intent/state version, no provider payload or full payment details |
| Reconciliation needs attention | Request owner, when the domain marks a delay/issue requiring action | Safe request reference and honest pending/settled state; no fabricated "free request" or resend button |
| Account security change | Affected user, after identity link/unlink, recovery or session/key revocation | Safe action/time and link to own account controls; never the new credential, identity token or complete key |
| Scheduled price/model/service change | Affected authorized workspaces, after approved publication | Exact effective time, affected public models and next step; unique announcement/revision, no private account inventory |
| Owner service notice | Customers eligible when the owner confirms publication; recheck access before delivery | Bounded plain-text title/body in an approved template, once per recipient and published revision. In-app only; no external recipient, HTML or custom link. |

In `/admin/notifications`, saving a draft has no recipients or outbox effect.
Publication uses owner proof, expected revision, operation ID and a server-side
audience cutoff. Later registrations do not inherit that broadcast. Discard
unpublished drafts freely; never rewrite a published message in place.
The [qualification implementation](../../hosted/server/README.md#in-app-service-notices)
commits the notice, its inbox items and the operation acknowledgement together for
a maximum of 16 recipients. A failed transaction leaves a draft and no recipients;
after a lost response, retry the same operation. Public audiences require durable
outbox checkpoints and delivery rechecks rather than a larger transaction.
Informational text cannot publish a tariff or substitute for its required notice.

The low-balance threshold is a positive editable microUSD amount chosen by the
customer; unset disables that optional trigger rather than inventing a universal
minimum purchase. A temporary reservation may trigger a notice, which must say
that some funds are held. Do not send a notice for every failed request or attempt.
Routine alerts have cooldowns; lifecycle/security notices retain their distinct
event identities rather than disappearing into a money-warning bucket.

Delivery steps: commit the domain transition with an idempotent notification-outbox
entry; project one recipient-scoped in-app notice; enqueue optional external delivery
through existing durable jobs; recheck the recipient binding/consent and expiry;
call the channel outside database locks; record sent/failed/unknown evidence.
An unavailable optional delivery channel never rolls back a payment or retries
inference. Missing outbox insertion is retried with the same domain operation;
outbox processing can be rebuilt from the committed event without another effect.

External delivery is at-least-once where the channel cannot deduplicate: use its
idempotency facility when available, otherwise disclose possible repeated delivery
after lost acknowledgement. Never claim exactly-once human delivery. Keep bounded
retry/backoff/expiry and one logical notice per recipient/event/channel. Unsubscribe,
recipient change, workspace revocation and stale announcement recheck before send;
mark no-longer-eligible deliveries suppressed. Reading/marking a notice read is not
settlement, incident resolution or authorization to contact a different recipient.

Store only typed message keys/allowlisted values and protected recipient references.
Notice links are same-origin route templates with server-authorized object lookups,
not bearer-token URLs. Keep in-app notices 30 days and terminal delivery metadata
14 days by proposed default; unread state does not extend retention. The original
journal/security/audit evidence follows its own policy. Show when a notice is old,
superseded or no longer actionable instead of rendering a stale promise as current.

### Web data boundary, accessibility and acceptance

Split the preview's combined `HostedSnapshot` at the wire boundary. Anonymous,
customer and owner responses are separate server projections. Never send private
inventory to customer JS and filter it there. Only dev/test can inject fixtures;
production never substitutes a local balance, demo key, tariff or session on failure.

The frontend needs versioned Hosted contracts for session/workspace, catalog,
wallet snapshot/journal, payment intent/status, key management, request page/detail
and aggregate reports. These are required capabilities, not invented routes in
a shipped Desktop API. Reports return normalized filters, coverage,
bucket granularity, `as_of`, completeness and pagination metadata. `as_of` fixes
the time range, not the state of late corrections. Pin a committed read-model
watermark across charts, request pages and exports; recheck authorization on
every read. If that snapshot expires, require a fresh query instead of mixing
revisions or holding a database transaction open across browser requests. Authorization
and monetary calculation stay on the server; user-supplied workspace/key/model IDs
are selectors to validate, never authority. Sensitive responses use no-store.

For the restricted installation, report creation copies bounded safe receipts
from one PostgreSQL statement snapshot. The report ID pins that population,
including settled charges and posted corrections; model selection, bucket
drill-down and pagination read the same copy until expiry. The cache is disposable
and has [explicit limits](../../hosted/server/README.md#customer-reports).
It is not the ledger or a substitute for the retained aggregates required for
larger reports and production retention.

Keep server-revocable HttpOnly sessions with CSRF protection for mutations. Store
no tokens, keys, balances, request history or payment data in localStorage,
IndexedDB or service-worker caches; in-memory caches are keyed by authenticated
scope and cleared at sign-out. Only non-sensitive presentation preferences may
persist. Public static asset caching must not cache authenticated API responses.
Embed no third-party session replay or analytics that captures customer data.

Support English (default/fallback), Russian and Simplified Chinese, light/dark themes, narrow/mobile screens and keyboard
navigation using the original Relay web design. Provide visible focus,
descriptive form errors, reduced-motion behavior and tables/text alternatives to
charts; color alone never conveys outcomes or balance restrictions. Keep filters
and destructive/payment confirmations usable without pointer hover.

Before calling the cabinet complete, verify with synthetic accounts and focused
frontend/API acceptance:

- A clean browser can register, fund access, create a key and inspect the first
  request; an existing account can return with its correct wallet and history.
- Cross-workspace IDs, revoked/expired sessions and stale responses cannot expose
  another customer's keys, payments, aggregate totals or request detail.
- Charts, summary cards, tables and exports agree for the same snapshot, including
  retries, overlapping token components, unknown usage, late settlement and refunds.
- Duplicate payment callbacks cannot credit twice; pending redirects cannot show
  a funded balance, and repeated key actions cannot expose a previous secret.
- UTC boundaries, empty/partial periods, retained aggregates versus expired detail,
  zero-denominator comparisons and unavailable latency are represented honestly.
- Balance/usage API failures, revoked keys, suspension, capacity errors and lost
  login sessions have actionable UI without paid retries or leaked diagnostics.
- All customer routes work in each shipped locale and both themes on mobile and with keyboard;
  refresh/deep links preserve authorization and charts have equivalent tables.

Use mocked fixtures only in tests/development and label previews accordingly.
Full live identity/payment/provider acceptance remains a separate authorized release
gate. [Image Studio](#public-source-and-operator-installations) is a separate
customer workflow, not a paid test button in the cabinet. General chat/playground,
team management, saved conversations and subscription bundles remain outside this scope.

## Identity and account channels

Customer federation is in the first release. The installation owner configures
enabled methods: Google, Telegram, and a standards-based OIDC provider for a
self-managed identity system. One enabled, verified method is sufficient for a
deployment; support and test the three integration paths before public release.
Additional methods are explicit adapters, not arbitrary scripts or a promise that
every identity service is compatible. Provider settings/secrets belong to the
owner and vault, not to unauthenticated customer input.

Keep the owner/admin [passkey-only sign-in](#passkey-only-administrator-access)
separate. Enabling Google or Telegram for customers never grants an administrator
session. Telegram sign-in is distinct from a Telegram bot; a bot uses the same
account API and cannot maintain a second wallet or authenticate a raw user ID.

Use each provider's verified server-side login contract. OIDC binds a short-lived,
single-use authorization attempt to the configured issuer/client/redirect and
browser session, with state, PKCE where applicable, and nonce/token validation.
Validate signature, issuer, audience and expiry using a maintained library.
Telegram uses its supported OIDC authorization-code flow with PKCE and validated
ID tokens; a forwarded profile or chat ID is not proof of login. Its configured
bot represents the login application, not a separate customer account store.
Allowlist destinations and callback origins; do not discover/fetch arbitrary
customer-supplied identity endpoints. Login tokens are not provider-pool secrets.

External identities have a unique `(provider, subject)` linked to internal
`user_id`. Linking requires an authenticated account or approved one-time
challenge, never a matching display name/email. Telegram IDs and payment
callbacks are not authentication by themselves.

First successful registration atomically creates one user/personal workspace.
Concurrent callbacks cannot create duplicate wallets. Linking a second method
requires a fresh authenticated session and proof of the new identity; a previously
linked identity cannot move to another user implicitly. Never merge on email.
Allow unlinking only with a verified remaining method. Normal recovery uses the
identity provider or another linked method; support does not reset ownership from
a matching email or Telegram name. Disable a provider only after warning about
affected users and preserving an explicit recovery path. Blocked users remain
blocked across all linked identities and their active sessions are revoked.

Browser sessions use secure HTTP-only cookies and CSRF protection. Sessions
are bounded/revocable; recovery and unlinking cannot orphan the owner.
Payment intents resolve the owning workspace server-side.

**First restricted sign-in.** The installation pins one public OIDC client, its
issuer, and its authorization, token and signing-key endpoints. `/login` starts a
three-minute code flow with PKCE S256, nonce, state and a browser-bound attempt.
In setup a verified subject receives only an opaque review ID (ten minutes).
No name, email, login token, wallet or session is created from that ID. The
tester gives the ID to the owner over a trusted channel. The owner confirms the
IDs with a recent passkey, CSRF, expected revision and operation ID. This opens
only the named customer-identity qualification scope for up to 24 hours; it is
not readiness evidence for payments or inference. Pausing invalidates its sessions.

A second verified login against the current scope creates the user, workspace,
empty wallet, external identity binding and session in one transaction, or reuses
the existing binding. Scope and user revocation, the provider configuration,
installation epoch and deadlines are checked on session reads. Matching emails
never identify a returning customer. Subject keys are versioned keyed digests
of the registered authority and case-sensitive subject; do not replace the digest
key to recover access. The first adapter accepts RS256, a single audience and
same-origin endpoints. Other provider profiles require separate qualification.

Implementation references: [Google OpenID Connect](https://developers.google.com/identity/openid-connect/openid-connect)
and [Telegram Login/OIDC](https://core.telegram.org/bots/telegram-login).
Revalidate provider configuration and library behavior at implementation time.

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

### Abuse controls and compromised access

H05/H06/H09/H16 specify numeric bounds and fixtures for these Hosted trust boundaries before
exposure; a generic "rate limited" checkbox is not sufficient.

| Boundary | Required control and visible behavior | Required failure case |
| --- | --- | --- |
| Anonymous login/setup | Bound ceremony starts, challenge size/lifetime and failed verification; generic unauthenticated errors, no public owner reset | Repeated guesses or fabricated user identifiers do not enumerate users or lock a victim out indefinitely |
| Registration/linking | Verify identity before creating durable customer objects; bound attempts and pending challenges; linking never trusts matching email | Repeated callbacks create one identity/wallet and cannot consume unlimited storage |
| Customer API and inference | Enforce key + workspace + installation bounds, queue age, request/body/export sizes and fair scheduling | Many keys do not multiply one workspace's allowance or starve other eligible workspaces |
| Stolen customer key | Owner can revoke the exact key; customer can do so through the own-key action; invalidate cached authorization before further dispatch | Revocation races queue/dispatch correctly; it cannot silently erase in-flight usage or release money twice |
| Owner/session compromise | Revoke affected owner/device authority, stop extended capture, rotate affected credentials through the vault and audit recovery | A revoked device/session cannot continue mutations using cached capabilities or stale fresh proof |
| External integration addresses | Only owner-configured, reviewed purposes/destinations; credentials bound to the selected destination, redirects and outbound policy | A caller cannot turn a request/model/payment/login field into an arbitrary server-side fetch or forward a secret to another host |

Reuse the bounded core scheduler. H09 qualifies starvation/queue behavior with mixed workspaces and both
short and long streams. Separate unauthenticated abuse controls from customer money
and inference counters; an attacker cannot assign charges or sanctions using a
supplied workspace ID. Do not trust arbitrary forwarded client-address headers.

The outbound policy is installation-owned and purpose-scoped: provider/API source,
identity metadata, payment adapter and notification channel. H16 specifies allowed
schemes/ports, DNS resolution and redirect handling, proxy ownership, credential
forwarding rules, and blocked local/internal/metadata destinations. A deliberately
self-hosted private provider needs an explicit reviewed exception, never a blanket
"allow private network" switch available to customers. Validate the actual connected
destination, not only an initial URL string. Changed DNS/redirect targets must not
reuse approval for a different destination. No new generic URL tester or log viewer.

The closed native text slice uses an installation-configured HTTPS hostname and
public IPv4 pin per source. Discovery, probe and paid execution share that
transport policy; PostgreSQL binds each observation to the pin. See
[outbound approval](../../hosted/server/README.md#direct-https-outbound-approval).
This is not evidence for external proxy support or the deployment's H16 gate.

Use short-lived, restricted abuse counters; define TTL, cardinality and cleanup
before collecting client-address derivatives. They are not request history or a
permanent tracking database. Public quotas/backoff messages remain safe; limits
cannot turn into a bypass for payment/webhook verification or mandatory settlement.
Missing security/authorization state fails closed for affected operations, while
authenticated history and recovery remain available where their own checks pass.

### Passkey-only administrator access

Human sign-in to the Hosted administration panel is passkey-only
through WebAuthn/FIDO2. Google/OIDC login, passwords, email/SMS sign-in, Telegram
authentication, and wallet signatures cannot authenticate administrators.
This restriction does not exclude first-release customer federation above.
A passkey stored in Google Password Manager is not Sign in with Google.
Desktop retains its local authentication.

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
- Inference keys stay non-administrative, and diagnostics/backups expose no
  private auth material.

Implementation references: [WebAuthn](https://www.w3.org/TR/webauthn-3/),
[server-side registration](https://developers.google.com/identity/passkeys/developer-guides/server-registration),
and [server-side authentication](https://developers.google.com/identity/passkeys/developer-guides/server-authentication).
Recheck the selected library and applicable specification at implementation time;
these references do not commit Relay to a JavaScript backend or a Google login.

## Pools, connections, and model access

A route is available only under the authenticated grant/key, an enabled member,
compatible endpoint/credential/features, and current quota/health policy.
The same model has different readers:

| Reader | Source and limit |
| --- | --- |
| Anonymous visitor | Hosted `GET /public/models`: deliberately published offers and retail tariffs only. A listing is not a grant. |
| Signed-in customer | Hosted `GET /models`: the workspace's authorized catalog, with applicable prices and context bands. [Status](#model-status-history) has a separate observation history. |
| External inference client | `GET /v1/models` on the inference base: usable public IDs for **this key** and supported surface. A narrower key can legitimately list fewer models than its owner's cabinet; no wallet, prices or upstream inventory. |

Serve these reads from scoped server projections of core discovery and policy.
The existing [Desktop model handler](../../crates/relay-core/src/gateway/request/codex_models.rs)
can refresh account access and load client-specific manifests. Do not mount it
as a Hosted read-through to providers: opening a picker cannot probe an account,
generate traffic on its behalf or grant access. Apply current authorization even
when the catalog snapshot is cached.

Effective input limits combine model/route evidence, grant, key, and service
limits; output is independent. Unknown hard limits are not advertised guarantees.
Route identity includes public/upstream model, endpoint, protocol, adapter,
evidence, and revision. Aliases/protocol bindings do not multiply physical capacity.

### Model status history

`/app/status` uses current customer catalog grants for its rows and H12 observation
history for its cells. H07 reconciles core discovery by stable model identity:
an unseen model enters owner inventory without a frontend release. It reaches
each customer/public projection only when that projection's own grant/publication
rules pass. Losing a grant removes the customer's row, including cached history.
An open tab picks up changes on its next authorized refresh; an old tab cannot
keep a revoked grant usable for dispatch.

`GET /models/status` returns the Models order, one server UTC anchor and half-open
rolling bins (48 × 30m or 28 × 6h). Initial qualification uses the same authorized
listing projection for both routes. The publication epoch survives price changes,
but a withdrawal and republication start a new epoch. Every read reauthorizes the
session and catalog; the browser clears prior rows when it rechecks.

Service evidence is a completed 30-minute UTC slot attested for that publication
by a qualified monitor. Intersect only slots inside the publication and complete
at the read anchor. A fully covered interval displays its observed state, or
degraded for mixed states. Any gap keeps the interval neutral and the tooltip
distinguishes partial coverage from no observation. Availability is fully
operational time divided by monitored time, with partial coverage disclosed.
Catalog checks, absence of errors and customer requests cannot fill slots. The
qualification installation has no continuous collector or monitor write grant.
Qualified ingestion and retention must precede live monitoring.

Requests are durable logical receipts started during the current publication
and belonging to the caller. Before that coverage there is no count; within it
zero is meaningful. Cache uses observed cached/nonzero input pairs, taking
accepted late usage over earlier evidence, and reports sample coverage. Latency
is unknown until instrumented; settlement timestamps are not timing samples.
The snapshot allows 64 listed models and scans at most 8193 workspace receipts;
over 8192 receipts, usage metrics remain unknown instead of showing truncated
counts. No physical member, foreign traffic or private model is exposed.
Reading this or `/public/status` never sends a probe.

### Shared rotation and Hosted admission

Use one core selection policy for Desktop and Hosted. Connectors supply typed,
fresh quota evidence in the provider's units and scope; quota-aware selection
does not require an additional reset capability. The scheduler does not assume
ChatGPT quota windows, manual resets or subscription deadlines for other families.
Missing evidence stays unknown and cannot grant capacity or an expiry/reset
preference. Connector-qualified admission remains authoritative when a provider
does not report numeric quota.

Separate two responsibilities: Hosted bounds and fairly admits work across
workspaces/keys; the shared core selects a qualified connection for admitted work.
Many keys cannot multiply a workspace allowance, and healthy account rotation
does not prove fair customer admission. Qualify both under H09 without duplicating
the core scheduler.

All strategies first preserve authorization, route compatibility, physical limits,
[paid fallback consent](#paid-api-fallback-for-subscription-offers),
[API purchase limits](#api-purchase-limits), provider waits, execution certainty
and continuation ownership. Within those constraints, Automatic balances
independent work and fresh expiry/resource evidence while retaining eligible
warm conversations. Expiring first prioritizes
fresh deadlines for independent work; round-robin/request shares and primary/reserve
retain their explicit user-selected meaning. Reuse the current core definitions
in [PLANNING.md](../project/PLANNING.md#pool-and-execution), not another Hosted scoring formula.

For paid API supply, Automatic chooses the lowest estimated purchase cost among
stable, interchangeable routes for the requested offer. Hosted supplies approved
[source rate cards](#api-source-rates-and-the-public-tariff) to the shared selector.
First enforce the grant, exact operation/tier, required features, ownership,
capacity and current health. Stability needs fresh qualification or observed
success under the route's health policy; an unknown route or one in cooldown
does not win on price. Compare each remaining source's **whole** rate card using
the same prepared input and output budget. Do not assume a cache hit or discount;
use the applicable conservative cost when the split is unknown. Break equal
estimates using existing affinity and balancing. With one suitable source use
it; with none, use the existing bounded wait/unavailable path. A cost change
cannot move a pinned turn, authorize a retry after an uncertain send, or change
the customer's tariff. This API preference does not redefine account rotation.

Qualify correctness separately from efficiency. Compare policies on matched
synthetic workloads before permitted live measurements: completed/rejected work,
queue delay, starvation, member switches and unused expiring capacity. Measure
provider-reported cached tokens, first-output/end-to-end latency and supported
cost evidence separately. An affinity hit is not a provider cache hit. Current
thresholds are versioned heuristics, not an established optimum or proof of
superiority to another implementation. Explain decisions through existing bounded,
opt-in diagnostics, not continuous trace capture or a new mandatory UI panel.

### Paid API fallback for subscription offers

**Off by default.** In **Pool → Model rules**, the seller may authorize named
metered API connections as backup for one subscription-backed offer in that
pool (model, operation, tier and context band). When enabling the offer, the
seller confirms its primary supply class. Store it and the approved backup IDs
in the offer's revisioned policy; an empty set means off. Import, discovery,
member order, rotation presets and **No Relay limit** do not grant permission.
A replacement source cannot inherit approval by reusing a label. An API-only offer uses
its approved APIs directly. Losing the last subscription does not turn a
subscription offer into an API-only one.

| Subscription state for a new request | Dispatch decision |
| --- | --- |
| A compatible subscription can serve it | Use the subscription. One exhausted member is not enough if another can serve. |
| All enabled subscription routes otherwise qualified for this offer have fresh, connector-confirmed quota exhaustion | With consent, consider only the approved, stable APIs qualified for the same offer. Without consent or a usable API, take the ordinary bounded wait/unavailable path. |
| No matching subscription remains, or the only evidence is stale quota, a busy slot, an unclassified `429`, a timeout or an auth failure | Do not spend on an API on that basis. A missing reading is not evidence that quota ran out. |

Apply rotation **within** the permitted supply class. For a paid attempt, check
the request's admitted source scope against the current owner policy and
recheck exhaustion, grants, capabilities, region and [purchase limits](#api-purchase-limits)
at the pre-send permit. Enabling fallback cannot add a paid route to already
queued work; disabling it fences new permits, not an attempt already committed.
This switch does not authorize subscription overage, credit purchases or quota resets.

A subscription rejection can lead to a paid send only with proven
pre-execution rejection or non-send, replayable input, no client-visible output
and valid ownership. Respect the remaining retry budget and scoped provider
waits. Never move an opaque continuation or retry an uncertain
send. Once quota recovers, new independent requests use subscriptions again.
An in-flight stream finishes on its route; a new turn pinned to a now-disallowed
route gets the existing continuation error, not a silent migration.

Keep one logical request, one customer hold and its original selling tariff.
Record the consent revision, exhaustion evidence reference and purchase cost
basis on the paid attempt in owner Usage. Customers see their normal receipt
or a capacity error, not the supplier's identity or a prompt to top up their
own wallet because the seller ran out of funds.

### Pool model controls and catalog order

The web owner panel retains local-pool management semantics for every qualified
account/API-source family. Its capabilities come from the backend; the browser
does not calculate eligibility, rotation order or effective prices. The paired
Desktop console retains its [limited scope](#desktop-hosted-scope); parity here
means the full web owner panel versus local Computer-mode controls.

Keep these controls distinct:

| Control | Authoritative effect |
| --- | --- |
| Connection membership, enable/drain and per-member model rules | Determine which saved capacity may serve an eligible request; saving a connection does not admit it. |
| Pool-wide model enablement | Combine with member rules, grants and qualified routes. Effective disablement fences new admission/dispatch; it does not delete history, reprice reservations or silently cancel dispatched work. |
| Public catalog publication | Controls which safe descriptors/offers are exposed; visibility alone grants neither route access nor execution capacity. |
| Retail tariff and supported context/service-tier variants | The seller sets [a markup or fixed selling rates](#api-source-rates-and-the-public-tariff) and publishes one tariff per offer. The serving route does not set the customer's price. |
| Company/model display order | Changes presentation only, never member rotation, eligibility, capacity or billing. |

Group models by validated catalog publisher/company identity, for example OpenAI,
not by the serving account's sign-in method, gateway address or a guessed name
prefix. A mixed-model API source can contribute to several publisher groups.
Unresolved publisher metadata stays neutral rather than inventing an attribution.

The backend owns one revisioned `modelDisplayOrder` of complete company blocks.
Owners can reorder whole groups and models within their own group; dragging cannot
change a model's publisher. Preserve disabled models' positions, append newly
discovered models within their group and reject stale writes without losing other
settings. Owner, authorized-customer and public projections preserve the applicable
relative order after visibility/grant filtering. Search, collapse and temporary
filters do not rewrite it or create a second client-side ordering policy.

Show the model's readable name, effective state and prices with currency/per-million
units; retain the exact ID for copying/details. Keep input, cache and output rates
and applicable variants distinguishable. Blank/unknown is not free, and an enabled
switch does not imply a runnable paid route. **Pool → Model rules** edits prices
inline; **Connections → API providers → Edit → Models** opens the same pricing
controls in source context. **Models** shows the effective offer and links back
to its rule. H04 owns tariffs; H07 owns grants, order and activation.

For the standard shared offer the owner approves a revisioned audience policy
once, not a list of individual customers to add to a pool. H07 applies it to eligible
existing and newly registered workspaces and rechecks it on reads and dispatch.
A private/restricted offer still needs its own explicit grant. Discovery can match
an already approved exact offer, but cannot create a new tariff, grant or permission.
Removing access also invalidates stale key/catalog views; it does not rewrite receipts.

## Provider onboarding and native API surfaces

A connector records provider/product, credential kind, region, exact API/version,
features, limits, usage/quota method, pricing, permission review, and revision.
Revalidate its official contract at implementation time; provider/example URL
tables are not evidence that a model or plan supports a surface.

Native upstream and client ingress are separate: permitted ingress bridges use
the existing four codecs to reach the chosen model's exact native surface.
Unsupported required semantics fail before dispatch. A route may require
`native_only`; this restriction applies to that route's ingress conversion.

Versioned mappings must establish the same underlying model and model provider;
a matching public name is not enough. The [paid API backup](#paid-api-fallback-for-subscription-offers)
is a separately qualified route for the same offer, not a substitute model or
a credential swap. Credential rotation stays within the same provider, product,
model, surface, compatible capabilities and retry contract. Relay keys are
replaced with only the selected vault credential at the provider boundary.

Do not sell a route without permitted commercial use, supported billable usage,
a valid tariff, and finite enforceable reservation bounds. Otherwise keep it
private/BYOK-only where permitted, or disabled for paid serving.

### Server-side provider connection onboarding

The owner has two explicit entry paths: authorize/import a supported connection
directly into Hosted, or transfer a selected local connection through the
[preview/apply/recovery flow](#local-and-hosted-pool-transfer). Neither requires
Desktop to remain an inference or renewal hop. Provider connection sign-in is
separate from the owner's passkey and the customer's Google/Telegram/OIDC identity.

For direct onboarding, choose a qualified family and method. Hosted owns the
authorization attempt, binds it to the authorized actor, installation, connector,
intended principal/target and expiry, and validates completion before vault storage.
The owner may complete provider consent in a browser; exact redirects, device flows,
client registration and renewal must be qualified for that provider. A local-only
flow is not automatically valid on a server: offer a qualified transfer or sign-in
method, not an invented callback or silent grant reuse.

Cancellation, timeout, duplicate completion and replacement fence older attempts.
Expired or terminal attempts discard temporary exchange secrets and retain only
the bounded safe outcome required by the authentication/audit retention policy.
Reauthentication preserves the intended connection's settings and rejects a
different principal unless an explicit replacement operation is approved. Persist
the issuing authority and credential generation; coordinate refresh through the
existing ownership boundary. Browser persistence, logs and normal API projections
never receive provider secrets. A saved connection remains outside the runnable
pool until explicit admission and exact-route qualification succeed.

### From connection import to first offer

SC-09 starts after [owner provisioning](#owner-setup-and-readiness). An owner
opens **Connections** on the named installation. Choose the entry path according
to where the credentials already live; never ask for a customer's Relay key.

| Starting point | Owner action | What can be saved |
| --- | --- | --- |
| A new provider account connection | **Accounts → Add** → qualified connector and issuing method → approved provider consent. If the connector supports file import, explicitly select its supported credential file instead. | Match the verified provider principal and issuing method, not a display name/email. File input is bounded and transient. No scanning browser profiles, cookies or local app folders. Do not import renewable credentials still managed elsewhere outside the fenced transfer flow. |
| An API provider | **API providers → Add** → label, allowed base URL/protocol, write-only provider credential and intended proxy/direct route. Review observed models or declared IDs awaiting qualification. | One source with a protected credential reference. Editing a label is not reauthentication; changing endpoint, credential or proxy invalidates relevant observations. A typed model ID is not proof of support. |
| An existing Desktop connection | From Connections' device action, pair the intended Desktop with limited scope. **In Desktop**, select the destination installation, the local accounts/API sources and *Transfer*; review the server plan before confirming. | Static API source: confirmed copy. Transferable OAuth: [stage and drain](#local-and-hosted-pool-transfer), one refresh owner. The current [Desktop contract](../project/PLANNING.md) does not support SIWC Hosted transfer; omit that method. Offer destination sign-in only if independently qualified. |

1. **Preview.** Show the destination, safe source/issuer label, the effective
   proxy (specific, inherited or explicitly direct), dependencies and the effect
   on the source. For a batch, mark each row new, already present, conflicting
   or unsupported. Select only eligible rows. A preview may stage expiring
   protected input for a direct, explicitly submitted import; Desktop transfer
   preview sends **metadata only**. Neither admits, publishes or overwrites. Any provider check
   with an external effect is a separately named action.
2. **Confirm.** Bind the selected item IDs, intended principal/target, destination
   revision and proxy mapping to one operation. An unchanged duplicate is a no-op
   and retains its existing settings/membership; a different identity, authority
   or binding is a conflict, not a merge by name or host. Resolve it explicitly or
   skip the row. Confirm against the latest revision before committing any change.
3. **Read the result.** Report each row as saved outside pool, unchanged, blocked,
   awaiting ownership verification or failed, with a safe reason and operation
   reference. A durable write failure is not “saved.” After a lost reply/restart,
   look up that operation; retry only proven failed rows after a fresh review.
   Cancel/expiry discards unused staged secrets, never a committed credential or
   the only recoverable OAuth generation. The [proxy contract](#preserve-the-effective-proxy-during-transfer)
   applies through discovery, renewal and dispatch, including after Desktop closes.
4. **Admit.** In **Pool → Participants → Add**, choose from saved inventory,
   review and confirm. A Connections shortcut opens the same membership action.
   Adding from inside the picker returns to the selection; it neither admits
   silently nor resumes a paused member. Membership is not evidence of a runnable
   route. A blocked member shows the repair target, not a green activity light.
5. **Offer.** Reconcile core discovery; an unseen model appears in the owner's
   publisher group. In **Pool → Model rules**, validate enablement, the applicable
   price components/bands and audience; review and explicitly publish. **Models**
   shows the effective offer only after activation. Public listing, customer grant,
   key eligibility and [observed status](#model-status-history) are separate reads.
   A missing tariff or permission stops at the owner; no frontend model constant,
   made-up rate or backfilled green interval.

SC-09 ends when a qualified client request can be admitted under the approved
offer. Importing inventory must also be useful on its own: the owner can stop
after step 3, sign out, and later find the parked connection without having sold
anything.

The implemented [Desktop copy path](../../hosted/server/README.md#copy-one-desktop-api-source)
saves one native Responses source at a time. The pairing grant reads inventory;
it cannot write. In **Connections → Desktop source copy**, the owner uses a
fresh passkey to approve a ten-minute grant for the exact installation, device,
source, and reviewed route. Desktop confirms separately, checks the approval
and re-reads the saved source. A change to the reviewed address, key or proxy
requires a new review. A private transfer worker commits the connection and
receipt with its vault slots; the browser handles only review metadata. Resolve
an uncertain write with the same operation. Local model rules, prices, and
membership are not part of this copy.

## Request lifecycle

### Paid ingress and dispatch boundary

Hosted registers an allowlist of method, path, operation and transport at the
server, not the whole [Desktop/LAN router](../../crates/relay-core/src/gateway.rs).
A new core route, alias or WebSocket upgrade is closed until it has its own
authorization, grant, finite bound and evidence contract. Catalog/status reads
cannot cause inference. Do not translate customer credentials to a shared local
key or create independent per-tenant schedulers that each count the same member.

Pass a server-created workspace/key/request context to the shared execution
path. Before **every physical generation send**, including an eligible retry or
connector-internal generation, obtain a host acknowledgement for the fenced
attempt against the same logical request and approved bound. If the fence or its
commit cannot be confirmed, do not send. If a send may already have happened,
reconcile the original attempt; a lease expiring is not proof of non-execution.
The core [usage callback](../../crates/relay-core/src/usage.rs) is an observation,
not a durable pre-send permit, a payment receipt or proof of non-execution when absent.

A client WebSocket upgrade authenticates a transport, not future turns. Each
generation on it needs current key/grant/mode checks and a distinct request and
reservation. Recheck before send even on reused upstream sockets. Control frames
do not create billable generations. Do not reuse a handshake idempotency key for
multiple turns; leave paid client WebSocket closed until the exact client has a
qualified turn correlation, duplicate/reconnect and status contract.

### Execution and reconciliation handoff

Keep durable execution and accounting state separate:

```text
execution: received → admitted → reserved → [queued] → dispatched → [streaming]
           → completed | incomplete | failed | cancelled | uncertain
accounting: none → held → settled | released
                      → held_for_reconciliation → settled | released
```

`incomplete` is a known non-complete terminal result, not an alias for `uncertain`.
Preserve a safe normalized reason and the available usage evidence; do not infer
completion from HTTP 200, output delivery or payment settlement. Customer `active`
groups nonterminal execution states; accounting is a separate state machine.

1. Bound parsing/connections and authenticate key/workspace before lookup.
2. Validate request, grants, route features, context/output caps, idempotency,
   and tariff. Size prepared input before the reservation transaction.
3. Atomically create the request and bounded reservation; no transaction spans
   provider I/O or streaming.
4. Acquire bounded physical capacity and execute through a server-created core
   scope. If prepared billable quantities change, revalidate or adjust the hold
   within approved bounds at the saved tariff before dispatch. A higher supplier
   purchase price cannot increase the customer's hold.
5. Capture upstream usage before conversion and validate completion. Record one
   logical outcome and every dispatched attempt, including uncertain failures.
6. For a route offering resume, commit reusable continuation state before
   advertising resumable success. A stateless turn needs no such checkpoint.
   Settle once and release unused funds. Disconnect cancels supported upstream
   work but does not prove non-execution or zero expense.

### Reconciliation and estimated settlement

Requests proven not dispatched release immediately; a missing acknowledgement
is not that proof. A possibly dispatched request without final usage gets a
finite, snapshotted reconciliation deadline; worker retries/restarts cannot
extend it. Use authoritative final usage received before closure. Otherwise
apply the first matching row at the deadline:

| Evidence | Closure policy |
| --- | --- |
| Valid cumulative partial usage with a supported, priceable component | Settle supported observed components as `provider_partial`; missing components remain unknown. Do not add an estimate on top. |
| No usable provider usage; the saved, published estimator applies to observed substantive output | Settle only the supported estimate as `settled_by_stream_estimate`, capped by the hold. Never estimate an unsupported hidden component. |
| Neither usable usage nor an applicable estimate | Release as `policy_waiver`, including when no substantive output was forwarded or the evidence is too incomplete for the estimator. Provider expense can still exist. |

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
For a same-scope, same-input duplicate, return the original logical request ID
for status lookup even if the first response was lost before headers. The new
HTTP correlation ID is not a replacement. Conflicting or foreign requests must
not disclose the prior request.

A scoped Relay status endpoint exposes execution/accounting states, usage
completeness, settlement basis, and currency/precision, not prompts, answers,
continuation content, credentials, or foreign metadata. Pending amounts are not
zero. Publish a durable deduplication window covering execution/reconciliation/
supported retries. Without a key, each POST is a new request; prompt similarity
cannot establish duplication. Existing core retry safety still applies.

## Durable continuations and restart recovery

Before enabling resume, its schema needs tenant/key-scoped `continuation_sessions`: origin
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

Commit a completed-turn checkpoint before promising resumable success. Earlier streamed IDs
remain pending; partial tool arguments are not resumable. Restore only committed,
unexpired state after key/grant/owner/authorization/version checks. Restart is not
reauthentication; refresh preserves ownership only under the connector contract.

Advance turns with short version-CAS transactions and fenced execution leases.
Concurrent distinct turns cannot overwrite state; deliberate branches need child
identities. Expired leases imply uncertain execution, not permission to resend.
Graceful upgrades stop admission and drain/flush under a deadline. Abrupt restart
cannot preserve sockets or unobserved output; uncertain work enters reconciliation.
Persist hard quota/retry deadlines separately from disposable affinity/cache hints.

For supported OpenAI routes, apply the shared
[migration contract](../../.agents/skills/relay-protocol-adapters/references/contracts.md#openai-model-migrations):
retain ordered configuration/tool updates and native reasoning under the same
provider/end-user/organization identity. Neither a matching cache key nor restored
metadata proves reasoning reuse. Keep provider-native compaction distinct from
summary reconstruction.

## Customer balance and provider expense

An append-only double-entry journal owns money; balances are reconstructible
projections. Use one explicit wallet currency and checked integer subunits.
The initial precision/rounding policy is below; validate every tariff before paid traffic. Corrections are
compensating entries, not edits/deletions. Keep customer charge, physical provider
expense, provider quota, and operator pricing/margin distinct.

### Initial tariff and wallet policy

The initial wallet currency is USD, represented as signed integer microUSD
(1 USD = 1,000,000 units), with checked wider intermediates for multiplication.
Payment-provider currency conversion, if needed, is quoted and accepted before
payment; never mix settlement currencies in one wallet or infer an exchange rate.

The owner publishes a revisioned retail tariff per public model, displayed per
million tokens. Preserve the existing token components and evidenced context/service-tier
bands: ordinary input, cached input, output, and separately supported cache-write
or reasoning components only where they are not already included. Reuse core
usage parsing and price data structures; do not treat its API-equivalent estimate
as a posted customer debit or provider invoice. A model needs an explicit complete
retail tariff before paid enablement, even if a reference price exists.

For the current OpenAI cache contract, ordinary input, cached reads and cache
writes are disjoint components of total input. Price mixed write lifetimes
separately; a write rate replaces the ordinary input rate for those tokens,
not an additional charge on top. Do not count both aggregate writes and their
TTL breakdown. Retain missing usage/prices as unknown and qualify core parsing
for the exact route before enabling its paid tariff. Do not impose provider
reference rates as the owner's retail prices or infer subscription quota spend.

Sum supported component costs with exact decimal/integer arithmetic and round
up once to microUSD at logical-request settlement, never per streamed chunk or
token. Reservation bounds round conservatively. Display at least six decimals for
small charges; presentation rounding does not change the journal. A confirmed
zero tariff is explicit; missing pricing or usage is not zero. Persist tariff and
rounding version with every reservation/settlement. Top-ups are prepaid; no
automatic overdraft or recurring subscription is implied.

Paid opening requires at least one [enabled top-up method](#payment-method-builder)
qualified for the merchant, region and channel, including refunds and reversals.
Provider quota exhaustion rejects/queues within published limits, never sells
more concurrency just because customer money is available.

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
dispatch. Provider billing may continue after visible output stops.
Settle one logical customer charge from supported evidence; each physical
attempt retains separate expense/usage even when it fails.

### API source rates and the public tariff

The API provider's **Edit → Models** editor puts **Purchase** and **Sale** side
by side. Purchase belongs to the selected API: enter its USD rates per million
ordinary input, output, cache-read and supported cache-write tokens (including
distinct lifetimes). Version them by upstream model, operation, service tier and
context band. A missing required cost disqualifies that source for that paid
variant, not other sources. A catalog reference or subscription estimate cannot
fill the gap.

Sale belongs to the seller. Set one optional default markup on the API, then
choose how to price each public model/operation/tier/band in the same editor:

| Model setting | How the server prepares selling rates for review |
| --- | --- |
| Use API default | Apply each approved source's own default to its purchase rates. An unset default supplies no proposal; an explicit `0%` proposes selling at cost. |
| Model markup | Use the seller's percentage for this offer **instead of** each API default. It is not a second markup and does not affect other offers. |
| Fixed sale prices | Use the exact component prices entered by the seller for this offer, across every source. A blank required selling rate blocks publication; it never falls back to markup. |

Markup is a percentage **of purchase cost**: `sale = purchase × (1 + percent / 100)`
for each component. Use exact decimal arithmetic and the [rate precision rules](#naming-value-types-and-schema-boundaries).
With one complete proposal, preview its marked-up prices. With several, preview
the highest **selling** rate for each comparable component across complete
proposals for that offer, not the highest raw purchase price. With none, there
is no markup-based tariff to publish. Compare only approved supply on matching
terms; temporary routing health does not enter the calculation. Fixed prices
bypass it. In review, flag a selling rate below a known comparable purchase
rate; the seller can still publish it.

Saving a cost or default does not publish prices. On review the server shows
which offers would change and snapshots the rule and its inputs. Supplier edits,
discovery and outages never rewrite a saved, scheduled or effective tariff.
If a markup input changed since review, require a fresh preview before
[publication](#price-and-model-change-walkthrough). Customers see only the
published selling rates. A reservation and its settlement use that one tariff
even after a permitted fallback. The selected API and its purchase revision
belong to the physical attempt: they inform [routing](#shared-rotation-and-hosted-admission)
and expense estimates, not a customer surcharge or an official discount claim.

### API purchase limits

The seller funds the API directly with its provider. **No Relay limit** is the
default: Relay imposes no additional purchase ceiling, but the provider's credit
and quota still apply. A connector-confirmed lack of credit blocks the affected
supply until the connector confirms it can serve again. A stale or unavailable
balance is neither zero nor unlimited; use the connector's normal admission
policy. Relay neither moves supplier funds nor automatically replenishes them.

The seller can also set a **Monthly purchase limit** in USD per API connection.
It applies across that connection's models, aliases, keys and customers. Both
controls apply independently: a provider top-up does not reset the Relay limit,
and raising the Relay limit cannot fund the provider. The limit covers purchases
made through this connection, not the provider's whole invoice or the customer's
wallet. For example, with $100 prepaid upstream and a $60 Relay limit, Relay
can authorize at most $60 of bounded purchases while that limit applies.

Count purchases in the UTC calendar month of the pre-send permit, even in
no-limit mode. Before acknowledging each physical attempt, atomically record
its conservative upper cost at the saved **purchase** rates. Under a cap, the
month's accounted cost plus outstanding bounds plus this attempt's bound must
fit; unknown exposure is not zero. Settle the attempt once against supported
expense evidence, releasing any unused bound. Release in full on proven non-send;
after an uncertain send keep the bound counted until reconciled, independently
of customer settlement. An actual cost above the bound stays visible in full;
block that source variant until its bound is requalified, without charging the
customer the difference. Late evidence stays in the original month. Changing or
toggling the limit does not erase earlier purchases or cancel in-flight work.

If either control disallows a new attempt, skip only the affected supply. Another
qualified source in the request's approved supply scope may serve the same offer
under the existing ownership and retry rules, at the saved customer price. With
no eligible source, follow bounded wait/unavailable handling; a request proven
never sent releases its customer hold.

### Payment method builder

A new installation has **no merchant connections and no enabled payment methods**,
including Stars. The installation owner configures checkout under **Setup → Money →
Payment methods**. Customers choose how to pay; they do not connect merchants.

The local qualification build exposes this as **Admin → Payment methods**.
The private worker registers immutable capabilities for a provisioned sandbox
binding; no credentials, URLs or merchant identity are accepted by the owner
form. The local check covers supported choices and amounts, not provider
connectivity or payment qualification. A successful check applies to the exact
saved revision for ten minutes. Live merchant setup below is gated by HD-02.

The Store Bot reference is `zenith-store-bot` at `ec3e9b462de4`: `src/keyboards.ts`
defines the visible choices, `src/payments/registry.ts` the external link adapters,
and `src/handlers/payments.ts` the Stars flow and Platega method mapping. These
are candidates for Hosted templates, not live merchant settings or proof that
the same checkout works on the web.

| Bot integration | Customer choices in the bot | What must be qualified for Hosted |
| --- | --- | --- |
| Platega | SBP (`2`), Russian bank card (`11`), Platega Crypto (`13`) | One merchant ID and secret, three independent choices. The bot supplies Telegram user metadata; do not invent a Telegram identity for a web customer. Qualify the payable total and currency for each method. |
| CryptoBot | CryptoBot, under Cryptocurrency | Configure the operator's application token and accepted assets; bind a verified paid invoice to the accepted quote. |
| Lolzteam | Lolzteam | Configure the operator's merchant ID, token and callback; verify the invoice with that merchant. The bot also checks availability before showing this choice. |
| Telegram Stars | Telegram Stars (`XTR`) | Connect the operator's own bot. Use its native invoice and verified successful payment, not a browser redirect. Offer it from the web only with a qualified handoff for the same intent and wallet. |

The bot groups CryptoBot and Platega Crypto under **Cryptocurrency**; that heading
is not a seventh method. Its **From purchase balance** action spends existing
funds, not an external top-up or a reason to add another Hosted wallet.

1. **Add a connection.** Choose a server-installed adapter; enter the operator's
   merchant details and environment once. Show only its required fields and the
   installation's callback address where applicable. Save a draft with a vault
   reference. New integrations in a fork supply a server adapter, not arbitrary
   scripts, outbound URLs or rules for marking a payment successful in the UI.
2. **Choose what customers see.** Select supported methods and their display
   order. For each method, the owner chooses who pays the processing fee. With
   `merchant`, the seller absorbs it; with `customer`, the server adds it to the
   payable total and itemizes it before confirmation. Neither reduces the
   requested wallet credit. Allow only choices supported for that method and
   channel. The server quote
   binds the choice, fee, conversion, payable total and wallet credit; customers
   cannot override them. Previewing the list never initiates checkout. If the
   adapter cannot honor the reviewed total, do not open checkout at another price.
3. **Check, then enable.** An explicit check reports a specific missing field,
   unsupported channel or verification result; saving is not activation. HD-02
   qualifies the merchant, environment, method and channel. Use restricted
   qualification for an authorized funding/refund test before public activation.
   Editing a draft leaves the published revision in place.
4. **Change or disable.** Check the expected revision and require fresh owner
   proof for sensitive changes. A disabled method disappears from new checkouts;
   an old unconfirmed quote cannot start one. An already-created intent, late
   callback or refund still uses its original merchant binding, method and
   accepted amounts. Replaying an event cannot credit the wallet twice.

**Top-up amounts.** The owner sets the minimum wallet credit, the ordered quick
amounts and whether to allow a custom amount (on by default). Store these in a
checkout-scoped configuration revision, separate from customer access.
Amounts are USD wallet credit before fees or currency conversion. The local
sandbox accepts whole USD cents; the customer API uses exact microUSD. Quick
amounts must be positive, distinct and at or above the minimum. Fixed-only
checkout requires at least one.

For each method, the server returns only amounts within its qualified limits.
With custom entry on, any supported credit within the effective range is valid;
with it off, only a listed quick amount is valid. Omit methods with no eligible
amounts. Enforce the same rules when quoting and before first confirmation: if
a selection is no longer eligible, require a new choice rather than substituting
an amount. Existing payment intents keep their accepted terms.

**Open invoices.** The default is three per customer wallet across all payment
methods. The owner can set the limit from 1 to 10 in checkout settings. Lowering
it never cancels existing invoices. Quotes use no slots. At confirmation, resolve
any replay or operation-ID conflict first. For a new operation, serialize the
slot check, quote claim and intent insertion per wallet in one database
transaction before calling the merchant. At capacity, return `429 open_invoice_limit`
with the wallet's open count and limit; leave the quote unused and make no merchant call.

Count intents being created, payable invoices and unresolved merchant outcomes.
A slot is freed once payment is verified or there is proof that no payable invoice
remains (including definite non-creation). Use the merchant's expiry terms, not
the browser timer or quote deadline. After a timeout, look up the stored operation
or merchant reference; escalate unresolved cases at the adapter's deadline.
Reopening takes no new slot; a verified late payment still credits the wallet once.

**Balance** lists only enabled, qualified methods for this customer and channel.
With none, new funding is unavailable; existing funds, payment status and receipts
remain accessible. There is no fallback to the operator of another installation.

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
balance read and unconditional decrement. Check the authenticated wallet, workspace
and currency and require `available >= hold >= 0`. A zero hold is valid only when
the complete published tariff and bound prove zero exposure for that request.
Keep its scoped request, reservation and once-only closure; no wallet funds move.
An unknown component cannot qualify as zero. For a positive hold the operation
claim, wallet update, reservation, balanced postings and price snapshot commit
together or all roll back.

Settlement/release conditionally closes the exact held reservation once. For
hold `H` and charge `C`, require `0 <= C <= H`, remove `H` from reserved, and
return `H - C` to available. Release uses `C = 0`. Losing concurrent closers
read the committed result instead of decrementing reserved again.

Use uniform lock order and bounded lock/statement timeouts. Retry whole
transactions for supported transient failures. A lost commit acknowledgement
requires same-operation lookup, never inference resend. All credit/refund/adjustment
and spend-counter writers use this accounting owner. Measure wallet contention;
`READ COMMITTED` does not remove locks or make arbitrary multi-row decisions safe.

Payments follow server intent → signed verified event → idempotent journal
credit → receipt. Bind intents and event deduplication to the configured merchant
and environment as well as provider IDs; sandbox credentials/events cannot post
to a live wallet even if external references coincide. Callbacks cannot choose the
workspace/amount. Duplicate, delayed, refunded, and out-of-order events have
explicit transitions; administrative money changes require actor/reason/reference.

### Money scenarios and worked checks

SC-03 expands the existing journal/reservation rules into customer-visible cases;
it does not select a merchant, real tariff, tax treatment or commercial permission.
All amounts below are synthetic test fixtures in USD, calculated as integer
microUSD. Each independent case starts with available `10.000000`, reserved zero;
the test hold is `2.000000`, not a required minimum balance or a model-price quote.

| Case | Ordered domain actions | Expected available / reserved / net request charge | Customer explanation |
| --- | --- | --- | --- |
| Normal completion | Reserve 2; observe supported final usage priced at 1.2; close once | `8.800000 / 0.000000 / 1.200000` | Two dollars were held; actual cost was 1.2; unused 0.8 became available |
| Proven zero exposure (instead of the 2 USD hold) | All applicable rates are explicitly zero; scope, finite bounds and physical capacity still pass. Close the zero reservation once. | `10.000000 / 0.000000 / 0.000000` | The request and its measured tokens are visible with a confirmed zero charge. With a required rate missing instead, reject before send. |
| Proven not dispatched | Reserve 2; cancel before any dispatch; commit release | `10.000000 / 0.000000 / 0.000000` | No generation was sent; release is shown only after commit |
| Disconnected, outcome unknown | Reserve 2; disconnect after possible execution; await evidence/deadline | `8.000000 / 2.000000 / pending` | The hold is not a final charge; disconnection alone proves neither success nor free use |
| Partial usage at deadline | No final usage; valid cumulative input prices to 0.4. Visible output alone would yield a 0.6 estimate. Settle the evidenced 0.4 only. | `9.600000 / 0.000000 / 0.400000` | Partial, not estimated or final; unmeasured output is unknown, not billed again. |
| Waiver at deadline | Previous uncertain case has no substantive forwarded output and no usable usage; apply launch waiver | `10.000000 / 0.000000 / 0.000000` | Reservation released by policy; provider expense may still be unknown/nonzero |
| Bounded estimate at deadline | Previous uncertain case has substantive output, no usable provider usage, and its saved estimator yields 0.6; settle | `9.400000 / 0.000000 / 0.600000` | Charge is explicitly estimated under the saved policy, not labelled exact |
| Late lower usage | After the 0.6 estimate, authoritative charge under the original tariff is 0.4; credit 0.2 once | `9.600000 / 0.000000 / 0.400000` | Original charge remains visible together with a separate correction |
| Late higher usage | Alternative to the previous row: after the 0.6 estimate, later priced evidence is 0.8 | `9.400000 / 0.000000 / 0.600000` | No additional customer debit; 0.2 is operator exposure, not hidden debt |
| Bound violated before settlement | With a hold of 2, supported final usage at the saved tariff prices to 2.3 | `8.000000 / 0.000000 / 2.000000` | Customer debit stops at the hold. Preserve the uncapped valuation and 0.3 discrepancy separately from actual provider expense; fence the affected paid route/variant pending sizing and tariff requalification. |

For each closure assert `available_after = available_before + hold - charge`,
`reserved_after = reserved_before - hold`, `0 <= charge <= hold`, balanced postings
and one operation identity. For the normal case, `8 + 2 - 1.2 = 8.8`; the initial
10 equals available 8.8 plus net charge 1.2. Repeat each closure/correction and drop
its first acknowledgement: all amounts must remain unchanged on recovery.

Payment steps are similarly explicit:

1. Validate the caller, funding policy, accepted quote and unique operation. Persist
   an intent before contacting the payment provider; no credit yet.
2. Replaying the same operation and quote returns the existing intent. Reusing
   its `operation_id` with a different quote, method, amount or currency is a
   conflict. To use another method while an invoice is open, the customer
   reviews a fresh quote and confirms it with a new `operation_id`. The second
   invoice does not cancel, replace or extend the first.
   Closing the checkout tab does not cancel the invoice. From payment history,
   the customer can reopen the same invoice while the original adapter confirms
   it is unpaid, still payable and resumable on this channel. The invoice's
   expiry governs this action, not the expiry of its already accepted quote.
3. Verify the provider event and match merchant/reference/amount/currency to the
   original intent. Unverified redirect, message or screenshot remains no evidence.
4. Post one balanced journal transaction per verified paid invoice and link its
   receipt. Equal amounts in the same wallet do not make two payments duplicates.
   A synthetic 5 USD top-up changes 10 to 15 once; three deliveries of that
   verified effect still leave 15.
5. Resolve delayed/unknown payment state by querying the same operation/provider
   reference. Notification failure never reverses a confirmed credit.

For a voluntary refund, the customer's request in Balance is not an accounting
event. On owner approval, the wallet lock orders the original payment's refundable
remainder and available-balance checks with inference admission. The journal moves
the amount from available to a distinct refund reserve. A private worker durably
claims the same operation before calling the original merchant. On confirmed
success the reserve moves to payment clearing once; on proven failure it moves back
to available. An unknown result keeps the reserve until lookup of that same
operation proves the outcome. Provider-specific escalation is HD-02, not the
inference-waiver deadline. The local implementation uses credited sandbox USD
principal only, in whole cents; fees and imposed reversals are outside that path.

For an approved synthetic 2 USD refund from a 10 USD wallet: reservation leaves
available 8 and refund-reserved 2; confirmed success leaves available 8 and reserved
zero with 2 returned externally; proven failure restores available 10; unknown
outcome keeps available 8/reserved 2 pending reconciliation. A duplicate success
cannot return another 2. The public reserved total includes inference and refund
holds, with separate breakdowns; a refund is wallet activity, not negative model
usage or provider expense.

An externally imposed reversal after spending is not an ordinary voluntary refund.
For example, a 10 USD funding event followed by 6 USD spend leaves 4 USD, not enough
to absorb a 10 USD reversal. Show restricted spending and the unresolved obligation;
do not write `-6` into the nonnegative spendable wallet, delete the 6 USD usage or
invent that the customer legally owes a recoverable debt. HD-04/09 must approve the
separate accounting representation and customer terms; absent that decision, paid
launch is blocked. In-progress inference holds keep their own closure rules.

## Usage and operator views

Store tenant/key-scoped logical outcomes and separate physical attempts with
route revision, execution certainty, usage, expense, retry reason, and risk.
Never sum the logical charge once per attempt. A shared key identifies a key
and workspace, not the human who used it; individual attribution needs assigned keys.

Store a sanitized, length-bounded User-Agent label (at most 256 characters) as
untrusted client metadata. Strip control characters and redact known credential
patterns; never use it for authentication, pricing, authorization or proof of a
person/application. Keep the original authenticated workspace/key/user linkage
separate and snapshot a safe display label so later renames do not alter ownership.
Do not log all request headers to obtain this field. Full IP addresses are not
part of normal usage; short-lived abuse controls have separate restricted storage.

User views are tenant-scoped. Privileged changes/reads are audited. Operational
logs, audit, usage, expense, and journal have separate access/retention. No raw
prompts, responses, cookies, authorization, or provider bodies by default; authorized
continuation content is the separate policy above. Metrics labels are bounded,
not key/user/prompt/arbitrary-input labels. Deletion retains required financial
records while removing eligible personal data.

### Errors and production diagnostics

Diagnostics ship in the first release through role-scoped interfaces.
A user should understand what happened, whether output or
money is affected, what is known versus uncertain, and the next safe action.
An owner should follow the same request across admission, attempts, execution and
settlement without reading customer content. No new log-search/incident platform
is required: use the existing core observations, Hosted durable records, bounded
structured logs and the documented monitoring stack.

#### Diagnostic audiences and navigation

| Audience | Browser surface | Information and authority |
| --- | --- | --- |
| Customer | `/app/usage`, own request detail | Safe explanation, stable code, request ID, observed outcome, usage completeness, reserve/charge status and safe next action; own workspace only |
| Installation owner | `/admin/usage?tab=errors`, detail at `/admin/usage/requests/{request_id}` | Authorized customer/key attribution, model/source, stage, attempt timeline, safe technical evidence and separate accounting state |
| Installation owner | `/admin/operations` | Background jobs, refresh/payment/settlement/backup/storage incidents, alert state, diagnostics controls and runbooks |

The owner Errors tab remains inside Usage, alongside the existing request/model/
pool-member views. Do not add an eighth customer navigation section or expose the
operator table to customers. Background incidents do not become fake inference
requests. Paired Desktop clients may read only safe request/error detail within
their [Usage scope](#desktop-hosted-scope). Extended capture and its session
administration remain web-owner-only; starting capture requires fresh passkey
verification, not a broader Desktop grant.

#### Normalized diagnostic record and evidence

Keep a versioned allowlisted diagnostic DTO, shared by server projections and typed
web/desktop clients. Reuse core error codes and `ErrorOrigin`; do not reclassify
origin from an English message, a code prefix or an HTTP status. Hosted operational
failures also identify their emitting component. Source attribution is not proof
of root cause: show "cause not established" when evidence is insufficient.

| Field/group | Meaning and constraint |
| --- | --- |
| `diagnostic_version`, `event_id` | Schema version and server-generated event identity; duplicate event delivery does not create another occurrence |
| `occurred_at`, `observed_at`, `sequence` | UTC event/collection times and source-local ordering; distinguish delayed observation, do not invent unrecorded stages |
| `workspace_id?`, `request_id?`, `attempt_id?`, `job_id?`, `correlation_id` | Server-established links; a request event needs its workspace/request, an operations event needs its real job/component context |
| `component`, `stage` | Emitter and observed execution phase, independently from alleged cause |
| `origin?`, `code`, `category` | Core `provider`/`account`/`relay` origin where observed, stable code and UI category; absent origin is unknown, not a new core enum variant or an inferred account fault |
| `severity` | `info`, `warning`, `error`, `critical`; operational importance, not a replacement for execution outcome or root-cause evidence |
| `summary_key`, `safe_parameters` | Locale dictionary key and bounded allowlisted parameters; no arbitrary provider/body interpolation |
| `cause_status`, `evidence` | `confirmed` or `unknown` cause with typed safe supporting observations; missing evidence stays missing |
| `execution_certainty?`, `execution_state?` | Request-only projection of sending/execution evidence and canonical logical outcome; not applicable to an unrelated background event |
| `accounting_state?`, `settlement_basis?`, `usage_completeness?` | Request-only projection from the accounting/usage owners; pending is not zero, failed execution does not imply refund |
| `next_action`, `retry_after_seconds?` | Evidence-derived action, not a guess that every transient-looking error is safe to resend |
| `coverage`, `expires_at` | Complete/partial/unavailable retained detail and expiry; missing diagnostics cannot render as an empty healthy system |

Diagnostic stage names are `authentication`, `admission`, `queue`, `reservation`, `dispatch`,
`upstream_read`, `protocol_conversion`, `client_delivery`, `continuation`,
`checkpoint`, `settlement`, `payment`, `credential_refresh`, `backup`, `retention`, `storage`,
`configuration`, or `unknown`. H01/H12 map actual observation points to these
names; a stage is recorded where observed, not inferred from the final error alone.
Stable categories are `authentication`, `policy`, `capacity`, `quota`, `timeout`,
`transport`, `protocol`, `cancellation`, `accounting`, `payment`, `storage`,
`worker`, `configuration`, or `unknown`.

Owner-only evidence may include safe HTTP statuses, protocol/adapter version,
bounded allowlisted structural field/type/offset information, route revision and
validated upstream correlation ID. Keep source/key references as internal IDs or
masked labels. No raw body, tool arguments, arbitrary JSON field names, full URL
query, auth assertion, cookie, secret, or free-form exception dump. Redaction after
capturing everything is not the data-collection policy.

For each code/category, H12 fixes a message key, severity rule, evidence predicate,
customer-safe projection and next action. The emitting component owns the code;
the UI owns translation. An unfamiliar code displays "Unclassified error" with a
safe reference.

#### Owner errors list and request detail

Default to the same seven-day UTC reporting window as Usage. The Errors tab shows
one row per logical request with a known failed/incomplete outcome, unresolved
outcome, or an accounting problem requiring attention. A completed request with
failed settlement remains visible, with its successful execution clearly separate.
Normal confirmed cancellation and recovered attempt failures are separate optional
filters, not automatically red system failures. Historical failures are not erased
when later evidence resolves uncertainty; the current state and history differ.

The table has time, public model, source, customer/key, stage, short explanation,
severity, execution outcome, accounting status, attempt count and request ID.
On narrow screens, keep time/explanation/outcome and expose other fields in the
detail drawer rather than hiding the problem or requiring horizontal-only controls.
Filters cover period, model, source, customer/key, origin, category/code, stage,
severity, execution/accounting state and request ID. Search/filter values are
bounded selectors, not SQL, regex execution or an authority override.

Separate summary counters for failed, incomplete, uncertain, confirmed cancelled,
accounting-problem requests and failed physical attempts. Explain that the counters
may overlap: do not sum them into a fictitious number of failed requests. If showing
a failed-request rate, name it and use `failed / (completed + incomplete + failed)`;
exclude active, uncertain and confirmed cancelled work, disclose the denominator
and show unavailable when empty. Keep failed-request, completion and internal
service-defect rates distinct.

Request detail answers these questions in order:

1. **What happened?** Short localized explanation, stable code and request ID;
   current execution/accounting outcome and evidence completeness are prominent.
2. **Where and when?** Observed timeline from authentication/admission through queue,
   reservation, each attempt, response delivery, checkpoint and settlement. Show
   duration only where observed; mark missing/expired stages, clock gaps and delayed
   observations. A copied timestamp carries its timezone.
3. **What did the customer receive?** Whether substantive output was forwarded,
   known/partial/missing usage and interruption certainty. Socket writes do not prove
   human receipt. No prompt/answer preview is needed to explain the transport result.
4. **What happened to money?** Hold, posted charge, release/compensating credit and
   pending reconciliation, from the journal/reservation projection with currency
   and precision. A diagnostic event cannot claim a refund before it is committed.
5. **What can be done safely?** Status refresh, runbook, configured recovery action,
   or safe support summary. No generic "Replay request" button and no provider probe
   caused by opening/refreshing a detail page.

Physical attempts are nested under their logical request and explain why another
attempt was permitted, with available non-execution evidence. A recovered attempt
failure does not change a successfully completed request to failed. Viewing a
problem, acknowledging an alert or copying a summary never retries execution,
changes a tariff, releases money or changes pool membership.

#### Customer wording and safe next actions

The customer sees its own request's stable public code/ID, short explanation,
known execution/charge state and next action. Omit upstream identities, physical
route/attempt diagnostics, internal exception details and other customers. Unknown
financial/execution data has an explicit pending/unavailable state, not a reassuring
but unsupported "no charge" message.

| Observed situation | Example English summary | Required explanation/action |
| --- | --- | --- |
| Client connection closed, completion unknown | "Connection closed; request outcome is being checked" | Show uncertainty and real hold status; refresh status rather than suggest blind resend |
| Invalid upstream response shape | "Could not process the model response" | Preserve stable adapter code; owner gets safe stage/shape evidence, not a fabricated cause or raw body |
| Capacity unavailable before dispatch | "No capacity is currently available" | Retry guidance only with proven non-dispatch; show release only after committed closure |
| Upstream credential requires renewal | "The model is temporarily unavailable" | Owner receives connection-specific renewal guidance; customer sees no account identity |
| Completed output with pending settlement | "Response completed; charge is being reconciled" | Distinguish completed execution from pending money; do not offer another generation as repair |

`next_action` is one of `retry_when_allowed`, `wait_for_capacity`, `check_status`,
`fix_request`, `sign_in_again`, `owner_recovery`, `contact_support`, or `none`.
H12 defines the evidence/permission predicate for each. `retry_when_allowed`
requires proven pre-execution rejection/non-send and current policy/grants; an
HTTP 429/5xx or retry-after value alone is insufficient. The existing control API's
`retryable` flag must agree conservatively with this contract. Inference duplicate
responses retain their existing no-reexecution/idempotency policy.

Offer "Copy support summary" as an explicit action: schema/build version, time,
safe code, request/correlation ID, permitted state fields and coverage. Do not
include prompts, responses, full keys, account email, unrelated events or current
browser URLs/query strings. Never upload a support bundle automatically. Localized
labels and accessible text convey outcomes without relying on red/green alone.

#### Browser failures and unavailable services

The web shell and feature boundaries must render a safe fallback for an unexpected
render failure instead of a blank page. Distinguish a failed reporting read, session
expiry, permission denial, incompatible API version and unreachable backend. A
connection failure stays unclassified until its network, proxy or server cause
is confirmed. Classify deliberate navigation/request aborts separately from incidents.

Keep the last successful snapshot visibly stale with its time, or show unavailable;
never replace failed usage/balance reads with zeros. Retrying a safe read is separate
from submitting payment, key creation, refund or inference again. After a mutation
loses its response, look up the existing operation/status according to its domain
contract; reloading an error boundary cannot repeat the mutation.

Browser support summaries contain only frontend build/API version, time, a stable
local error code, route template and the last relevant server request ID when
available. A browser-only reference is labelled local and cannot masquerade as a
server request. Exclude query strings, DOM/form contents, arbitrary exception text,
stack traces and client-storage dumps. Keep production source maps in restricted
build artifacts, not public static assets. Do not install a third-party session
replay/error collector or an unrestricted browser-log upload endpoint for launch.
Browser-only failures reach support through explicit copy/share; backend diagnostics
cannot claim to have observed a failure recorded only in the browser.

#### Operations, alerts and temporary diagnostics

Use `/admin/operations` for refresh, payment, reconciliation, worker, database,
storage and backup failures, with last occurrence, component, safe code, count,
impact, current condition and runbook. Group repeated notifications by stable
component/category/source scope, not raw message text or a customer's prompt.
Keep request failures distinct from background failures, even when linked by a
correlation ID. Pre-authentication failures are bounded security/operations events;
an unverified request field cannot assign them to a customer.

Default cancellation produces no service-outage alert. Recovered retry noise and
repeated quota failures are grouped; sustained rates or latency can raise a warning.
Stuck settlement/holds, unavailable durable stores, failed refresh across usable
capacity and missing backup delivery follow explicit threshold/runbook rules.
Define severity, observation window, threshold, recovery condition, cooldown and
notification destination before enabling each alert. Acknowledgement records
operator receipt; resolution requires the defined recovery condition.

Normal production diagnostics are always safe and bounded. Extended diagnostics
add only approved timing/state/structural metadata, not customer content. They do
not require a debug binary or a public developer console. Starting capture requires
an owner session, CSRF protection, fresh passkey verification, an audited reason and
one explicit request/job scope or allowlisted component/source combination.
No arbitrary log query, filesystem read, all-headers mode or unrestricted global
capture is exposed through the API.

Proposed launch defaults: 10-minute duration, 30-minute hard maximum and one active
extended-capture session per installation. Event/byte/rate budgets are finite
required deployment settings qualified in H16; an unset budget disables capture.
Server-enforced expiry/budget exhaustion, owner revocation and explicit stop end
capture. Repeated calls cannot extend its original deadline. After restart, restore
only an authorized unexpired session with recoverable budget counters; otherwise
stop it. Every start/stop/expiry has an audit record without captured payloads.
Show scope, remaining time and consumed budget while active.

Do not expose stack traces, SQL, raw exceptions or backend file paths to customers.
Do not serialize request/config structures with generic debug formatting. Required
settlement evidence and privileged-action audit remain durable domain obligations;
optional diagnostic detail is not a second billing journal. A failed diagnostic
sink or full optional buffer must not generate another request or lose a charge:
mark coverage incomplete, bound/drop optional detail, and raise a safe health signal.
Keep collecting minimal mandatory evidence or apply the existing fail-closed rule
for the affected domain. Prevent recursive "logging failed" event storms.

### Initial data policy

These are proposed launch defaults, not existing Desktop settings.
The owner can shorten eligible retention; extending it requires a documented
purpose, customer notice and bounded storage. Lifecycle cleanup is a monitored,
idempotent job; deleting an account cannot bypass financial reconciliation.

| Data | Default and access |
| --- | --- |
| Prompts, answers, raw provider bodies and auth headers | No operational logging or response-replay archive |
| Request/attempt detail, safe errors and User-Agent label | 30 days; own workspace for customers, audited operator access |
| Optional request-linked `diagnostic_events` | At most the request-detail 30-day window; owner-only technical evidence, customer projection allowlisted separately |
| Daily token/charge aggregates without User-Agent or bodies | 365 days for customer reports; journal remains authoritative |
| Model health observations | Retain the bounded 7-day status window, including evidence still valid at its start. Expose only scoped aggregate intervals; expiring history never changes route authorization. |
| Customer notices / terminal notification outbox and delivery detail | 30 days for scoped in-app notices, 14 days after terminal delivery/outbox state; never extend owning data expiry or retain raw messages/recipient secrets |
| Readiness observations | 30 days of safe check evidence; current validity still depends on revision/expiry, privileged activation audit follows its own policy |
| Operational logs and non-request `diagnostic_events` | 14 days, redacted, operator-only; no customer-visible installation event feed |
| Extended-capture events and `diagnostic_sessions` | Events keep the applicable request/operations expiry, never reset by capture; session metadata 14 days after closure, privileged audit 180 days |
| Security and privileged-action audit | 180 days; restricted, append-only access; never store login assertions or tokens |
| Optional encrypted continuation payload | Off unless a supported route explicitly needs and permits it; then 24-hour TTL, 8 MiB per session and 256 MiB per workspace, with a separately configured global cap; reject unsupported resume rather than retaining more silently |
| Provider credentials and identity-provider client secrets | While explicitly registered; encrypted vault; removal revokes/deletes live references and fences pending work |
| Journal, payments, refunds and settlement evidence | No automatic purge in the initial implementation; set a jurisdiction-reviewed archival/deletion schedule before paid launch, separate from optional usage detail |
| Encrypted backups | Daily snapshots retained 30 days; owner-only recovery keys held separately; deletion/expiry and revocation reapplied before restored traffic |

Continuation metadata expires with its turn/session; financial references may
survive without content. Use explicit deletion/tombstone records so a restore
cannot revive removed identities, credentials or retained payloads. Backups age
out rather than promising immediate physical erasure from every prior snapshot.
Notify customers of these boundaries. No raw request capture is enabled for
support; credential exports require a separate explicit privileged action.

Capture expiry stops collection; it does not promise instant deletion of already
permitted evidence. Compute each event's expiry at write time from its class and
the earlier owning-detail expiry where applicable. Linking an event to a request
does not make otherwise forbidden operational/account data customer-visible.
Expired optional detail is deleted independently of journal, reconciliation and
audit obligations; those domains retain their own minimal required evidence, not
an indefinite duplicate diagnostic payload. Cleanup and restore reapply the same
cutoffs. Session metadata expiry cannot erase the required start/stop audit trail.

Initial session limits: customer sessions expire after 7 days absolute or 24 hours
idle; administrator sessions after 12 hours absolute or 30 minutes idle. Sensitive
owner actions require verification within 5 minutes. Revoke sessions/keys on
security recovery and enforce limits server-side; provider policy may be stricter.
Idempotency claims retain canonical digests, not bodies, for at least 7 days after
terminal settlement and longer while execution/reconciliation remains open.

## Local and hosted pool transfer

Use a revisioned preview/apply/recovery operation, not implicit synchronization.
Manifests contain portable settings, route declarations, rules, and credential
references, not user wallets, usage, keys, sessions, or conversations.

Static API sources default to `copy`. OAuth defaults to `stage_and_drain`:
atomically drain/disable the local refresh owner before transferring its secret.
Secrets move only after confirmation over HTTPS directly into the destination
vault. Unsupported transfers require destination sign-in.

Transfer support is declared per connector/issuing client, not inferred from a
ChatGPT-shaped token. Preview names the destination, selected connections, supported
settings and proxy dependencies, omitted/nonportable fields and required destination
sign-in. Preserve applicable model rules and rotation settings through core
validation; map local IDs explicitly. Preview conflicts with existing server rules,
catalog order or tariffs rather than overwriting unrelated configuration. A local
reference price or purchase cost does not become a published server retail tariff.

Validate/apply the target revision and return per-item outcomes. Failure before
apply restores the source only after confirming no destination owner was activated.
An uncertain apply response is reconciled by operation ID before resuming refresh
or retrying transfer; never create two active OAuth refresh owners. Source recovery
uses the latest verified credential generation, not a pre-transfer token snapshot.
Success keeps the source drained until an explicit completion/restore. Delete the source only
after operator verification and confirmation. Reverse transfer follows the same
rules. No newer revision or opaque conversation state is overwritten/transferred.

### Preserve the effective proxy during transfer

A confirmed local-to-server transfer of an account or API-provider connection
includes its effective proxy and the connection-to-proxy binding, not just the
provider credentials. Transfer the selected saved proxy's configuration and,
when required, its authentication into the destination vault. After transfer,
server inference, discovery, quota checks and credential renewal continue to use
that proxy under the existing shared routing rules; Desktop is not a forwarding
hop and does not need to stay open.

- An account-specific or API-source-specific proxy remains explicitly bound to
  that connection on the server. Remap local proxy IDs to destination IDs; do not
  interpret a local ID as an existing server proxy.
- Resolve the route using the connection type's runtime policy. A Desktop API
  source uses its assigned proxy or direct; the account common proxy is not an
  API-source fallback.
- If the connection inherits the local common proxy, include that effective proxy
  and create an explicit destination binding for the transferred connection.
  Do not overwrite the server's common proxy or move unrelated local bindings.
  An explicit direct/bypass-common selection remains direct rather than silently
  inheriting the destination's common proxy.
- A proxy shared by several selected accounts/API sources is transferred once
  with all selected bindings preserved. Reuse only a matching validated proxy
  identity; the same host/port with different authentication is not sufficient
  evidence that two proxies are interchangeable.

The revisioned preview and confirmation identify the proxy dependencies and the
effective route without revealing authentication. Proxy secrets use the same
confirmed HTTPS-to-vault path as provider secrets, never renderer snapshots,
ordinary configuration manifests, logs, audit payloads or diagnostics. Persist
the destination proxy and its bindings so server restarts do not change the route.
Local proxy configuration remains intact under the source-preservation rules.

Stage the connection and proxy dependencies together. A missing secret, unresolved
binding, unsupported proxy or destination network-policy/reachability failure
keeps the affected connection out of the runnable pool; never fall back silently
to direct traffic or a different server proxy. A loopback, LAN or Desktop-local
proxy address must be evaluated from the server, not assumed to remain reachable.
Changing that route requires a new explicit preview/confirmation. Destination
verification obtains its own observations; local latency, speed, location or
successful-check results do not prove that the server can use the proxy.

## Data and deployment

PostgreSQL owns identities/workspaces, grants/keys, pool/route revisions,
wallet/journal/reservation/payment records, request/attempt/audit/import records,
and metadata for offered continuations. Bootstrap establishes tenant-scoped
references; each domain adds its own migrations (for example reservation
closure uniqueness in accounting, turn versions/expiry before enabling resume).
The [implementation map](#implementation-contract-map) names planned records and
their owners; domain packages must still finish the full DDL/constraint dictionary.

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

### Recommended first-production stack

Recommended deployment baseline: one region and one supported Linux host with
Docker Engine and Compose, persistent storage, off-host backups and an independent
availability check. Prioritize a complete, supportable paid product and Relay reuse.
Plan for single-host outages, maintenance windows and recovery.

| Layer | Recommended choice | Responsibility and boundary |
| --- | --- | --- |
| Shared model engine | Existing Rust `relay-core` | Reuse discovery, protocols, routing, scheduling, quotas and usage semantics; no parallel Hosted codec engine. |
| Hosted application | Rust, Axum and Tokio; one modular backend with bounded workers | Own customer identity, management, admission, accounting and jobs. Keep Hosted composition separate from Desktop; do not make Desktop a required hop. |
| Durable database | PostgreSQL | Authoritative identities, grants, tariffs, wallets, journal, reservations, payments, durable jobs and usage; never replace this authority with a cache. |
| Rust database access | SQLx with PostgreSQL, Tokio and TLS support | Explicit SQL, bounded connection pools, transactions and append-only migrations. This is a new Hosted dependency, not an existing SQLite migration. |
| Default customer website | React, TypeScript and Vite in `zenith-relay/hosted/web/` | Separately built/deployed, configurable landing page/cabinet in the same repository, using the public Hosted customer API; replaceable by an operator's own site. No Tauri or backend dependency in the browser. |
| Public edge | Caddy | HTTPS, compiled frontend assets and reverse proxy to registered API routes; qualify SSE, WebSocket, cancellation and reload behavior. |
| Secrets | Relay's encrypted vault design | Protect upstream and identity-provider secrets; use versioned references and keep encryption/recovery keys outside PostgreSQL and images. No separate HashiCorp Vault service initially. |
| Background processing | Bounded Tokio workers with PostgreSQL-backed job state | Renewal, payment reconciliation, settlement and retention; durable ownership and deduplication, not memory-only fire-and-forget jobs. |
| Database recovery | pgBackRest with WAL archiving to an encrypted off-host S3-compatible repository | Database backups and point-in-time recovery; vault/configuration/recovery-key handling remains a separate coordinated procedure. |
| Observability | Prometheus, Grafana and Alertmanager | Private metrics, dashboards and actionable alerts. Export safe application/host/database metrics; financial truth remains in PostgreSQL. |
| Logs | Redacted structured application logs with bounded rotation/retention | Start without a separate log-search cluster. Never capture request/response bodies, credentials or login assertions. |

Existing foundations are Rust/Axum/Tokio, `relay-core`, React/TypeScript/Vite and
the Desktop encrypted vault. Hosted SQLx/PostgreSQL persistence, browser transport,
customer/billing modules, deployment packaging and the operational integrations
above must be implemented and qualified. Desktop SQLite is not Hosted persistence.

Use supported stable component releases and pin tested versions/digests together;
do not select release candidates or `latest` automatically. Rust/Cargo, Bun and
Playwright are development/build/test tools. Build static web assets in CI or a
build stage; the production request path needs neither a Bun/Node server nor a
running Vite development server. Initially pin separately built backend/frontend
artifacts from the same tested Relay release; qualify compatibility for later
independent updates. The backend also runs without the default web artifact.
Application ownership and both integration paths are defined in
[the web distribution contract](#one-repository-and-deployment-choice).

### Deployment topology and external dependencies

```text
Browser / API client / Desktop operator
                   |
              Caddy (HTTPS)
              |           |
        default/custom UI Hosted Rust backend + relay-core + bounded workers
                          |         |          |
                    PostgreSQL   vault     approved upstream providers

Private operations: Prometheus + Grafana + Alertmanager + backup jobs
Outside this host: encrypted backup repository, recovery-key copy, uptime check
External integrations: configured login providers and selected payment provider
```

Only the HTTPS edge is publicly reachable; PostgreSQL, backend listeners, metrics,
dashboards and vault storage stay private. Reach operational dashboards through
an authenticated private operator channel. Route registered API paths before the
frontend fallback; an API error must never become an HTML application page.
Serve the cabinet and management UI from the configured origin, with independent
server-side privileges; a shared origin does not make customer sessions administrative.

Compose manages versioned services and explicitly owned persistent volumes, with
resource limits, restart policy, health checks and bounded logs. Run application
containers without root privileges or a Docker socket. Deliver secrets through
restricted runtime files or host credential injection, never source, images,
command-line arguments or frontend configuration. Compose secret mounts alone
are not encrypted-at-rest storage or a replacement for vault-key recovery.

DNS and the approved hosting region, Google/Telegram/OIDC application registration,
payment merchant credentials, backup storage and an alert destination are operator
prerequisites. Customer login and payments are adapters in the same backend, not
additional customer databases or wallets. Use maintained OIDC/WebAuthn libraries;
do not implement signature verification or authentication protocols from scratch.
No separate Keycloak installation is required: a self-managed identity service can
be connected through the configured OIDC contract. Choose the payment provider for
the actual merchant/region; no particular processor is assumed available.

### Persistence, workers and recovery

Keep SQLx transactions short; never hold wallet row locks or database connections
for the lifetime of an upstream stream. Reserve/commit ownership, release the
transaction, execute, then settle through the durable request state. Database
constraints and transaction logic enforce tenant scope, unique payment events,
and one reservation closure; an in-process mutex is not a financial guarantee.
Apply migrations through one controlled runner with database-level exclusion,
not competing web/worker startup migrations.

Jobs have bounded attempts, backoff, ownership leases and terminal/uncertain
states. A lease expiring after dispatch does not authorize another generation:
reconcile external effects first. Persist payment event identity and processing
state so duplicate delivery or lost acknowledgements cannot duplicate credit.
Restart recovery must preserve the existing no-unsafe-retry and settlement rules.

pgBackRest protects PostgreSQL, not arbitrary vault files or external keys. Record
compatible database/vault restore points, durably archive vault changes and back
up deployment configuration without embedding plaintext secrets. Keep the recovery
key independently protected and available after total host loss. Monitor completed
off-host WAL/archive delivery, not just successful job startup. Follow the retention,
RPO/RTO and isolated-restore gates in the [operations contract](#initial-operations-contract).

Keep metrics labels bounded and non-identifying. Use existing redacted request
identifiers for authorized diagnostics, not raw prompts or User-Agent metric labels.
Monitor whole-host availability from outside the host; colocated monitoring cannot
report its own complete outage. Alerting and backups are first-release requirements,
not optional work after paid customers arrive.

### From an empty host to paid service

Implementation and acceptance requirements:

1. Qualify the region, upstream commercial permissions, initial model tariffs and
   payment merchant; provision DNS, off-host backup storage and operator recovery.
2. Select tested, pinned Hosted backend and compatible default web artifacts, or
   qualify the operator's custom frontend. Supply Compose/static deployment recipes,
   migrations, secret-input documentation and independent web update/rollback steps.
3. Prepare private networking, persistent volumes and secrets; initialize PostgreSQL
   and the vault, apply migrations, then provision the owner through local setup.
4. Configure HTTPS/origins, passkeys, customer login providers, signed payment
   notifications, allowed outbound integrations and bounded admission policies.
5. Register approved accounts/API sources, enable qualified model routes and prices;
   check inventory parking, refresh ownership and actual active-request reporting.
6. Enable metrics, external outage detection, alerts, retention and coordinated
   off-host backups. Restore into an isolated environment and measure recovery.
7. Enter restricted `qualification` for approved identities/routes. Exercise
   sign-in, top-up, key issuance, real inference and correct settlement;
   test tenant isolation, concurrent spending, duplicate callbacks, disconnects,
   restart/uncertain execution and upgrade recovery under the qualified workload.
8. Enable paid traffic only after the [delivery gates](#delivery-and-acceptance) pass.
   Container health, a successful backup command or a model list alone is insufficient.

### Owner setup and readiness

SC-01 is exposed as an owner-only checklist in `/admin/operations/setup`. Each row
shows what is missing, why it matters, an allowed next action and observed evidence;
no technical knowledge of database tables is needed to understand the result.

| Step | Owner action / implementation effect | Completion evidence and safe failure behavior |
| --- | --- | --- |
| 1. Installation | Install the pinned artifact pair; validate config, private storage and migrations | Correct schema/artifact pair and required stores usable; missing/invalid secrets fail startup, not a public setup wizard |
| 2. Ownership | Complete the server-local owner ceremony and register recovery access | Verified owner/passkey and documented lost-access path; first public sign-in cannot claim ownership |
| 3. Public entry | Set HTTPS origin, trusted edge and enabled customer login methods | Correct origins and tested callbacks/cookies; no wildcard-origin workaround or exposed private ports |
| 4. Capacity | Register an approved source, park/enable it and qualify a public model | Exact eligible source/model/protocol and current authorization; unknown availability is not a green check |
| 5. Money | Publish complete tariffs and configure at least one method in the [payment builder](#payment-method-builder) | Approved pricing/rounding/reversal policy and verified merchant binding; no default invented prices or fake payment success |
| 6. Protection | Set measured limits, data policy, recovery secrets and outbound rules | Finite enforced values and reviewed access/privacy rules; mandatory unknowns remain blocking |
| 7. Recovery and signals | Configure external alerts and off-host backup/key recovery; run an isolated restore | Delivered alert and verified restore evidence for compatible versions, not merely a scheduled backup job |
| 8. Qualification | Run the SC-02/03 journey with explicit test authorization and the real accounting paths | Correct identity, funding, key, request, settlement and safe failure results; a paid probe is clearly labelled and bounded |
| 9. Public opening | Review current evidence and explicitly enable public service | Atomic revision-checked transition to `live`; any changed/expired prerequisite blocks activation with a specific reason |

Readiness items have `check_id`, `status`, `required`, `scope`, `config_revision`,
`artifact_revision`, `checked_at?`, `valid_until?`, `evidence_ref?`, `reason_code?`
and `next_action`. Status is `not_configured`, `pending`, `passed`, `failed`,
`expired` or `not_applicable`; a required item cannot be hidden as not applicable.
Separate automated observations, explicitly authorized real tests and reviewed
operator decisions. A checkbox cannot prove commercial permission, restore success
or an actual merchant integration. H16 defines the dependency/invalidation map;
changing credentials, origins, schema, tariffs or artifacts invalidates affected
evidence rather than carrying a stale green badge forward.

Opening or refreshing this page performs reads only. Active checks are named
allowlisted operations with explicit authority, scope, deadline and operation ID.
Checks that send messages, move money or run inference require separate informed
confirmation and the same domain limits/ledger as ordinary activity. Do not store
synthetic payments or usage in real customer reports. A test payment/reference is
marked as qualification activity without exempting its real economic effect.

### Service modes and public condition

One installation-level `service_control` record owns intended service mode and
revision. Every backend admission path combines effective mode, workspace/key/model
policy and runtime health; frontend controls reflect that decision.

| Mode | New registration / funding / inference | Existing work and customer access |
| --- | --- | --- |
| `setup` | Closed to public activity; only owner provisioning and non-paid configuration checks | Owner can correct setup; no customer money is manufactured for demos |
| `qualification` | Only explicitly allowlisted verified test identities/workspaces, with normal auth, money and route limits | Authorized integration tests and their real settlement/reconciliation; no customer/admin role shortcut |
| `live` | Open subject to all ordinary eligibility, capacity and money checks | Normal usage, history, billing, support and recovery |
| `maintenance` | No new registration, payment intents or inference dispatch | Drain/reconcile existing work; authenticated history/status and approved refund handling remain available when their stores are healthy |
| `closing` | No new sales, registration, keys or inference dispatch | Resolve current work, return eligible unused funds, provide receipts/export and controlled support |
| `closed` | Sales and inference remain disabled | Continue retention, reconciliation and published support/archive access; preserve records and process late payment events |

Allowed paths are `setup -> qualification -> live`, `qualification/live ->
maintenance`, `maintenance -> qualification/live` after revalidation, and
`qualification/live/maintenance -> closing -> closed`. Closure is not a normal
pause/resume operation. Reopening a closed business is a separately reviewed
migration, not a hidden toggle. In qualification mode, identity allowlisting uses
verified provider subject bindings, not display names, email equality or a public
query flag. It cannot relax production authentication, pricing or isolation checks.

Mode changes require owner browser authority, CSRF, fresh passkey, expected revision,
durable operation ID, reason and audit. Commit mode/admission fencing together;
queued not-yet-dispatched work rechecks the new fence and releases safely if blocked.
The cutoff orders durable dispatch claims, not remote packet arrival: a claim
committed before the cutoff belongs to draining work even when sending is uncertain.
Do not call that work undispatched or release its hold without the required evidence.
Already dispatched work retains its execution/settlement policy and bounded drain
deadline. Mode changes do not authorize terminating unknown work as "not executed".
Verified callbacks for already-created payments and mandatory reconciliation continue
in every mode. Unexpected late external funds become an explicit reviewed credit/
refund obligation, not a discarded event because the shop is closed.

Readiness guards are transition-specific. Entering `qualification` requires secure
identity/storage/configuration plus the approved scopes and prerequisites for the
selected tests, not a prior successful result of those same tests. Entering `live`
requires all applicable public-launch evidence. Entering a more restrictive mode
does not require healthy capacity or a successful backup. If the control database
is unavailable, existing fail-closed admission/local recovery procedures stop new
effects; do not report a persistent mode change that never committed.

Public condition is a safe projection: `operational`, `degraded`, `maintenance`,
`unavailable` or `closed`, with observation time, freshness and a localized reason.
It exposes no account identities, inventory, queue/customer details or raw errors.
Mode `live` alone cannot report healthy capacity. Stale/unreachable status is unknown,
not healthy; the cabinet preserves that distinction. Put a banner in the cabinet
and public entry, including any announced window in the viewer's timezone with UTC
available. A deployment must publish a support/outage destination reachable without
this backend; the existing external monitoring verifies it. Do not require another
status microservice, and do not claim the backend can report its own total outage.

### Price and model change walkthrough

SC-06 separates draft, announced and effective configuration. Keep the current
immutable tariff/hold rules; do not edit a published price row in place.

1. Owner drafts a tariff or model-availability change, with affected public models,
   effective UTC time, reason and expected current revision. Validate all components,
   context/tier bounds and grants before it can be published.
2. Preview affected eligible workspaces/keys, queued work and existing reservations
   using authorized aggregate/count projections. Preview does not activate the model,
   change balances, execute a probe or expose one customer's data to another.
3. Publish an announcement and the scheduled revision together. Proposed launch
   default: ordinary price increases and planned model removal give at least 24 hours'
   notice; longer operator-published terms win. The notice period is a product default,
   not a statement of legal sufficiency. HD-09 must review actual customer terms.
4. At the effective time, activate once under revision checks. New admissions use
   the effective tariff. Existing holds keep their saved tariff; if a queued request
   can no longer execute under its reserved route/bounds, cancel/release with a clear
   reason rather than silently reserve a higher price. A different API supplier
   may serve the same qualified offer; price never justifies substituting another
   model, capability or service tier.
5. Show the new version/time in Models, retain the version on each request/receipt,
   and deliver the deduplicated notice. If a change is cancelled/superseded, publish
   that fact too; a stale notification cannot activate an obsolete draft.

If activation is due but the required runtime revision cannot be applied, mark the
affected route unavailable for new admissions until consistent; do not silently
continue an old price/removal policy. Restart and duplicated activation jobs use
the same change identity and original effective time. Already-held work follows
the explicit saved-price/route rules, never a blanket second charge.

Price decreases may take effect immediately after explicit publication. Emergency
access/capacity/security disable may also be immediate, with reason and notice as
soon as possible; never keep a dangerous or unauthorized route running just to meet
a notice window. Emergency disable cannot be used as an unannounced price increase.
The existing key/workspace spend caps remain enforced; H09 fixes their exact units,
windows and reservation treatment rather than treating a warning as a hard budget.

### Maintenance and recovery walkthrough

SC-07 is a bounded operational procedure, not "restart and see".

| Step | Required action | Observable stop/recovery condition |
| --- | --- | --- |
| 1. Announce or declare incident | Record reason, affected public capabilities and planned window if known; notify through configured channels | No invented restoration estimate; emergency incidents may start immediately |
| 2. Fence new effects | Enter `maintenance` and stop new funding intents/inference dispatch | New work is rejected safely, including queued work rechecked at dispatch; signed existing-payment callbacks still reconcile |
| 3. Drain and account | Wait for active work within its established deadline; persist observations, holds and job fences | No forced success/free cancellation; unknown executions remain uncertain and are never blindly resubmitted |
| 4. Protect state | Verify the compatible off-host recovery set and keys before a risky change | Failed backup/restore preflight blocks the change; mandatory evidence is not deleted to free space |
| 5. Change or restore | Follow the pinned upgrade/compatible recovery procedure with one migration owner | Mixed schema/binary versions fail safely; use documented roll-forward when rollback is incompatible |
| 6. Reconcile before reopening | Reapply revocations/tombstones, resolve payment/hold state, verify jobs and permitted capacity | Restored old credentials or missing financial evidence prevent reopening; no old snapshot is assumed complete |
| 7. Verify and resume | Run read-only/local checks while paused; explicitly enter restricted `qualification` for necessary external tests, then revalidate before `live` | No inference bypass inside maintenance; actual recovery evidence/current revision required, alert acknowledgement is not a health check |
| 8. Close communication | Mark the incident resolved and send one recovery update | Preserve audit and a safe incident summary without transcripts, credentials or customer payloads |

Every runbook names the command/UI action, required authority, expected safe output,
maximum wait, stop condition, escalation destination and rollback/roll-forward path.
H16 supplies tested executable instructions for the shipped artifact; these planning
steps are not commands to run on a user's live installation. Include separate drills
for lost owner access, vault-key loss, database outage, full disk and absent backups.

On an isolated copy, [restore-fence](../../hosted/server/README.md#fence-an-isolated-restore)
is stricter than ordinary maintenance. It prevents the restored server from
starting and blocks new merchant claims and automatic credit replay until the
missing interval can be reconciled. It does not affect callback handling on an
untouched authority, and there is no supported reopening command yet.

### Service closure and migration walkthrough

SC-08 distinguishes moving the same installation from ending its business. A host
move uses the backup/restore compatibility contract, preserves identities/ledger and
keeps only one active execution/payment authority. Changing domain/origin/RP identity
requires the documented reauthentication/passkey migration path; DNS alone is not
proof that old sessions or authenticators remain valid. No dual-active restore.

For permanent closure:

1. Owner confirms `closing` with fresh verification and a durable reviewed plan:
   sales cutoff, support contact, refund process, export window and retained-record
   policy. HD-02/09 supply provider/legal deadlines; do not invent them globally.
2. Announce the cutoff before planned closure, stop new registrations/funding/key
   issuance/inference, and finish or reconcile already accepted work. A notice
   delivery failure is recorded, not treated as customer consent or permission to
   remove access immediately.
3. Reconcile each wallet's available balance, inference holds, refund encumbrances,
   payments and disputes to the journal. Offer the reviewed return process for
   eligible unused funds. A customer's requested refund is not yet a completed one.
4. Give authenticated access to scoped history/receipts and bounded exports during
   the announced window. No cross-customer archive, permanent public download or
   export of credentials/prompts; download rechecks identity and authorization.
5. Resolve unknown external refund/payment outcomes and remaining liabilities via
   the approved provider/support process. Zero active streams is not zero financial
   obligations. Do not credit balances as "refunded" until the external effect is
   confirmed and the corresponding journal transition is committed.
6. Revoke inference keys and provider access only after work/refresh is fenced;
   keep the merchant callbacks, necessary identity/support route and required keys
   usable for outstanding reconciliation/retention. Avoid deleting the vault key
   while protected records still require it.
7. Enter `closed` only with a reviewed inventory of completed and outstanding
   obligations and their assigned recovery path. Retention/deletion jobs and
   independently recoverable evidence follow HD-09/11. Final infrastructure teardown
   is a separate verified action, never a cascade from changing service mode.

### What not to add until measured demand

- **Redis:** not a launch dependency. Add only for an evidenced cache or shared
  ephemeral-coordination need with defined outage semantics; never make it wallet,
  identity, grant or settlement authority. Shared admission/lease correctness must
  precede multiple inference nodes, whether implemented with PostgreSQL or another tool.
- **Kafka/RabbitMQ, microservices and Kubernetes:** not required for the initial
  durable worker model. Split/deploy independently only for a measured capacity,
  failure-isolation or operational requirement, not for the number of registered users.
- **Loki/ELK, ClickHouse and separate analytics storage:** defer until bounded logs
  and indexed/aggregated PostgreSQL usage demonstrably cannot meet requirements.
- **Managed PostgreSQL or multiple hosts:** evaluate when recovery/availability
  objectives or measured contention exceed the single-host baseline. Qualify the
  replacement's PITR and vault consistency; replicas and Redis alone do not supply
  shared scheduler ownership or safe money recovery.

Implementation references: [SQLx](https://github.com/launchbadge/sqlx),
[Caddy reverse proxy](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy),
[Compose in production](https://docs.docker.com/compose/how-tos/production/),
[PostgreSQL PITR](https://www.postgresql.org/docs/current/continuous-archiving.html),
[pgBackRest](https://pgbackrest.org/user-guide.html), and
[Alertmanager](https://prometheus.io/docs/alerting/latest/alertmanager/).
Revalidate selected versions, compatibility and configuration during implementation.

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

### Initial operations contract

Use one region, one version-pinned API image and PostgreSQL with bounded workers;
no Redis, Kubernetes or distributed scheduler is required for launch. HTTPS
terminates at a documented reverse proxy that supports SSE and WebSocket upgrades
without response buffering. Expose only public HTTPS; database, vault and internal
API ports stay on private/loopback networking. Keep owner management separately
authorized even when it shares the origin. DNS/TLS, trusted proxy handling, secret
injection and persistent-volume ownership are part of the installation guide.

Separate liveness (process responding), readiness (required durable stores and
runtime available) and a synthetic end-to-end inference check. Provider outage or
an empty pool is degraded service, not proof the process must restart. Alert on
failed settlement/reconciliation, oldest hold, database/disk pressure, stale
workers, vault/token-refresh failures, error/429 rates and missing backups.
Never put customer IDs, keys or arbitrary User-Agent values in metrics labels.

Planning objectives, to prove in drills before claiming them: no more than
15 minutes of durable-data loss (RPO) and recovery within 2 hours (RTO).
Daily backups alone cannot satisfy that RPO: archive PostgreSQL WAL at least
every 5 minutes and durably capture versioned vault changes with compatible
restore points. If consistent recovery cannot be established, do not claim the
objective or accept paid traffic. Keep encrypted off-host copies and an independent
recovery-key copy; test isolation/restoration monthly and before risky migrations.
Restore with ingress closed, reconcile payments and requests, reapply revocations
and expiries, then perform a controlled inference test before reopening.

Updates use an immutable version/digest, a preflight compatibility check, tested
backup, stop-admission/drain/flush, append-only migrations, readiness checks and
an inference smoke test. Do not roll back only the binary across an incompatible
schema. Document data-loss/payment-reconciliation consequences before any restore.
Abrupt termination is uncertain execution, not permission to resend requests.

Launch policy defaults: bound queued requests to 30 seconds and 20 per workspace;
start at 2 concurrent generations per customer workspace, adjustable by the owner
within measured global/member limits. Enforce per-key limits without multiplying
the workspace budget. Start a new request only when capacity and reservation
bounds are valid; explicit 429/503 errors include safe retry guidance. Global
concurrency, maximum prepared body, route output cap, paid request duration and
reconciliation deadline are required deployment settings selected from tests, not
unlimited defaults. A suggested reconciliation deadline is 15 minutes after the
execution becomes terminal/uncertain, snapshotted once and never extended by retries.
These Hosted bounds do not silently change the existing local runtime contract.

The operator runbook covers lost admin credentials, identity-provider outage,
expired upstream login, exhausted quota, stuck streams/holds, payment webhook
failure, disk full, migration failure and lost vault key. It distinguishes
disable/admission drain from data deletion, gives safe recovery checks, and never
requests customer prompts or provider secrets for routine diagnostics.

A restored database's own revocation/tombstone tables cannot prove what happened
after its restore point. HD-11 must specify an independently recoverable revocation/
deletion history or a conservative credential-invalidation/re-enrollment procedure,
and a financial reconciliation path for missing recent events. Fence keys/scopes
whose dispatch or idempotency claims could have been lost. An empty restored
lookup cannot authorize replay; reconcile the missing interval or keep that
authority disabled. A verified merchant statement can restore a funding event,
not prove that an inference was never sent. Keep ingress closed
when the evidence is insufficient. An RPO tolerance is not permission to silently
lose customer funds, resurrect deleted data or reactivate old access.

## Implementation contract map

Server paths, database objects and Hosted API interfaces below are **planned**.
The web package and shared-core anchors already exist. This map fixes vocabulary and ownership; the detailed work packages
expand each affected object into its full field/API/test specification before code.
Do not create empty files or tables just to match the map. The inventory is not
executable DDL or a claim that all field-level decisions are already resolved.

| Contract ID | Canonical subject | Implementing packages |
| --- | --- | --- |
| HC-01 | Application/file ownership and shared-code reuse | H01, H02, H13, H15 |
| HC-02 | Names, types, tenant references and migrations | H02 and every persistence package |
| HC-03 | Identity, sessions, owner access and key authority | H05, H06, H09 |
| HC-04 | Inventory, grants, catalog and commercial eligibility | H01, H07, H15 |
| HC-05 | Tariffs, journal, holds, payments and corrections | H04, H08, H10 |
| HC-06 | Execution, attempts, idempotency and recovery | H01, H09, H10, H11 |
| HC-07 | Customer API, errors, notices and compatibility | H02, H06, H09, H12, H13, H14 |
| HC-08 | Reporting, diagnostics, privacy, retention and deletion | H12, H14, H15, H16 |
| HC-09 | Vault consistency, jobs, fencing and external effects | H03, H07, H08, H10, H11, H16 |
| HC-10 | Web/operator state and distribution | H13, H14, H15 |
| HC-11 | Configuration, release, recovery and operations | H02, H16, H17 |
| HC-12 | Commercial decisions and end-to-end acceptance | H00, H17 |

### Target application and file ownership

Choose `hosted/server/` as a separate Rust crate in this repository, package/binary
`zenith-relay-hosted`. Desktop keeps its local SQLite/authentication contracts;
it does not become a mode-switch container for customer money. Hosted calls
`relay-core`, not Desktop, and does not copy the provider codec engine. This is one
modular backend, not a new service per module.

| Planned path | Responsibility |
| --- | --- |
| `hosted/server/Cargo.toml`, `src/lib.rs`, `src/main.rs` | Package, composition and CLI; independent existing crate builds |
| `hosted/server/src/app.rs`, `config.rs`, `http.rs`, `state.rs` | Startup/shutdown, validated config, registered routes and shared handles |
| `hosted/server/src/db.rs`, `db/`, `migrations/` | SQLx pool, transaction/context helpers, append-only migrations |
| `hosted/server/src/identity.rs`, `identity/` | Users/workspaces, identity linking, registration, recovery and suspension |
| `hosted/server/src/auth.rs`, `auth/` | Sessions, login attempts, WebAuthn, fresh verification, CSRF and Desktop pairing |
| `hosted/server/src/access.rs`, `access/` | Key verifiers, grants/limits and authenticated request context |
| `hosted/server/src/management.rs`, `management/` | Revisioned inventory/pool changes, safe projections, transfers and audit |
| `hosted/server/src/management/service.rs`, `management/readiness.rs` | Service-mode fencing, staged changes/closure and setup evidence; not a second admission or health authority |
| `hosted/server/src/catalog.rs`, `catalog/` | Customer projection over the core catalog and qualified routes |
| `hosted/server/src/accounting.rs`, `accounting/` | Exact money/tariffs, journal, wallet/hold projections and settlement |
| `hosted/server/src/payments.rs`, `payments/` | Merchant adapter, intents, events, refunds and reconciliation |
| `hosted/server/src/inference.rs`, `inference/` | Admission/reservation, core context, attempt observation and cancellation |
| `hosted/server/src/continuations.rs`, `continuations/` | Scoped checkpoints and optional encrypted payload lifecycle |
| `hosted/server/src/usage.rs`, `usage/` | Safe reports, aggregates, filters, details, diagnostic projections and bounded exports |
| `hosted/server/src/usage/diagnostics.rs`, `management/diagnostics.rs` | Shared diagnostic mapping/read projections and owner-only capture lifecycle; reuse domain evidence, no separate billing or inference engine |
| `hosted/server/src/notifications.rs`, `notifications/` | Domain outbox, own-user notices/preferences and optional qualified delivery; reuse durable jobs, never own money or identity verification |
| `hosted/server/src/vault.rs`, `jobs.rs`, `jobs/` | Versioned secret references, vault integration, durable bounded work and fencing |
| `hosted/server/openapi.yaml` | Dedicated Hosted schema |
| `hosted/server/tests/`, `tests/support/` | Real-PostgreSQL and synthetic integration/crash fixtures |
| `hosted/server/README.md`, `hosted/deploy/` | Future operator guide and pinned Compose/Caddy/monitoring/backup artifacts |
| `hosted/web/package.json`, `hosted/web/src/`, `hosted/web/tests/`, `hosted/web/README.md` | Independent website build, typed client, UI/tests and frontend guide |
| `hosted/web/src/api/` (planned), existing `hosted/web/src/features/` and `hosted/web/src/i18n/` | Typed transport, role-specific UI and locale dictionaries; no database/vault access |

The customer cabinet and `/admin` may share a static artifact, never a session realm
or authorization check. Rendering a hidden admin page is not access control.

Existing reuse anchors: `crates/relay-core/src/gateway.rs`, `runtime.rs`, `protocol/`,
`scheduler/`, `accounts/`, `sources/`, `pricing/`, `usage/`; Desktop reference flows:
`desktop/src-tauri/src/local_pool/{state.rs,store/vault.rs,accounts/authority.rs,usage_writer.rs}`.
H01 identifies actual exported integration points and necessary narrow shared changes.
These paths do not authorize copying local SQLite/auth assumptions into Hosted.
Reuse/extract the vault primitive only after format/crash guarantees are checked;
no new cipher and no whole Desktop dependency for one helper.
Shared changes retain Desktop regression coverage.

### Naming, value types and schema boundaries

Use a dedicated PostgreSQL database with application objects in `relay_hosted`.
Tables, columns and JSON fields use `snake_case`; Rust types are singular, tables
plural. New Hosted entity IDs are UUIDs. Existing core/provider/model identifiers
retain their validated types and meanings. IDs never prove access. Tenant-owned
references include `workspace_id` and a composite foreign key to the same workspace;
installation-owned data is explicitly classified, not assigned a magic customer ID.

| Value | Storage and wire rule |
| --- | --- |
| ID | PostgreSQL `uuid`; opaque JSON string; server-generated ownership references |
| Time | `timestamptz`; RFC 3339 UTC wire value; persisted deadlines never reset on retry |
| Money | Signed `bigint` microUSD; checked arithmetic; JSON decimal integer string, `currency: "USD"`, `precision: 6` |
| Token count | Nullable nonnegative `bigint`; decimal integer string or null; separate completeness |
| Purchase or selling rate | Exact `numeric(30,12)` USD per million tokens; canonical decimal string, explicit component/band/tier |
| Revision | Nonnegative `bigint`; decimal string on wire; owner transaction increments, mutations compare expected revision |
| Status/code | Finite validated set; compatibility/migration review before introducing a value |
| Digest/verifier | Versioned binary digest and algorithm/key version; no plaintext token or unkeyed low-entropy request hash |
| Metadata | Typed columns first; JSONB only for a versioned allowlisted bounded schema, not arbitrary headers/provider payloads |

Rate precision is not an actual tariff. Reject out-of-range precision rather than
silently round it. Checked exact wider intermediates and the existing single final
rounding rule own money. Browser `Number` may produce bounded chart coordinates,
never authoritative wallet/settlement arithmetic.

Do not cast a retail-rate string to integer microUSD per million. The
[Hosted calculator](../../hosted/server/src/accounting.rs) and the catalog/editor
share [exact rate vectors](../../hosted/server/tests/fixtures/retail-rates.json).
For example, `0.000000400000` USD per million × 3,000,000 ordinary
input tokens = 1.2 microUSD, rounded **once** to a 2 microUSD debit when it is
the only component. The editor must round-trip that rate exactly; customer
display must not turn it into a free offer. Journal integers and tariff decimals
have different units and wire parsers.

Each record has a required server-generated `id` and `created_at`, unless its
explicit composite key replaces `id`. Mutable records add `updated_at` and
`revision`; append-only records have no generic update path. `?` in the inventories
means nullable with explicit absent/not-observed meaning; other listed fields are
required. Required business values have no invented default. Do not make a field
optional to avoid finishing its specification.

Before DDL, specify every column's type/range/length/default, unique/check
constraint, tenant foreign key/delete action, writer, reader, sensitivity and
retention. Name every query and its matching index, including descending stable
pagination and pending-work scans. Default to `RESTRICT` for referenced financial
or security evidence; explicit jobs own tombstone/anonymization. A user deletion
must not cascade into journal/settlement deletion.

Use ordered SQL files under `hosted/server/migrations/`, beginning with
`0001_installation.sql`; H02 freezes the remaining descriptive names/order before
the first migration is committed. Applied migrations are immutable. One controlled
runner holds database-level migration exclusion. Separate runtime, migrator and
backup roles; the runtime cannot alter schema or rewrite protected journal rows.
Authorization remains mandatory even if RLS is enabled; transaction-local scope
must be cleared before pooled connections are reused.

### Persistent identity and access inventory

| Table | Domain fields beyond common fields | Constraint and meaning |
| --- | --- | --- |
| `installations` | `owner_user_id?`, `setup_state`, `authorization_epoch` | Singleton; owner set only by verified local provisioning |
| `users` | `status`, `display_name?`, `authorization_epoch`, `deleted_at?` | Internal person, not email identity; suspension spans all login channels |
| `workspaces` | `owner_user_id`, `kind`, `status`, `authorization_epoch` | One personal workspace per user; no implicit team access |
| `identity_providers` | `kind`, `issuer`, `client_id`, `secret_ref?`, `enabled`, `configuration_version` | Installation-owned verified authority; secret rotation is not issuer replacement |
| `external_identities` | `provider_id`, `subject`, `user_id`, `linked_at`, `revoked_at?` | Unique `(provider_id, subject)`; no email-based merge |
| `auth_attempts` | `realm`, `purpose`, `provider_id?`, `user_id?`, `challenge_digest`, `secret_ref?`, `expires_at`, `consumed_at?` | One-use setup/login/link state; exchange secrets encrypted when required |
| `admin_credentials` | `user_id`, `credential_id`, `public_key`, `user_handle`, `authenticator_metadata`, `revoked_at?` | Unique credential ID; library-required counter/backup state, no private key |
| `admin_assignments` | `user_id`, `role`, `granted_by?`, `revoked_at?` | Installation owner only initially; workspace ownership cannot grant this |
| `sessions` | `realm`, `user_id`, `token_verifier`, `authorization_epoch`, `last_seen_at`, `expires_at`, `fresh_verified_at?`, `revoked_at?` | Customer/admin realms distinct; server revocation and idle/absolute expiry |
| `management_devices` | `owner_user_id`, `device_label`, `credential_verifier`, `scope`, `expires_at`, `revoked_at?` | Explicit owner-approved pairing within the limited Desktop scope; no full owner grant or permanent master-token login |
| `inference_keys` | `workspace_id`, `issue_operation_id`, `assigned_user_id?`, `public_prefix`, `secret_verifier`, `verifier_version`, `status`, `expires_at?`, `last_used_at?`, `authorization_epoch` | Unique workspace issuance operation; recover only the masked result. Secret shown once; does not authenticate customer/admin APIs |
| `key_policies` | `workspace_id`, `key_id`, `policy_version`, `allowed_operations`, `model_scope`, `limit_policy` | Typed narrowing of workspace grants; H09 specifies each policy field |
| `revocation_events` | `target_kind`, `target_id`, `workspace_id?`, `authorization_epoch`, `reason_code`, `effective_at` | Durable restore fence; no resurrection of revoked access |

Changing a provider issuer creates a new authority registration, not a silent
reinterpretation of old subjects. Verification happens outside the DB transaction;
registration then atomically creates/claims identity, user, personal workspace,
empty wallet and ledger-account bindings. Conflicting callbacks resolve the same
identity. Display metadata is bounded optional presentation, never a matching key;
do not store raw external profile payloads.

### Persistent capacity and execution inventory

| Table | Domain fields beyond common fields | Constraint and meaning |
| --- | --- | --- |
| `connections` | `owner_scope`, `workspace_id?`, `source_kind`, `connector_id`, `core_connection_id`, `secret_ref`, `status`, `authorization_epoch` | Inventory independent of pool membership; connector identity preserves provider family and credential semantics, not a default ChatGPT client |
| `connection_auth_attempts` | `owner_scope`, `workspace_id?`, `actor_id`, `connector_id`, `target_connection_id?`, `expected_authorization_epoch?`, `connector_configuration_version`, `state`, `state_version`, `secret_ref?`, `expires_at`, `consumed_at?` | One-use provider connection onboarding/reauthentication, separate from customer/admin `auth_attempts`; protected exchange state, actor/target binding and cancellation/replacement fences |
| `connection_imports`, `connection_import_items` | Operation: installation/actor and optional paired device, operation ID, selection digest, expected revision, state and expiry. Item: operation/item ID, source kind, safe identity/proxy refs, stage/fence, result and destination ref | H07/H15: one result per approved item; commit its result with the inventory change. Keep ownership/recovery evidence until resolved. File contents, proxy passwords and tokens never enter the manifest or receipt. |
| `pools` | `owner_scope`, `workspace_id?`, `core_pool_id`, `status`, `policy_version`, `core_policy` | Reuse core validated pool policy; no new parallel scheduling semantics |
| `pool_members` | `pool_id`, `connection_id`, `enabled`, `drain_state`, `core_policy_version`, `core_policy` | Physical identity shared across aliases/protocols, not multiplied capacity |
| `route_qualifications` | `connection_id`, `public_model_id`, `upstream_model_id`, `operation`, `protocol`, `credential_class`, `evidence_version`, `status`, `expires_at?` | Exact-route permission/usage/bounds evidence, not trust in model names |
| `model_grants` | `workspace_id`, `public_model_id`, `route_scope`, `allowed_operations`, `status`, `authorization_epoch` | Can only narrow eligible capacity; no client-selected foreign source |
| `configuration_revisions` | `scope`, `subject_id`, `previous_revision?`, `schema_version`, `safe_configuration`, `actor_id` | Validated, monotonic and secret-free. Catalog `modelDisplayOrder`, per-API markup/purchase limits, selling rules and pool/offer supply policy have distinct scopes; a rotation edit cannot grant paid fallback |
| `requests` | `workspace_id`, `key_id`, `assigned_user_id?`, `endpoint`, `public_model_id`, `supply_policy_revision`, `execution_state`, `state_version`, `terminal_reason?`, `admitted_at?`, `dispatched_at?`, `terminal_at?`, `reconciliation_deadline?` | Logical execution/minimal financial anchor; retain the admitted supply scope, not prompts or answers |
| `request_details` | `workspace_id`, `request_id`, `safe_labels`, `user_agent_label?`, `timings`, `safe_error?`, `expires_at` | Short-lived detail separate from retained settlement evidence |
| `request_attempts` | `workspace_id`, `request_id`, `attempt_number`, `connection_id`, `route_revision`, `fallback_consent_revision?`, `quota_evidence_ref?`, `source_rate_revision?`, `purchase_month_utc?`, `purchase_bound_micro_usd?`, `accounted_purchase_micro_usd?`, `execution_certainty`, `state`, `observed_usage`, `usage_completeness`, `expense_evidence`, `started_at`, `ended_at?` | Unique request/attempt number; paid fallback evidence and physical expense stay separate from the logical charge |
| `idempotency_claims` | `workspace_id`, `key_id`, `endpoint`, `key_digest`, `request_digest`, `digest_version`, `request_id`, `defaults_snapshot`, `expires_at` | Unique scoped key digest; claim/request/reservation commit together |
| `continuation_sessions` | `workspace_id`, `key_id`, `origin_request_id`, `origin_attempt_id`, `provider_scope`, `model_id`, `endpoint`, `adapter_version`, `authorization_epoch`, `turn_version`, `state`, `expires_at` | Tenant/key/provider ownership; metadata alone is not replay material |
| `continuation_checkpoints` | `workspace_id`, `session_id`, `turn_version`, `request_id`, `call_correlations`, `payload_ref?`, `payload_schema_version?`, `committed_at`, `expires_at` | Unique session/turn; committed compatible state only |
| `execution_leases` | `subject_kind`, `subject_id`, `owner_instance_id`, `fencing_token`, `expires_at`, `last_heartbeat_at` | Stale owner cannot dispatch or overwrite newer state |

H07 finishes the pool/grant dictionaries against actual shared types before DDL;
the map does not authorize serializing a whole runtime object graph. Retain safe
historical source references after deletion. Secret removal and late refresh cannot
resurrect a connection. Hosted admission and the core scheduler have one documented
ownership handoff, not two independent counters claiming the same physical slot.

### Persistent accounting and operations inventory

| Table | Domain fields beyond common fields | Constraint and meaning |
| --- | --- | --- |
| `api_source_rate_revisions` | `connection_id`, `upstream_model_id`, `operation`, `service_tier`, `context_band`, `currency`, `component_rates`, `revision`, `effective_at` | Private purchase rates for one source variant; retain the revisions used by attempts and markup reviews |
| `api_purchase_periods` | `connection_id`, `month_start_utc`, `accounted_micro_usd`, `outstanding_bound_micro_usd` | One row per connection/month, rebuilt from attempts. Updated with the dispatch permit or expense reconciliation; not a wallet |
| `tariff_versions` | `public_model_id`, `currency`, `version`, `effective_at`, `status`, `rounding_version`, `pricing_review_refs` | Immutable published selling rates. Owner-only references identify the reviewed rule and just the purchase/default revisions it used. Holds use the saved tariff, not those inputs |
| `tariff_components` | `tariff_version_id`, `operation`, `component`, `context_band`, `service_tier`, `usd_per_million` | Exact selling rates for non-overlapping components of the offer; missing is not zero |
| `wallets` | `workspace_id`, `currency`, `available_micro_usd`, `reserved_micro_usd`, `version`, `spending_state` | Unique workspace/currency; nonnegative journal-rebuildable projections; reserved total includes separately disclosed inference/refund holds |
| `ledger_accounts` | `workspace_id?`, `account_kind`, `currency` | Explicit customer available/reserved and installation clearing/revenue/adjustment accounts |
| `journal_transactions` | `operation_kind`, `operation_id`, `workspace_id?`, `currency`, `reason_code`, `actor_id?`, `request_id?`, `payment_id?`, `reverses_transaction_id?` | Unique operation; append-only balanced group; original corrections remain visible |
| `journal_postings` | `transaction_id`, `account_id`, `amount_micro_usd`, `sequence` | Signed exact amounts sum to zero per transaction/currency; no isolated posting commit |
| `reservations` | `workspace_id`, `wallet_id`, `request_id`, `hold_micro_usd`, `state`, `tariff_version_id`, `sizing_snapshot`, `policy_version`, `closed_at?` | One per logical request; bounded hold and atomic once-only closure |
| `settlements` | `workspace_id`, `request_id`, `reservation_id`, `charge_micro_usd`, `basis`, `usage_snapshot`, `journal_transaction_id?`, `settled_at` | Unique reservation closure; explicit zero/waiver without fictitious money postings |
| `merchant_bindings` | `id`, `provider_id`, `environment`, `merchant_identity`, `secret_ref`, `revision` | Installation-owned connection. Changing merchant identity or environment creates a distinct binding; rotating credentials does not reassign existing payments. |
| `payment_method_revisions` | `method_id`, `revision`, `merchant_binding_id`, `adapter_method`, `channel`, `fee_bearer`, `display_order`, `state`, `check_ref?` | Only the qualified, published revision is selectable for a new quote. Changing who pays the fee never reprices an accepted quote. Disabling a method preserves its history and settlement references. |
| `payment_intents` | `workspace_id`, `wallet_id`, `operation_id`, `quote_ref`, `method_id`, `method_revision`, `provider_id`, `merchant_binding_id`, `merchant_reference?`, `quoted_currency`, `quoted_amount`, `credit_micro_usd`, `state`, `expires_at` | Claim operation and accepted quote once for this wallet before contacting the merchant; provider reference may not exist yet. Redirect alone cannot credit funds |
| `payment_events` | `provider_id`, `merchant_binding_id`, `provider_event_id`, `merchant_reference`, `event_kind`, `verified_at`, `processing_state`, `safe_evidence`, `processed_at?` | Unique provider/binding/event; binding identifies the merchant account and environment, not the current signing-secret version. Resolve the same intent/economic effect across credential rotation. No raw webhook archive. |
| `payment_refunds` | `workspace_id`, `wallet_id`, `payment_intent_id`, `operation_id`, `amount_micro_usd`, `provider_reference?`, `state`, `reservation_state`, `hold_micro_usd?`, `reserve_transaction_id?`, `close_transaction_id?`, `reason_code` | Unique operation; refund reservation is `none`, `held`, `consumed` or `released`, independent of inference; unknown provider effect cannot release spendable funds |
| `durable_jobs` | `kind`, `dedupe_key`, `safe_payload`, `state`, `not_before`, `attempt_count`, `max_attempts`, `lease_owner?`, `fencing_token`, `lease_until?`, `last_error_code?` | Bounded at-least-once work with idempotent effects, never a generic inference retry job |
| `usage_daily` | `workspace_id`, `day_utc`, `model_id`, `key_id`, `metric_version`, `coverage`, `counts`, `tokens`, `net_charge_micro_usd`, `latency_summary` | Rebuildable adjustment-aware projection; no body/User-Agent dimensions |
| `notification_outbox` | `event_key`, `event_kind`, `workspace_id?`, `subject_kind`, `subject_id`, `subject_revision`, `audience`, `template_key`, `safe_parameters`, `state`, `not_before`, `expires_at` | Unique domain effect/revision; commit with the source transition; bounded fanout, no raw message or copied financial authority |
| `notifications` | `workspace_id`, `user_id`, `event_key`, `template_key`, `safe_parameters`, `read_at?`, `superseded_at?`, `expires_at` | Unique recipient/event; current authorization rechecked on read/action; opaque event key survives outbox cleanup |
| `notification_preferences` | `workspace_id`, `user_id`, `event_preferences`, `low_balance_micro_usd?`, `balance_episode_id?`, `balance_below_threshold?`, `observed_wallet_revision?`, `external_channel_id?`, `protected_recipient_ref?`, `recipient_revision`, `recipient_verified_at?`, `consent_at?` | Unique workspace/user; durable threshold episode/rearm state; external delivery requires explicit consent and verified channel-specific binding, never just a login claim |
| `notification_deliveries` | `workspace_id`, `user_id`, `event_key`, `channel_id`, `recipient_revision`, `operation_id`, `state`, `attempt_count`, `provider_reference?`, `next_attempt_at?`, `expires_at`, `last_error_code?` | Unique recipient/event/channel; bounded queued/sent/failed/unknown/suppressed delivery, no exactly-once claim where provider cannot deduplicate |
| `service_control` | `installation_id`, `mode`, `admission_epoch`, `qualification_scope_ref?`, `operation_id`, `reason_code`, `changed_by`, `changed_at` | One row per installation; revision-CAS mode + admission fence; qualification scope is protected verified-subject/workspace policy, never a public bypass flag |
| `readiness_results` | `check_id`, `scope`, `status`, `required`, `config_revision`, `artifact_revision`, `checked_at?`, `valid_until?`, `evidence_ref?`, `reason_code?` | Safe check history; only current applicable evidence can authorize activation; invalidated/expired evidence is not green |
| `service_announcements` | `kind`, `state`, `target_kind`, `target_id?`, `target_revision?`, `affected_public_models`, `effective_at`, `ends_at?`, `template_key`, `safe_parameters`, `published_at?`, `operation_id`, `actor_id` | A scheduled change references a domain revision. Informational `service_notice` uses a fixed template with bounded plain-text title/body; `published_at` fixes its audience cutoff and immediate effective time. |
| `model_health_observations` | `public_model_id`, `qualification_scope`, `observed_at`, `valid_until`, `condition`, `evidence_revision` | Bounded service-health evidence for the 7-day status window; query only applicable routes, never expose source IDs. Customer traffic metrics use scoped usage records, not these observations. |
| `audit_events` | `actor_kind`, `actor_id?`, `workspace_id?`, `action`, `target_kind`, `target_id`, `outcome`, `reason_code`, `safe_changes`, `correlation_id` | Append-only privileged/security evidence; no plaintext secrets/assertions |
| `diagnostic_events` | `diagnostic_version`, `scope_kind`, `workspace_id?`, `request_id?`, `attempt_id?`, `job_id?`, `connection_id?`, `capture_session_id?`, `component`, `stage`, `origin?`, `code`, `category`, `severity`, `summary_key`, `safe_parameters`, `cause_status`, `safe_evidence`, `correlation_id`, `occurred_at`, `observed_at`, `sequence`, `expires_at` | Bounded non-authoritative observations; common `id` is DTO `event_id`; retries preserve event identity, no raw payload or copied current wallet/execution state |
| `diagnostic_sessions` | `owner_id`, `authorization_epoch`, `operation_id`, `scope_kind`, `scope_refs`, `reason_code`, `state`, `started_at`, `expires_at`, `ended_at?`, `limits`, `consumed_events`, `consumed_bytes`, `fencing_token`, `stop_reason?` | Owner-only scoped temporary capture; unique start operation and serialized installation-wide active limit; original expiry never extended |
| `deletion_tombstones` | `subject_kind`, `subject_id`, `workspace_id?`, `effective_at`, `retention_policy_version` | Restore reapplies removal/expiry without erasing required financial evidence |
| `vault_generations` | `generation`, `manifest_digest`, `key_version`, `durable_at`, `archive_reference?` | Available encrypted generation; backup manifest binds compatible DB/vault state |

Refund reservations use a distinct customer refund-reserved ledger account, not an
inference request row. The public wallet projection returns total reserved plus
`inference_reserved_micro_usd` and `refund_reserved_micro_usd` whose sum must match.
Financial closure cannot depend on notice retention or delivery state. H04 completes
the balanced posting chart and concurrent refund/spend guards under HD-04.

Notification preference state is current policy; outbox/notice/delivery records are
not identity or authorization evidence. A low-balance episode has a stable key and
durable rearm state scoped to the preference revision; H12 specifies its transaction
and index so worker replay cannot notify repeatedly. Expired notices/outbox payloads
cannot cascade into journals, audit or an active delivery. Cleanup removes eligible
children first or retains only an opaque event reference; no copied payload outlives
its allowed expiry. Pending work has a finite deadline and terminal reason.

H02/H12/H16 freeze indexes for pending outbox/jobs, own-user `(created_at, id)`
notice pagination, unique recipient/event delivery, due change activation and
current readiness scope/revision. Enforce server-owned audience/scope and protected
recipient references; neither null workspace nor a supplied user ID creates access.
Do not serialize free-form message templates, credentials or arbitrary URLs into
these JSON fields. Lifecycle/qualification policy mutations remain separately audited.

`qualification_scope_ref` references a protected `configuration_revisions` row, not
a file/URL or client-selected JSON. Its versioned allowlist binds registered provider
IDs to digests of verified subject identities, digest version, optional resolved
workspace IDs and grant expiry. H06 obtains an unregistered test identity through
normal verified login-attempt evidence before owner approval; approval does not
itself create a wallet or admin role. Registration/dispatch recheck the current
scope revision and expiry. H05/06 specify allowed fields, bounds and digest rotation;
raw assertions, tokens and email-based identity shortcuts are forbidden.

Diagnostic reads compose current execution/accounting state from its domain owner
with optional chronological observations; `diagnostic_events` is not another ledger,
attempt scheduler or source of terminal truth. Do not copy every runtime log into
PostgreSQL. Retain allowlisted transitions/errors and explicitly scoped capture
detail with bounded ingest, queue, event-size and storage budgets. When a budget or
sink fails, report partial/unavailable coverage rather than a complete empty result.
H12 fixes how coverage/watermarks are recorded and conservatively recovered after
restart; the domain's required evidence is not optional telemetry.

Event `scope_kind` is `request`, `job` or `component`, derived server-side. Request
events require matching workspace/request/attempt references; job/component events
are owner-only. Optional connection references are validated against the resolved
scope, not accepted from a caller. A null workspace or client-supplied correlation
ID never grants access. H12 defines composite reference checks and stable indexes
for `(workspace_id, request_id, created_at, id)`, owner operations by time/component,
and expiry cleanup before DDL. Scope-bound cursors use server insertion order;
`occurred_at`/source sequence describe the observed timeline, not a total cross-
component ordering. Late arrivals and missing stages remain explicit.

Deleting closed capture-session metadata clears only optional `capture_session_id`
backreferences (`SET NULL`), never still-eligible request events. Privileged audit
keeps its opaque target ID independently; it cannot cascade with session cleanup.

Capture states are `active` -> `stopped`, `expired`, `exhausted` or `revoked`;
terminal sessions cannot reopen. Start commits the authorized scope, fixed deadline,
limits and audit atomically before collection. Stop and terminal reason use the
existing fenced/idempotent job primitives. Budget reservation happens before append;
concurrent collectors cannot each consume the full installation allowance. Restart
never resets counters/deadlines; unsafe recovery closes capture with a recorded
reason. H16 qualifies byte/event/rate limits, durable accounting of capture budgets,
expiry enforcement and restart behavior without introducing a second job system.
If expiry, authority or remaining budget cannot be established, collection stops
immediately. Audit-storage failure cannot delay stopping or authorize more capture;
record closure on recovery and disclose any missing audit evidence, never invent it.

H04 expands ledger accounts into a precise chart and posting matrix for top-up,
hold, settle, release, refund, chargeback and adjustment before money code. Positive
customer available/reserved postings increase those projections; counter-postings
balance them. Name each operation's preconditions, zero-case behavior and postings;
account names alone are not an accounting algorithm. No customer ledger-edit API.

A posting may reference its transaction's customer accounts or explicitly allowed
installation clearing/revenue accounts, never another customer's accounts. Null
workspace scope is restricted to named installation account kinds, not an
authorization bypass. Enforce those references in schema and accounting writers.
Report corrections apply to request-linked postings only; a top-up refund is wallet
activity, not negative model usage.

The base hold/closure invariants use `H` for reserved amount and `C` for charge:

| Operation | Customer available delta | Reserved delta | Condition |
| --- | --- | --- | --- |
| Verified top-up `T` | `+T` | `0` | Unique verified payment; currency/quoted credit match |
| Reserve `H` | `-H` | `+H` | Available at least `H`; exact request/key/grant/price snapshot valid |
| Settle `C` | `+(H-C)` | `-H` | Exact held reservation; `0 <= C <= H`; revenue/clearing counter-postings |
| Release | `+H` | `-H` | Same closure guard; explicit waiver/non-dispatch reason |
| Late lower charge correction `D` | `+D` | `0` | Unique compensating operation referencing original charge; no hold reopening |

For the first funding slice, the trusted input is an **already accepted invoice**.
Checkout owns the workspace, quote, merchant binding, payable currency and integer
minor-unit total, and the separate USD wallet credit. The adapter verifies the
delivery before opening a database transaction. Its configured binding identifies
one provider, merchant and environment; the delivery supplies only event and invoice
references (1–128 printable non-space ASCII bytes), paid currency and positive
minor-unit amount. No browser, redirect or event supplies the wallet or its credit.

Claim `(binding, event)` against the matching invoice, then lock its wallet.
One receipt per invoice posts `+credit` to that workspace's USD available account
and `-credit` to installation payment clearing, in the same commit as the wallet
update. A second event for that invoice returns the original receipt; reusing an
event for another invoice is a conflict. A missing reference or mismatched amount
is not consumed. Reconcile lost acknowledgements by redelivering the same verified
facts. These are existing obligations even when admission is closed. Wallets and
account bindings start empty in the personal workspace creation transaction.
Only the private settlement role may execute the posting function; it has no
direct table writes. Keep intent, matched event, receipt and postings together as
financial evidence, with no cascading deletion.

Refund/chargeback after funds are spent needs an explicit H04/H08 policy: record
the externally imposed obligation separately, restrict new spending and reconcile;
never delete historical spend, make reserved funds silently negative or ignore the
external reversal. The receivable/posting representation and customer disclosure
are blocking finance decisions, not assumed implementation details.

### Transaction, external-effect and restart matrix

| Operation | Atomic durable boundary | Failure/duplicate rule |
| --- | --- | --- |
| Registration | Identity claim + user + workspace + empty wallet/account bindings | Duplicate callbacks resolve one identity; no transaction during external verification |
| Key issuance | Unique scoped issue-operation claim + verifier/scope + audit | Lost response looks up the masked record; revoke/reissue explicitly, never recover plaintext |
| Request reserve | Idempotency claim + guarded wallet update + postings + request/hold/price snapshot | Commit-ack loss triggers same-operation lookup, not new generation |
| Dispatch | Fenced intent/attempt and, for paid API supply, a revision-checked purchase allowance and period update in the same commit before send | Lost acknowledgement recovers the existing attempt and bound; uncertain send or expired lease cannot authorize resend |
| Terminal observation | Versioned execution/usage evidence + required continuation checkpoint | Already streamed output cannot be recalled; failed commit cannot advertise resumable success |
| Settle/release | Conditional held-state closure + settlement + balanced postings + wallet projection | Duplicate closer reads original outcome, never a second debit |
| Payment credit | Verified event claim + intent transition + journal credit + receipt reference | Duplicate/out-of-order events cannot credit twice; external reconciliation outside locks |
| Refund | Durable operation claim, external action, verified completion posting | Stable provider reference/idempotency; timeout requires reconciliation before resend |
| Secret create/rotate | Durable encrypted version, then DB reference/revision/fence | Unreferenced ciphertext reclaimable; failed commit preserves old live secret |
| Secret removal | Fence/revoke references and jobs, then eligible ciphertext cleanup | Late refresh cannot restore a source; backup expiry remains disclosed |
| Configuration edit | Expected revision + validated replacement + audit | Stale writer gets conflict; failed runtime activation stays explicit |
| Retention/export | Scoped cutoff/snapshot and idempotent progress | No deletion of unsettled dependencies or export across changed authorization |

One lock order applies to all accounting writers: unique operation claim; relevant
workspace/key policy/counter rows; API connection policy then purchase period;
wallets; request/attempt/reservation rows; journal append. Use stable ID order
within each class. Omit unneeded classes, never
reverse them. Specify exact rows, bounded lock/statement timeouts and classified
whole-transaction retries. Revalidate mutable authority/limits inside the
transaction. No provider/payment/identity/archive calls while holding these locks.

An intent plus a lease does not make a remote effect exactly-once. Recovery uses
that external system's idempotency/evidence contract. Inject failures before commit,
after commit before acknowledgement, before/after send, before observation and
before/after closure. The expected money and execution outcome must be specified
for each cut point, including when safe progress is only reconciliation.

Additional transaction boundaries for the scenario walkthroughs:

| Operation | Atomic durable boundary | External effect / retry rule |
| --- | --- | --- |
| Approve refund | Guard original refundable remainder and wallet, reserve refund funds, record operation/postings | Call merchant outside locks; unknown result queries the same reference, never refunds again blindly |
| Confirm import item | Recheck actor/device, selected identity, revision and ownership fence; commit destination binding, vault reference and item receipt together | Resolve missing acknowledgements by operation/item. Never declare OAuth ownership transferred before destination verification or resume a fenced source on timeout |
| Emit customer notice | Domain transition plus unique outbox event, or same-operation recovery of that commit | Jobs project/deliver by event/recipient identity; channel outage cannot reverse money |
| Change service mode | Expected revision, current authority/readiness guards, mode + admission epoch + audit | Dispatch rechecks fence; active work settles normally; lost acknowledgement reads same transition |
| Publish scheduled change | Immutable target revision, effective time, announcement and notification outbox | Due activation is fenced/idempotent; late worker cannot silently apply stale policy |
| Activate public service | Required current readiness scopes/versions plus verified owner transition | No paid probe inside the transaction and no frontend-only launch switch |

### Canonical execution and accounting states

The lifecycle diagram is a summary; this table owns the execution vocabulary.
`active` is a customer grouping, not another persisted terminal state. Queueing is
an optional bounded phase after reservation; nonstreaming responses may go directly
from dispatched to a terminal observation. Rejected pre-admission activity is
reported separately and never counted as a new admitted generation.

| Execution state | Meaning | Allowed next state or evidence rule |
| --- | --- | --- |
| `received` | Parsed/authenticated request awaiting admission | `admitted`, or rejection with no hold/dispatch |
| `admitted` | Policy route/bound checks passed | `reserved`, or rejection/cancellation before dispatch |
| `reserved` | Durable claim and bounded hold committed | `queued`, `dispatched`, `failed`, `cancelled` |
| `queued` | Waiting within workspace/global queue bounds | `dispatched`; deadline/revocation becomes `failed` or `cancelled` and releases hold |
| `dispatched` | Durable dispatch intent; external execution may have begun | `streaming`, `completed`, `incomplete`, `failed`, `cancelled`, `uncertain` |
| `streaming` | Observing/forwarding provider work | `completed`, `incomplete`, `failed`, `cancelled`, `uncertain` |
| `completed` | Validated successful terminal result | No repeat execution; accounting/expense evidence may reconcile independently |
| `incomplete` | Known non-complete terminal result, including evidenced truncation | Preserve reason and usage; no automatic repeat |
| `failed` | Known failed logical outcome | Preserve execution certainty and any expense; failure does not imply no execution |
| `cancelled` | Cancellation outcome established | Preserve observed usage; client disconnect alone is insufficient evidence |
| `uncertain` | External execution/outcome not established | Only new authoritative evidence may resolve outcome, never lease-expiry resend |

The durable request is inserted with the reservation; earlier stages can be
transient admission states. H09 records pre-admission rejection activity in bounded
safe diagnostics, not phantom funded requests. State changes use version-CAS and
execution fences. Separate physical attempt state/usage from logical outcome.
Late authoritative evidence may resolve uncertainty but cannot erase the original
observation, reopen closed money or re-run an already dispatched request.

Accounting states remain `none`, `held`, `held_for_reconciliation`, `settled`,
`released`. Reconciliation preserves the original finite deadline. Settlement basis
and usage completeness are separate fields, not more execution states. Settled
does not mean successfully completed; incomplete does not mean free. Customer
completion-rate denominators and filters use this mapping consistently.

### Hosted API and browser contract inventory

Reserve `/api/hosted/v1` for Hosted control/customer APIs. The existing supported
inference protocol paths remain owned by `relay-core`; do not redirect them into a
new control envelope or infer support for every `/v1/*` path. The Hosted schema is
[the implemented OpenAPI subset](../../hosted/server/openapi.yaml). The inventory
below describes the wider Hosted target, not Desktop routes.

The following are target route names. H02 fixes shared envelopes; each domain
package must specify every request/response field, permission, limit, pagination,
state/error and synthetic example before implementing its routes. No endpoint is
complete merely because this inventory names it. Below, paths omit the prefix.

| Method and path | Authority / behavior owner |
| --- | --- |
| `GET /public/config` | Anonymous allowlisted branding, supported API version, enabled login methods and public site entries (including Studio when offered). Visibility is not an image grant; no prices/secrets/private config. |
| `GET /public/status` | Allowlisted public condition, observation/freshness and announcement/support link; no private capacity/customer detail and no active probe |
| `GET /public/models` | Anonymous bounded public model/tariff publication, revisions/effective time and freshness; no customer grants, upstream inventory or credential/capacity disclosure; H07 owns explicit publication and fields |
| `POST /auth/customer/{provider_id}/begin` | Bounded login/link attempt; provider registration selected from server allowlist |
| `GET /auth/customer/{provider_id}/callback` | Configured authorization-code callback, verified one-use attempt; provider-specific contract revalidated in H06 |
| `POST /auth/customer/logout` | Customer session + CSRF; server revocation, browser state cleared |
| `POST /auth/admin/sign-in/options`, `POST /auth/admin/sign-in/verify` | Bounded WebAuthn ceremony; verified owner role before admin session |
| `POST /auth/admin/logout` | Separate admin session + CSRF; no customer-session substitution |
| `GET /admin/overview` | Current closed service mode and session-bound CSRF token; never a combined preview snapshot |
| `GET /account` | Current user/workspace, session capabilities and account restrictions |
| `GET /account/identities`, `DELETE /account/identities/{id}` | Own verified identity links; fresh proof and remaining-method guard for removal |
| `GET /account/sessions`, `DELETE /account/sessions/{id}` | Own customer sessions/revocation; never administrator session inventory |
| `GET /account/notifications`, `POST /account/notifications/{id}/read` | Own bounded in-app notice list and idempotent read marker; CSRF on mutation, no acknowledgement of money/incident resolution |
| `GET /account/notification-preferences`, `PATCH /account/notification-preferences` | Own revision-checked event/threshold/consent policy; cannot select another user's recipient or an unqualified channel |
| `POST /account/notification-recipient/begin`, `POST /account/notification-recipient/verify` | Optional qualified channel only; fresh customer proof + CSRF, bounded single-use verification and destination controls under HD-13 |
| `GET /models` | Workspace-authorized model/pricing projection, revisions and evidence-backed availability |
| `GET /models/status` | Customer session; bounded authorized [model history](#model-status-history). No private inventory, other workspace metrics or active checks |
| `GET /keys`, `POST /keys`, `PATCH /keys/{id}`, `DELETE /keys/{id}` | Own masked keys, narrowing policies, one-time issue and revocation. Look up a lost issuance by `operation_id` in the scoped list; reissue is a new confirmed issuance referencing the owned original, not implicit revocation |
| `GET /wallet`, `GET /wallet/entries` | Authoritative current balance and paginated posted history; holds distinguished from charges |
| `GET /payments/methods` | Customer session: enabled, eligible methods in display order, with safe IDs, labels, effective credit limits, ordered quick amounts and custom-entry availability for this channel; no credentials, drafts or merchant diagnostics |
| `POST /payments/quotes` | Own wallet and eligible method: validate the requested credit against the published amount rules and method limits. Return a bounded quote reference binding the amount-policy revision, method revision, merchant, payable amount/currency, wallet credit, applicable fees and expiry. Review does not open checkout, hold funds or post credit |
| `POST /payments`, `GET /payments`, `GET /payments/{id}` | Confirm a matching unexpired quote with `operation_id`; recover via own history (including lookup by that operation) or payment ID. Replaying a committed intent returns its status even if the original quote has since expired. History and detail indicate whether checkout can resume, without exposing a merchant checkout link |
| `POST /payments/{id}/resume` | Customer session + CSRF, own wallet only. Recheck the invoice on its original merchant binding and reference. Return the adapter-approved handoff for that same payable invoice, at the accepted amount and currency; if payability cannot be confirmed, return status without a handoff. Never create a quote, intent, invoice or wallet posting |
| `GET /payments/{id}`, `POST /payments/{id}/refund-requests` | Own payment with wallet entry ID and latest refund case; requesting cannot post a refund/credit directly |
| `POST /webhooks/payments/{provider_id}` | Selected provider's signature/merchant verification; no browser session trust |
| `GET /usage/summary`, `GET /usage/timeseries`, `GET /usage/breakdown` | Authorized filters/snapshot/coverage; independent current wallet/activity |
| `GET /usage/requests`, `GET /usage/requests/{id}` | Own paginated safe detail, localized-message key, stable code, coverage, execution/accounting status and safe next action; never the owner diagnostic DTO |
| `GET /usage/reports/{id}/csv` | Direct export of an active bounded request report with the same filters and UTC bounds; session, workspace and expiry rechecked on download |
| `POST /usage/exports`, `GET /usage/exports/{id}` | Bounded scoped CSV job/status/download; authorization checked again on download |
| `GET /requests/{id}/status` | Original workspace/key inference authority; safe duplicate/recovery status, no response replay |
| `/admin/*` | Owner session; paired Desktop only for the explicit three-area operation allowlist, not wildcard admin authority; action matrix and fresh-proof requirements below |
| `POST /admin/connections/imports`, `GET /admin/connections/imports`, `GET /admin/connections/imports/{id}` | Owner or explicitly scoped device: prepare a bounded preview, find own recent operations after a tab closes, read safe per-item state. No provider check, admission or sale on status read |
| `POST /admin/connections/imports/{id}/confirm`, `POST /admin/connections/imports/{id}/cancel` | Confirm reviewed item IDs and revision with one operation identity; cancel only uncommitted work. Reconcile ambiguous staged items before releasing a fence; a cancel cannot undo an acknowledged import |
| `GET /admin/usage/requests`, `GET /admin/usage/errors` | Owner session: retained logical requests, exclusive cursor and rolling or exact UTC bounds. The Errors read uses canonical outcomes and independent accounting/evidence issues. Scoped Desktop requires its own reduced projection. |
| `GET /admin/usage/requests/{id}` | Owner session: retained request timeline, physical attempts and live domain-derived accounting. Scoped Desktop must not receive customer-wallet/payment-management data. GET never probes/reexecutes. |
| `GET /admin/operations/status` | Owner current condition/coverage; scoped Desktop receives only the server/pool-health summary needed by Overview |
| `GET /admin/operations/events` | Owner browser session, paginated retained job/component events; absence of events is not a health proof |
| `GET /admin/operations/diagnostic-sessions`, `GET /admin/operations/diagnostic-sessions/{id}` | Owner browser session metadata: scope, original expiry, budget consumption, state and stop reason; no raw log download or paired Desktop access |
| `POST /admin/operations/diagnostic-sessions` | Owner browser session + CSRF + fresh passkey; audited reason, finite scope/budgets and idempotent start operation; repeated operation returns the same unextended session |
| `POST /admin/operations/diagnostic-sessions/{id}/stop` | Owner browser session + CSRF; idempotent stop, no capture extension or inference/payment effect; automatic expiry/revocation also enforced server-side |
| `GET /admin/operations/readiness`, `GET /admin/operations/service` | Owner browser session current checklist/mode/revision and missing prerequisites; reads do not run qualification checks; Desktop uses its reduced Overview status projection instead |
| `POST /admin/operations/readiness-checks` | Owner + fresh passkey + CSRF; named allowlisted check, scoped operation and explicit approval of any cost/external effect; no arbitrary command/URL |
| `POST /admin/operations/service/transitions` | Owner + fresh passkey + CSRF; expected mode/revision, operation ID and reason; backend enforces readiness/admission fence and closure rules |
| `GET /admin/operations/announcements`, `POST /admin/operations/announcements` | Owner list/create a bounded in-app service notice or a staged change referencing a valid domain revision; create requires CSRF, no automatic publication |
| `PATCH /admin/operations/announcements/{id}`, `DELETE /admin/operations/announcements/{id}` | Owner + CSRF and expected revision; edit/discard an unpublished draft only. Fanout reads the immutable published revision, never an editable draft |
| `POST /admin/operations/announcements/{id}/publish`, `POST /admin/operations/announcements/{id}/cancel` | Owner + fresh proof + CSRF; revision/operation guards and notice-period checks, no generic rewrite of published tariffs |
| `GET /admin/settings/notifications`, `PUT /admin/settings/notifications` | Owner-only qualified channel policy, masked configuration and vault references; mutation requires fresh proof/CSRF/outbound-policy validation |
| `GET /admin/settings/payments` | Owner-only adapter catalog, masked connections, method order, top-up amounts, open-invoice limit and readiness. H08/H15 own draft, check, publish, reorder and disable operations: revision-checked, CSRF-protected and audited, with fresh proof for merchant or publication changes. No checkout or account credit from a settings action. |

The customer-status and inference-status surfaces can share a service projection,
not an ambiguous credential fallback. A customer session accesses its own usage;
an inference key cannot acquire account/payment/admin privileges by calling a
different path. Setup/recovery/passkey-registration and Desktop pairing endpoints
must have complete H05 ceremony specifications before exposure; none is a public
owner-reset shortcut.

Admin resource families are `/admin/connections`, `/admin/pools`, `/admin/models`,
`/admin/tariffs`, `/admin/customers`, `/admin/payments`, `/admin/usage`,
`/admin/settings`, `/admin/operations`, `/admin/audit`, `/admin/devices` and
`/admin/credentials`. H05/H07/H08/H15 enumerate each allowed verb/action, including
park/drain/trash/restore, revision conflict, secret export and fresh verification.
No generic arbitrary-SQL, arbitrary-URL fetch or generic credential-read endpoint.

H07 owns bounded start/status/complete/cancel operations for connection authorization
attempts and the connector-specific import contract under `/admin/connections`.
For SC-09, bind preview, confirmed selection, per-item result and safe status
lookup to the same actor and installation; device transfer additionally binds its
pairing and allowed destination. Persist enough nonsecret result data to reconcile
after a lost response, not a replayable credential file. H03/H07/H15 define the
durability, expiry and fencing before exposing confirmation.
They follow [provider onboarding](#server-side-provider-connection-onboarding), not
the customer/admin identity-login API; status responses expose no exchange secrets.

The [Desktop scope](#desktop-hosted-scope) is an operation and response-projection
allowlist, not access to entire resource families. H05/H07/H12/H15 and HD-12 must
specify the exact permitted reads, connection-addition/transfer and pool-membership
mutations. All other actions are denied to paired-device credentials even when
the owner could perform them through a separately authenticated browser session.

Wire rules for the Hosted control API:

- Successful object responses use `{data, meta}`. `meta` includes `request_id` and
  `api_version`; paginated/report responses additionally expose applicable cursor,
  `as_of`, normalized filters, coverage/completeness and metric revision. Specify
  absent versus null per field. Do not require report metadata on unrelated routes.
- Errors use `{error: {code, message, request_id, retryable, field_errors?}}` with
  stable machine codes and safe human text. Field errors never echo secrets/bodies.
  Inference keeps its protocol-specific envelope and the documented idempotency codes.
- Use 400 for malformed input, 401 for missing/invalid authentication, 403 for
  authenticated denied actions, 404 for absent or unauthorized object lookup,
  409 for revision/state/idempotency conflicts, 429 for bounded limits and 503 for
  unavailable dependencies/capacity. Each route fixes its exact cases; errors never
  become empty successful charts or an HTML frontend fallback.
- Monetary/counter strings follow HC-02; prices carry tariff/rounding revisions.
  Cursor pagination has a deterministic `(created_at, id)` tie-breaker, enforced
  page bounds and a scope/filter-bound cursor. No unbounded `all=true` query.
- Mutations validate server-side scope, CSRF, state and expected revision. Payment,
  refund and other effectful retries have durable operation identities. A repeated
  key-issuance operation cannot reveal an old secret or silently create extra keys.
- Sensitive responses use `Cache-Control: no-store`. Scrub query strings on auth
  callback paths; authorization codes must not enter edge/application/access logs.
  Redirect destinations are allowlisted same-origin paths, never arbitrary URLs.
- Use distinct Secure/HttpOnly customer/admin cookies with no parent-domain scope.
  Fix exact cookie names/paths/SameSite/CSRF transport in H05/H06; callbacks must be
  tested with the selected flow rather than weakened to wildcard credentialed CORS.
- Export CSV cells as data, neutralize spreadsheet-formula injection, enforce row,
  byte, time and expiry bounds, and recheck account/authorization on retrieval.
  No public permanent download URL or raw request/response content.

For every route above, H02 and its domain package complete a field-level contract
before code: JSON name/type/required/null/default, range/length/enum, source of truth,
visibility, example and each status/code. Control mutations specified with durable
operation identities bind an opaque UUID `operation_id` to normalized input and
caller scope; conflicting reuse returns 409, not a second effect. Inference retains
its `Idempotency-Key` contract; auth ceremonies, provider webhooks and idempotent
read markers keep their own challenge/event/object identities. Revisioned mutations
name `expected_revision`, rejected before effects when stale. Define lost-response
recovery for success and unknown outcomes.
Unknown optional capabilities render unavailable, never silently activate a channel,
test, model or recovery action. Schema examples use synthetic IDs and no secrets.

### Configuration and local data placement

The planned non-secret configuration file is
`/etc/zenith-relay-hosted/config.toml`; the Linux application data root is
`/var/lib/zenith-relay-hosted`. These are Hosted defaults, not changes to the
Desktop data directory. Host and container paths are mapped explicitly
in the deployment recipe. Never migrate existing SQLite or vault data on startup.

| Configuration boundary | Target names and validation |
| --- | --- |
| Bootstrap path | `ZENITH_RELAY_HOSTED_CONFIG`; path to typed configuration, fail clearly when missing/invalid |
| Database credential | `ZENITH_RELAY_HOSTED_DATABASE_URL_FILE`; restricted runtime file, no credential in committed TOML or logs |
| Vault key | `ZENITH_RELAY_HOSTED_VAULT_KEY_FILE`; versioned independently recoverable key; wrong/missing key fails closed |
| Digest key | `ZENITH_RELAY_HOSTED_DIGEST_KEY_FILE`; separate purpose/version for keyed request/token digests; rotation must preserve open claims |
| Server | `server.bind`, `server.public_origin`, `server.trusted_proxy_cidrs`; explicit HTTPS origin and bounded trusted proxy set |
| Database | `database.max_connections`, `connect_timeout_ms`, `lock_timeout_ms`, `statement_timeout_ms`; bounded independently of stream count |
| Owner authentication | `auth.admin_rp_id`, `admin_origins`; fixed origin/RP binding and documented recovery path |
| Admission | `limits.global_concurrency`, `workspace_concurrency`, `workspace_queue_length`, `queue_wait_seconds`, `prepared_body_bytes`, `request_duration_seconds`; positive finite qualified bounds |
| Accounting | `accounting.reconciliation_seconds`, `estimation_policy_version`; finite deadline, published policy, no implicit overdraft |
| Workers | `workers.concurrency`, `lease_seconds`, `max_attempts`, `max_backoff_seconds`; explicit job-kind overrides and jitter policy |
| Continuations | `continuations.payload_enabled`, `payload_ttl_seconds`, `session_bytes`, `workspace_bytes`, `global_bytes`; default disabled, no unlimited global bound |
| Retention | `retention.request_days`, `aggregate_days`, `log_days`, `audit_days`; defaults from initial data policy, reviewed financial policy separate |
| Notice retention/delivery | `retention.notice_days` = 30, `notification_delivery_days` = 14; `notifications.max_attempts`, `max_backoff_seconds`, `delivery_ttl_seconds`, `fanout_batch_size`, `cooldown_seconds` require finite qualified values; external delivery disabled without a qualified channel |
| Service changes/readiness | `service.change_notice_seconds` defaults to 86400 for ordinary increases/removals, subject to reviewed terms; `readiness.check_validity_seconds` is a typed per-check map; persistent service mode is never overwritten from TOML on restart |
| Outage access | `operations.outage_contact`; validated public destination independently reachable without this backend, tested before public activation |
| Abuse/outbound policy | `abuse.login_window_seconds`, `login_max_attempts`, `registration_max_attempts`, `counter_ttl_seconds`, `max_counter_entries`; `network.outbound_policy`; positive finite limits and purpose-scoped destination/redirect/credential rules, no implicit unrestricted fetch |
| Normal diagnostics | `diagnostics.event_max_bytes`, `queue_max_events`, `queue_max_bytes`, `ingest_events_per_second`, `storage_max_bytes`; positive finite bounds qualified in H16; optional detail sheds load, mandatory financial/audit evidence follows its own durability rules |
| Extended capture | `diagnostics.capture.default_seconds` = 600, `max_seconds` <= 1800 (default 1800), `max_active_sessions` = 1 for launch; `max_events`, `max_bytes`, `max_events_per_second` required finite qualified bounds; missing capture bounds disable capture, never normal error reporting |
| Public presentation | `branding.name`, `locale`, `logo`, `support_url`, `terms_url`, `privacy_url`; typed/escaped allowlisted public projection |

Every setting needs type/unit, allowed range, required/default value, secrecy,
reload/restart behavior and invalid-input example before its package implementation.
Use documented launch defaults where already specified; measured global/output/
duration values remain required deployment inputs, not guessed numbers. Database,
vault key, digest key, RP ID and public origin changes require an explicit migration
or restart/revalidation procedure; no silent hot reload of identity boundaries.
Capture duration must be positive and no greater than the configured hard maximum.
Invalid configured limits fail validation; missing optional-capture budgets make
capture visibly unavailable with a safe reason. Capture controls cannot override
the installation's normal diagnostics/storage ceiling or enable payload recording.

PostgreSQL data lives in its private persistent volume, not under frontend assets.
Application vault generations/manifests live in the protected data root; optional
continuation payloads have a separate encrypted, bounded namespace. Secret material
never lives in `hosted/web/`, images, backups of plaintext config or command-line arguments.
Rotating a secret creates a new reference/version before changing live consumers.
Provider/login/payment secrets are delivered through the audited vault boundary,
not copied into the public runtime configuration.

### Design decisions that must not be guessed

The role below owns the answer; one person may fill several roles. Record the
responsible person, exact scope, revision, evidence reference and when it must
be rechecked. A design choice can unblock writing its adapter; only verified
results can authorize real use. Fixture values are not decisions for a real
installation. Stage numbers refer to the [delivery stages](HOSTED_RELAY_ROADMAP.md#delivery-stages).

| Decision | Accountable role | Required answer or evidence | First gated use; behavior if missing |
| --- | --- | --- | --- |
| HD-01 | Installation operator | Document the basis for shared paid use of the exact credential class, provider/product, route and region; record review conditions. A working personal login is not evidence. | Assess feasibility at H00. Before H07 real paid qualification or any sale, exclude each unapproved scope. Changing the proposed first offer requires a product decision. |
| HD-02 | Operator responsible for payments | Select the installation's merchant accounts; verify quote/currency, payment evidence, idempotency, refund/dispute and receipt behavior for each offered method, environment and channel. | Before implementing a real H08 adapter; before public funding, demonstrate funding and refund for its offered scope. A bot template or merchant credential alone does not qualify it. |
| HD-03 | Identity/security lead | Select maintained WebAuthn/identity libraries. Specify RP/origin, cookie/CSRF, recovery and supported authenticators; verify issuer, subject and callback contract for each login method. | Stage 2 H05 needs the owner ceremony; stage 3 H06 needs its fixture identity contract. A real login method stays disabled until separately verified. |
| HD-04 | Persistence lead; finance owner approves postings | For each new migration specify fields, tenant keys, uniqueness, indexes, deletion and upgrade. For each money operation specify balanced postings and recovery; decide how spent-fund reversals become obligations. | Stage 2 H02 needs only its own schema. H06 registration needs the H04 empty-wallet transaction; H04/H08 must resolve the affected postings before money moves. Unresolved reversals block real funding, not bootstrap. |
| HD-05 | Security/storage lead | Verify the Desktop vault primitive or its extraction against protected generations, commit cut points, key/digest rotation and a compatible recovery manifest. | Before H03 writes protected material. H16 must recover it on an isolated host before launch; an unavailable key cannot enable dispatch. |
| HD-06 | Accounting lead; core lead supplies sizing evidence | For the exact offered operation/band prove post-expansion input and finite output bounds, disjoint usage components, pricing vectors and disclosed missing-usage/estimate policy. | Stage 3 H09/H10 uses known fixture bounds and tariff. No real paid variant until its bounds and settlement are proven; an unqualified media/tool path stays closed. |
| HD-07 | Operations lead | Define finite body, queue, member/global, duration, job and report/export limits. Measure them under the workload and failure recovery the operator intends to support. | H09/H10/H12 enforce finite limits even on a test installation. H16/H17 need measurements before publishing capacity or opening traffic; the stage 1 one-slot fixture is not a capacity claim. |
| HD-08 | Control API lead with web/client owner | For each exposed operation fix actor, fields, errors, cookies/scopes, version compatibility and, for mutations, idempotency/recovery semantics. Identify supported clients and transports. | H02/H13 must agree on the first public read in stage 2; later handlers qualify their own DTOs and clients. Unspecified methods stay unregistered, not forwarded. |
| HD-09 | Installation operator with finance/privacy review | Identify the operator, jurisdiction, support contact, refund terms, retention/deletion duties and what customers must see before paying. Review the actual service and merchant; no universal period or policy is assumed. | Before enabling H12 financial purge or H17 commercial launch. No real customer onboarding or funds on the strength of sample terms. |
| HD-10 | Release/operations lead | Pin compatible artifacts and schema, origins, paths, trusted edge/outbound policy and secret delivery for the intended deployment. | Stage 2 H02 fixes its local bootstrap pair/config. H16 verifies the actual edge and packaging before real qualification; no public exposure with development defaults. |
| HD-11 | Recovery operator | Supply independently recoverable off-host backup/key references. Demonstrate isolated restore, revocation replay and reconciliation of payments and dispatches beyond the restore point; measure loss/time. | H16/H17 before paid launch or reopening after restore. Without proof, keep public admission closed and reconcile existing obligations. |
| HD-12 | Relay core/Desktop lead; web lead verifies parity | Map supported credential families, local-pool controls and Desktop pairing versions to exact permitted server operations/projections. Specify how unsupported transfer or an unresolved proxy is refused. | H01 starts with a static fixture only. Before H05/H07/H12/H15 expose an actual connector, paired read, import or mutation, prove that slice and deny all others. |
| HD-13 | Operator responsible for notifications | If external delivery is offered, verify recipients/consent, allowed template fields, delivery/idempotency, bounded retries and retention with that channel. | Before H06/H12 send an external customer notice; otherwise leave the channel off. In-app notices and independent operator alerts do not depend on it. |

## Delivery and acceptance

The order is in the [Hosted roadmap](HOSTED_RELAY_ROADMAP.md#delivery-stages). Paid launch remains
blocked until acceptance establishes:

- A new customer can sign in through each supported Google/Telegram/configured
  OIDC integration, pay, create a key, call an enabled model and inspect the correct
  charge; linking/recovery/suspension never creates another wallet or admin role.
- Web owner inventory/pool controls match local Desktop semantics, including
  parking, enable/disable, prices and live activity; customer screens never expose
  inventory. Hosted Desktop implements only its three-area scope, with tests for
  allowed reads/addition/membership actions and rejected web-only API calls.
- User-Agent labels remain untrusted and bounded; usage attribution comes from
  authenticated identities. Retention, deletion and restored revocation are tested.
- Versioned installation/upgrade/rollback, TLS, offered streaming/transports,
  alerting, backup and isolated restore meet the launch objectives under the
  qualified workload. Unoffered operations and upgrades produce no provider send.
- Linked channels share identity/state; tenant/key isolation prevents enumeration,
  cross-workspace reads, grants, continuation reuse, and spending.
- Physical capacity/quota is counted once; model/context projection matches grants
  and exact runnable provider surfaces without cross-vendor substitution.
- Real PostgreSQL tests cover same-wallet contention, duplicate closure/payments,
  failed postings, rollback, deadlock/timeout retry, and lost commit acknowledgements.
- Cancellation/late usage/restart never settles twice. At the saved deadline
  every outstanding hold [settles or releases on the available evidence](#reconciliation-and-estimated-settlement);
  an uncertain execution or provider expense need not become known.
- Supported SDK JSON/SSE duplicate requests receive valid non-2xx envelopes,
  including ignored retry hints, without another hold or generation.
- For offered resume, graceful/abrupt restart restores only valid committed turns,
  including turns awaiting tool results. Reject pending, expired, revoked or
  foreign state. Where resume is not offered, attempts fail before send without
  retaining payloads or pretending the original response can be replayed.
- Routes have reviewed commercial permission, region, billable usage, tariff,
  finite bounds, and real-client/provider qualification.
- Transfers and isolated backup/restore recover without deleting source credentials;
  logs, exports, operator views, and metrics expose no forbidden content/secrets.
- The [diagnostics contract](#errors-and-production-diagnostics) is implemented in
  both owner and customer surfaces: cancel/recovered-attempt noise is distinguished
  from failed logical execution, and completed output can still have pending money.
- Unknown causes, unavailable/expired detail and dropped telemetry are explicit;
  malicious upstream errors cannot inject secrets/content into UI, logs or support
  summaries. Foreign request IDs do not disclose a customer's records.
- Error reads, alert acknowledgement and browser recovery never probe, regenerate
  or mutate money. Temporary capture enforces owner proof, finite budgets, expiry,
  revocation and restart-safe limits without exposing a public debug console.
- SC-01–SC-09 have step-level contract and acceptance evidence. A stale readiness
  check cannot open sales; maintenance/closure fence new effects while preserving
  existing settlement, refunds, history and signed callback handling.
- Synthetic money examples reconcile exactly, including refund/spend races and
  repeated acknowledgements. Notice failure/read state cannot change money or grants.
- Scheduled price/model changes preserve held tariffs and announce customer impact;
  missing activation evidence blocks affected new work rather than silently using
  an obsolete price. Service migration/closure preserves obligations and revocation.
- Abuse, key revocation, cross-workspace access, outbound destination changes and
  notification-recipient verification have explicit negative tests before exposure.
