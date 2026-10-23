# Hosted Relay Architecture

**Target product contract, not shipped behavior.** Current Desktop contracts are in
[PLANNING.md](../project/PLANNING.md); implementation order is in the [Hosted roadmap](HOSTED_RELAY_ROADMAP.md#delivery-stages).

Navigation: [design gates](#specification-before-implementation) ·
[product](#decision) · [website](#customer-website) ·
[web design](HOSTED_RELAY_WEB_DESIGN.md) ·
[selected reference patterns](#reference-derived-implementation-decisions) ·
[scenario map](#step-by-step-scenario-map) · [notifications](#customer-notifications-and-delivery) ·
[identity](#identity-and-account-channels) · [administrator access](#passkey-only-administrator-access) ·
[execution](#request-lifecycle) · [money](#customer-balance-and-provider-expense) ·
[errors and diagnostics](#errors-and-production-diagnostics) ·
[data/stack](#data-and-deployment) · [operations](#initial-operations-contract) ·
[files/schema/API map](#implementation-contract-map) ·
[unresolved decisions](#design-decisions-that-must-not-be-guessed) ·
[acceptance](#delivery-and-acceptance).

## Specification before implementation

This is intentionally a detailed implementation specification, not a short product
pitch. Keep the detail needed to implement and review a behavior without inventing
its data ownership, failure policy or customer meaning during coding. Length is not
a reason to remove constraints. Use navigation, stable identifiers and one canonical
definition per subject instead of parallel architecture documents or copied rules.

This document owns the target contracts; [the detailed delivery plan](HOSTED_RELAY_ROADMAP.md#hosted-implementation-work-packages)
owns ordered work packages, dependencies and acceptance. `PLANNING.md` continues to
describe implemented behavior. A specified table, endpoint, file or command below
is a target, not evidence that it exists. Do not mark a capability shipped because
its design, mock screen or happy-path test exists.

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

Each package has a design gate before code and a separate implementation acceptance
gate. An unresolved dependency is named and blocks the affected behavior, not every
independent documentation task. Do not guess merchant details, commercial permission,
real tariffs, capacity or recovery results to make the plan look complete. Record
the decision owner, required evidence, consumers and acceptance condition instead.

The goal is to remove avoidable ambiguity, not to promise that prose can eliminate
all implementation defects. A discovered contradiction updates the canonical
contract and affected package/checks together. Tests, failure injection and actual
restore/provider qualification remain required even after the design is complete.

### Step-by-step scenario map

Use this map as the implementation reading order, not a second set of domain rules.
Each scenario must connect the screen/action to a named API, server authorization,
state transition, transaction, external effect, visible result and recovery check.

| Scenario | Canonical walkthrough | Owning packages |
| --- | --- | --- |
| SC-01 | [Owner setup and readiness](#owner-setup-and-readiness) | H02, H03, H05, H07, H08, H15, H16 |
| SC-02 | [First customer request](#first-customer-request-walkthrough) | H06, H08, H09, H10, H13, H14 |
| SC-03 | [Money scenarios and worked checks](#money-scenarios-and-worked-checks) | H04, H08, H09, H10, H12, H14 |
| SC-04 | [Errors and investigation](#errors-and-production-diagnostics) | H12, H14, H15, H16 |
| SC-05 | [Customer notifications](#customer-notifications-and-delivery) | H03, H06, H08, H12, H14, H16 |
| SC-06 | [Price and model changes](#price-and-model-change-walkthrough) | H04, H07, H09, H12, H14, H15 |
| SC-07 | [Maintenance and recovery](#maintenance-and-recovery-walkthrough) | H03, H05, H08, H10, H15, H16 |
| SC-08 | [Service closure and migration](#service-closure-and-migration-walkthrough) | H03, H04, H08, H12, H15, H16, H17 |

Before coding any step, its owning package records: preconditions and actor; exact
input/output types and limits; state/version checked; records written in one commit;
external calls outside that commit; operation identity and timeout; customer/owner
text; failure/restart behavior; and named acceptance cases. A happy-path screenshot
or endpoint name alone does not finish the step. Reuse the existing field/API/state
inventories; add missing definitions there instead of creating competing copies.
Separate an approved product default, a measured deployment value and an externally
verified commercial fact. An unresolved fact blocks only the dependent step.

## Decision

Build a multi-user service around `relay-core`, initially as one modular backend
and deployment unit with bounded workers. PostgreSQL owns durable hosted state;
provider secrets use an encrypted vault with keys outside the database.

Keep the source in the existing `zenith-relay` repository. The default landing page
and customer cabinet are a separate `web/` application with their own build/tests
and optional deployment artifact; operators can instead connect their own site.
See [application boundaries and deployment choice](#one-repository-and-deployment-choice).

Reuse the existing protocol, routing, scheduler, and stream semantics. Do not
fork the codec engine or scatter Hosted-mode conditions through Desktop.
Use the planned `relay-hosted/` composition defined in the implementation map.
The retired personal-server runtime is removed; Hosted is a separate implementation.

### First release and commercial model

The first public release is a complete paid product, not an administrator-panel
demo. The operator supplies account/API-provider capacity; customers buy model
token usage through Relay keys, not provider credentials or ownership of an
account. Provider quota limits available supply; it is not a fungible customer
token balance. Customers see model tariffs, their money balance, limits, usage,
and charges. Never promise unlimited capacity or sell unverified quota estimates.

ChatGPT is the initial account family. Claude and other account families are
subsequent connectors, introduced into Desktop and server together through the
shared lifecycle. A generic Messages endpoint is not a Claude subscription
connector. Each family must qualify its own sign-in/import, token renewal,
entitlements, quota, execution, usage, recovery, and commercial permission.
Customer Google/Telegram sign-in is separate from these upstream credentials.

Public release includes customer registration/sign-in, account recovery, model
catalog and prices, payment/top-up, scoped request keys, working inference,
usage/receipts, owner management, support diagnostics, and tested operations.
Implementation milestones can be private and incremental; none alone satisfies
this release definition. Customer BYOK charging, account resale, subscription
bundles, and a marketplace are not part of this first offer.

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
| Issue/revoke request keys | Own workspace; suspend customer keys with an audited reason | Own workspace only |
| Manage identity methods, payments, tariffs, limits and security | Yes; privileged changes require fresh verification | Own linked identities, sessions and account settings only |
| Credit, refund, or adjust a wallet | Through the accounting journal with actor/reason/reference | Request a top-up/refund; never choose a journal credit |
| Read/export provider secrets | Explicit confirmed operation, fresh verification, no-store and audit | Never |

Suspension blocks new requests and key issuance without deleting financial
evidence. Revocation is checked at admission; cancellation of already dispatched
work is explicit and cannot imply zero provider expense. Customer deletion follows
the retention policy, not a cascading deletion of requests or money records.

### Operator interface and customer cabinet

The Hosted operator interface preserves the current local-pool controls and
semantics, not a simplified replacement. Desktop remains an operator client;
the server-side interface has no local application launcher/profile attachment.
Preserve Desktop local application integrations and LAN access.

| Area | First-release responsibilities |
| --- | --- |
| Overview | Existing pool overview and availability, plus active logical requests/physical attempts by model, errors, customer charges and separately evidenced provider expense |
| Connections / credential inventory | Saved account connections and API providers, labels/notes, auth and refresh state, proxies, quota/models, automations, import/export, reauthentication, trash/recovery; saved does not mean admitted to the pool |
| Pool | Existing membership, order/rotation, enable/disable, model rules, context/reasoning/service-tier settings, prices, quota, tool and request-timezone policies; reuse shared validation and capability checks |
| Access | Public API addresses, protocols, key scopes/limits/rotation/revocation; no Applications section in the Hosted panel |
| Usage | Existing request/attempt, timing, tokens, model, origin and error detail, plus authenticated workspace/user/key attribution, a client User-Agent label, settlement and filtering; Errors tab and request timeline follow the diagnostics contract |
| Customers and payments | Customer status, limits, balances, top-ups, refunds, receipts and auditable adjustments; no provider secrets or prompt/response bodies |
| Settings and operations | Sign-in providers, public origins, security, retention, backup/restore status, health and update status |

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

## Reference-derived implementation decisions

Adopt the selected functional patterns and failure cases below, not another
product's frontend or runtime. These are inputs to the target contract and named
implementation packages, not claims of shipped functionality. Existing Relay
contracts and official provider specifications remain authoritative; a reference
implementation, its popularity or a passing upstream test does not prove our
money, security or protocol behavior.

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
   Use the canonical `/app/billing` and `/app/settings` routes. Implement this with
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
existing package tests instead of creating another standalone test framework.

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
affected callers/data boundaries and named acceptance tests. Source review is
not runtime qualification. H00 reviews this mapping; H17 verifies the resulting
behavior rather than accepting an upstream screenshot or test result as evidence.

Default to Relay-owned implementation of these requirements, reusing our core and
existing suitable utilities. No imported gateway, Go/Python sidecar, mandatory
Redis, external analytics store or additional billing service is introduced by
this reference inventory. Keep the recommended first-production stack unchanged.
In particular, an optional reference implementation's fail-open in-flight cache
reservation is not our durable financial admission contract.

If an isolated non-visual source reuse is later proposed, identify its exact files/
revision, dependencies and rights holders; review applicable licenses, retain
notices, and handle it as third-party material under [CONTRIBUTING.md](../../CONTRIBUTING.md#license-and-contributor-agreement).
Do not describe it as newly authored/assigned Relay code. Upstream access and
commercial permissions remain separate HD-01/HD-02/HD-09 launch decisions; an open
source license or working connector does not close those decisions.

## Customer website

This section owns the first-release customer-web specification, not shipped UI
or a user guide to existing screens. The website is where a customer registers,
funds access, obtains connection credentials, understands usage and manages their
account without installing Desktop. It is not the operator pool dashboard.
Implementation slices are tracked in the [Hosted roadmap](HOSTED_RELAY_ROADMAP.md#customer-website-implementation-slices).

### Frontend application boundary

Build the default Relay landing page and customer cabinet as the `web/` application
inside the existing `zenith-relay` repository, against the Hosted API. This is the
target layout, not a claim that the application is implemented. Do not create a
separate Git repository for the initial website or replace the Desktop `src/` app.
Reuse transport-independent Relay formatting, i18n, accessible interaction and
chart utilities where suitable; do not inherit Desktop page layouts/styles or
import Tauri commands or local account state. The backend must also support an
operator's own website without deploying the default web application.

### Original Relay design

The detailed [web design specification](HOSTED_RELAY_WEB_DESIGN.md) owns visual
tokens, responsive layouts, screen passports and interaction acceptance. This
architecture remains authoritative for routes, permissions and data semantics;
the design specification is not a second backend contract or shipped UI.

Design the Hosted website and operator web surfaces from scratch as Relay's own
product: page compositions, typography, spacing, colors, icons/illustrations,
chart presentation, responsive behavior and interface copy. Preserve existing
local-pool capabilities and meanings, not a requirement to reproduce its pixels.
Do not redesign the existing Desktop application as part of this web work.

External projects inform functional requirements, failure scenarios and technical
patterns, not visual deliverables. Do not copy their screens, CSS/themes, branded
assets, page components or marketing copy, including by reproducing screenshots.
An original design does not require replacing suitable accessible primitives,
chart libraries or existing Relay utilities; review their licenses and retain
applicable notices. New styling never transfers third-party copyright to Relay.

H13 defines the original web design tokens, components and interaction states;
H14/H15 use them for customer/operator surfaces. Review desktop/mobile, both
locales/themes, keyboard use and loading/empty/error/pending states before visual
acceptance. Functional contracts below define information and permitted actions,
not a fixed external layout. Branding configuration customizes our design rather
than re-skinning another project's frontend.

### One repository and deployment choice

Use one repository with independent applications: the existing Desktop/server/core
areas and the planned `web/` customer frontend. Give `web/` its own package manifest,
development/build commands, tests and deployable static/container artifact. Separate
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
| Planned `zenith-relay/web/` application | Default landing page, customer cabinet, branding configuration, typed API client, web tests, separate build artifacts and frontend setup/contribution guides | Database access, wallet calculation, provider secrets, customer authentication verification or a second backend |
| Installation configuration | Chosen frontend, domains, branding, legal/support links, registered login/payment integrations and pinned compatible artifacts | Changes to shared authorization, money or request-execution rules |
| Operator's custom website | Alternative customer presentation using the documented Hosted customer API | Direct access to the database, vault, operator management credentials or private engine interfaces |

Offer two explicit deployment paths:

1. **Default website:** install the backend and a pinned compatible web artifact
   built from `web/`, configure the domain/branding and integrations, then use the
   included landing page and complete cabinet. A normal operator should not need to fork
   code, compile the frontend or customize server internals to get a usable site.
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

Make `web/` a clear contribution area in the existing repository: app-specific
README/quick start, local development with synthetic fixtures, configuration
reference, compatibility information and tests. Reuse the repository's contribution,
security-reporting and release policies instead of maintaining another project.
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

Add `web/` inside Relay with the [original Relay design](#original-relay-design).
Reuse only explicitly identified utilities/dependencies with their notices, not
reference-project pages/assets. Review authored and reused source/assets, examples,
build configuration and release artifacts for secrets, customer data and
unconfigured external integrations before publication.

Reconsider a separate repository only when independent team ownership, access rules
or a genuinely separate release lifecycle makes it necessary. That is a future
decision with migration costs, not a prerequisite for custom sites, community
contributions, neutral branding or independent deployment.

### Deployment origin

The operator configures the installation's HTTPS origin through
`server.public_origin`; no particular domain is part of the product contract.
The default website and a custom website use the same Hosted API and authorization
rules. Branding and origin changes do not require a separate frontend fork.

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
After sign-in, open Overview in the customer's personal workspace.

Public `/` uses section anchors for explanation, models/prices, FAQ and support;
`/login` owns customer sign-in. The public price table uses `GET /public/models`
under the Hosted API prefix, never the authenticated customer model projection or
frontend constants. Publish only deliberately public qualified model descriptors,
supported operations/protocols and complete retail tariff/band revisions, including
effective times. This public projection is bounded and read-only, exposes no
customer grants, provider inventory, physical capacity or credentials, and is not
a promise that a signed-in customer can use every listed model. Customer access
and actual readiness remain separately checked. Missing/stale publication data
is visibly unavailable, not a free or fabricated price. H07 owns its field contract.

The following are target browser routes, not currently implemented API endpoints:

| Section | Target route | Content and primary actions |
| --- | --- | --- |
| Overview | `/app/overview` | Current available/reserved balance, usage and spend for a selected period, active requests, charts, important notices; top up, create a key, inspect usage. |
| Usage | `/app/usage` | Filtered logical request history and aggregates, safe request detail, settlement state and bounded CSV export. |
| Models and prices | `/app/models` | Authorized public models, supported protocols/capabilities and current retail tariffs; connection examples and availability explanations. |
| API keys | `/app/keys` | Create, label, restrict, rotate and revoke own keys; show masked metadata and last use, never a stored full secret. |
| Balance and payments | `/app/billing` | Available/reserved funds, top-up, payment status/receipts, wallet journal and refund/support entrypoint. |
| Account and security | `/app/settings` | Linked login methods, bounded sessions/revocation, language/theme and deletion/recovery guidance. |
| Connection and help | `/app/help` | Copyable base URL/model configuration with placeholder secrets, supported-client instructions, safe request-ID diagnostics and support. |

This table owns the target browser route names used by H13/H14 in the roadmap.
The current UI preview implements billing in `web/src/features/billing/`; API-key
management and account settings remain future work, not unused placeholder pages.
Browser route names do not rename the Hosted API's `/wallet` and `/account` resources.

Use one desktop navigation and a compact mobile equivalent. Header shows account,
current workspace, balance shortcut and sign-out. Start with the personal workspace;
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
| Overview | `/admin/overview` | Current capacity/activity, service mode and actionable incidents |
| Connections | `/admin/connections` | Accounts and API providers in separate tabs; saved inventory, not automatic pool admission |
| Pool | `/admin/pools` | Membership, drain/enable policies and existing shared pool controls |
| Models / tariffs | `/admin/models`, `/admin/tariffs` | Qualification, public publication and revisioned retail pricing; historical contracts preserved |
| Access | `/admin/access` | Customer key policy and explicitly paired devices/owner-access management; no Applications launcher |
| Customers | `/admin/customers` | Authorized identity/status/access inspection and guarded actions; no impersonation |
| Payments | `/admin/payments` | Payments, refunds, journal-linked adjustments and reconciliation |
| Usage / errors | `/admin/usage`, `/admin/usage/requests/{request_id}` | Logical request list, Errors tab, nested attempts and safe observed timeline |
| Operations / setup | `/admin/operations`, `/admin/operations/setup` | Incidents, diagnostics, readiness and guarded service lifecycle |
| Audit | `/admin/audit` | Scoped attributable administrative history; not an editable ledger |
| Settings | `/admin/settings` | Identity/security/integration/presentation policies and protected configuration |

Tabs/sheets use validated browser state and opaque authorized IDs; they do not
grant direct access to API families or secrets. Separate the Access page's device/
key inventory from security configuration in Settings; both use the same backend
authorities. H05/H07/H08/H09/H15 still specify every allowed action and permission.

### Overview and chart layout

1. **Current account strip:** available balance, reserved funds, current active/
   queued work and applicable concurrency/spending limits. Mark this as current
   state: changing the historical period does not backdate the wallet or live count.
2. **Period summary:** logical requests, completed/incomplete/failed/cancelled outcomes,
   normalized token usage and settled customer charges. Show unsettled requests
   and incomplete usage alongside totals rather than silently excluding them.
3. **Time series:** requests by outcome, input/cache/output token usage, customer
   charges, and observed successful-request latency. Give each its own units and
   legend; do not hide different scales behind an unlabeled combined graph.
4. **Breakdowns:** model and API-key tables with request counts, supported token
   components and charges. A chart/table selection opens Usage with matching filters.
5. **Recent activity and notices:** recent own logical requests, pending payment/
   settlement, low balance, service degradation and actionable access restrictions.

Default to the last seven UTC calendar days including the current partial day;
offer Today, 7 days, 30 days and a bounded custom period within retained coverage.
Reports use explicit UTC half-open time ranges and server-generated buckets in the
first release. Label UTC and incomplete buckets; do not silently rebucket older
daily aggregates into the browser's timezone. Alternative reporting timezones need
compatible stored aggregation and daylight-saving acceptance before being offered.

Use hourly buckets for Today where retained detail supports them and daily buckets
for longer ranges. Coverage is authoritative: request detail expires under the
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

The model catalog comes from the Hosted authorized projection, not a hardcoded
frontend list. Show public model identity,
supported protocol/capabilities, current price per million tokens and relevant
context/cache/service-tier bands. Distinguish no grant, temporary capacity trouble,
disabled model and missing price without leaking pool capacity or account details.
New tariffs affect new reservations; past request detail shows its snapshotted price.

Key creation shows the full secret once after confirmation, only in transient page
memory with an explicit copy action. Losing it requires rotation, not reveal. The
key list contains prefix/label, scopes, limits, expiry, revocation and last use.
Requested permissions cannot exceed workspace grants; limits cannot multiply the
wallet or workspace budget. Confirm revocation and explain that it blocks new
admission without promising cancellation/refund of already dispatched requests.
Connection examples contain placeholders unless the user explicitly copies the
just-issued secret; never put keys in links, analytics, browser storage or logs.

### Customer journeys and exceptional states

- **New customer:** configured login -> atomic personal workspace -> empty Overview
  with connection guidance -> view models/prices -> top up/create a scoped key ->
  first permitted request -> visible usage and charge. Do not auto-generate paid
  test requests or fake sample usage in a real account.
- **Top-up:** server-issued payment intent -> hosted payment flow -> pending return
  screen -> server-confirmed journal credit -> refreshed balance/receipt. A browser
  redirect, provider UI screenshot or repeated refresh never proves credit. Handle
  expired/cancelled/failed payments and delayed/duplicate notifications explicitly.
- **Troubleshooting:** select a request -> distinguish access, balance, capacity,
  execution and settlement problems -> copy safe request ID/context for support.
  No automatic resend of a paid or uncertain generation from the dashboard.
- **Security:** link another verified login, inspect/revoke sessions, rotate keys
  and sign out. Expired sessions clear sensitive UI state and return to login;
  a sanitized return path cannot become an external redirect or bypass authorization.
- **Restricted account:** explain suspension/read-only state, outstanding settlement
  and support options from server policy; never silently erase history or imply
  that deleting a key/account deletes financial obligations.

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
| 1. Sign in | Choose an enabled method; verify the registered identity flow and current service registration policy | Verified identity creates/loads one workspace and wallet; failed/cancelled login creates no funded account and grants no admin role |
| 2. Understand access | Read authorized models, current tariffs, key/workspace limits and service status | Distinguish unavailable model, missing grant, no capacity and missing balance; never advertise inventory as guaranteed supply |
| 3. Fund balance | Create one server-priced payment intent, complete its provider flow, then read that intent's status | Pending remains pending until the journal credit commits; returning/reloading cannot create another top-up |
| 4. Issue a key | Name and constrain one inference key under existing grants; confirm its intended client | Show the secret once with a warning to store it safely; lost issue response follows key recovery, not old-secret reveal |
| 5. Configure a client | Copy the configured public inference base URL, supported model and protocol-specific instructions | Show placeholders in examples; a connectivity/auth check is not a hidden paid generation |
| 6. Send a request | The external client submits through normal key admission, sizing, reservation and shared execution | The request has one logical ID and known hold; capacity/price/policy rejection explains what to change without blind retry |
| 7. Inspect usage | Read the own-workspace request and journal projection | Explain output state, observed tokens, hold, final charge or pending reconciliation; physical attempts never multiply the customer charge |
| 8. Get help or revoke | Copy a safe support reference or revoke the key/session through its own action | Support cannot impersonate identity from a message; revocation blocks new use, while in-flight evidence and settlement remain accountable |

If key issuance committed but the response was lost, identify the resulting key
by the same operation without disclosing its secret; explicitly revoke it and issue
a replacement only on a new confirmed action. A page reload is not that action.
Keep the first-request guide in `/app/help` and the empty Overview state, not a new
navigation section. Explain "available", "reserved", "charged" and "pending" in
RU/EN beside the relevant values, with the [money examples](#money-scenarios-and-worked-checks).

### Customer notifications and delivery

SC-05 uses a small in-app notice list inside Overview/Settings plus contextual
banners, not a new chat/helpdesk service. Financial history remains authoritative;
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
| Account security change | Affected user, after identity link/unlink, recovery or session/key revocation | Safe action/time and Settings link; never the new credential, identity token or complete key |
| Scheduled price/model/service change | Affected authorized workspaces, after approved publication | Exact effective time, affected public models and next step; unique announcement/revision, no private account inventory |

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

The frontend needs versioned Hosted contracts for session/workspace, catalog,
wallet snapshot/journal, payment intent/status, key management, request page/detail
and aggregate reports. These are required capabilities, not invented routes in
a shipped Desktop API. Reports return normalized filters, coverage,
bucket granularity, `as_of`, completeness and pagination metadata. Authorization
and monetary calculation stay on the server; user-supplied workspace/key/model IDs
are selectors to validate, never authority. Sensitive responses use no-store.

Keep server-revocable HttpOnly sessions with CSRF protection for mutations. Store
no tokens, keys, balances, request history or payment data in localStorage,
IndexedDB or service-worker caches; in-memory caches are keyed by authenticated
scope and cleared at sign-out. Only non-sensitive presentation preferences may
persist. Public static asset caching must not cache authenticated API responses.
Embed no third-party session replay or analytics that captures customer data.

Support Russian/English, light/dark themes, narrow/mobile screens and keyboard
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
- All customer routes work in both locales/themes on mobile and with keyboard;
  refresh/deep links preserve authorization and charts have equivalent tables.

Use mocked fixtures only in tests/development and label previews accordingly.
Full live identity/payment/provider acceptance remains a separate authorized release
gate. Chat/playground, image-generation UI, team management, saved conversations and
subscription bundles are not implied by this first cabinet specification.

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

These are Hosted trust-boundary requirements, not a finding that the current local
product is vulnerable. H05/H06/H09/H16 specify numeric bounds and fixtures before
exposure; a generic "rate limited" checkbox is not sufficient.

| Boundary | Required control and visible behavior | Required failure case |
| --- | --- | --- |
| Anonymous login/setup | Bound ceremony starts, challenge size/lifetime and failed verification; generic unauthenticated errors, no public owner reset | Repeated guesses or fabricated user identifiers do not enumerate users or lock a victim out indefinitely |
| Registration/linking | Verify identity before creating durable customer objects; bound attempts and pending challenges; linking never trusts matching email | Repeated callbacks create one identity/wallet and cannot consume unlimited storage |
| Customer API and inference | Enforce key + workspace + installation bounds, queue age, request/body/export sizes and fair scheduling | Many keys do not multiply one workspace's allowance or starve other eligible workspaces |
| Stolen customer key | Owner can revoke the exact key; customer can do so through the own-key action; invalidate cached authorization before further dispatch | Revocation races queue/dispatch correctly; it cannot silently erase in-flight usage or release money twice |
| Owner/session compromise | Revoke affected owner/device authority, stop extended capture, rotate affected credentials through the vault and audit recovery | A revoked device/session cannot continue mutations using cached capabilities or stale fresh proof |
| External integration addresses | Only owner-configured, reviewed purposes/destinations; credentials bound to the selected destination, redirects and outbound policy | A caller cannot turn a request/model/payment/login field into an arbitrary server-side fetch or forward a secret to another host |

Reuse the bounded core scheduler rather than create a second fairness algorithm in
the website. H09 qualifies starvation/queue behavior with mixed workspaces and both
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
This is planned behavior, not a replacement for Desktop local authentication.

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
The initial precision/rounding policy is below; validate every tariff before paid traffic. Corrections are
compensating entries, not edits/deletions. Keep customer charge, physical provider
expense, provider quota, and operator pricing/margin distinct.

### Initial tariff and wallet policy

The initial wallet currency is USD, represented as signed integer microUSD
(1 USD = 1,000,000 units), with checked wider intermediates for multiplication.
Payment-provider currency conversion, if needed, is quoted and accepted before
payment; never mix settlement currencies in one wallet or infer an exchange rate.

The owner sets a revisioned retail tariff per public model, displayed per million
tokens. Preserve the existing token components and evidenced context/service-tier
bands: ordinary input, cached input, output, and separately supported cache-write
or reasoning components only where they are not already included. Reuse core
usage parsing and price data structures; do not treat its API-equivalent estimate
as a posted customer debit or provider invoice. A model needs an explicit complete
retail tariff before paid enablement, even if a reference price exists.

Sum supported component costs with exact decimal/integer arithmetic and round
up once to microUSD at logical-request settlement, never per streamed chunk or
token. Reservation bounds round conservatively. Display at least six decimals for
small charges; presentation rounding does not change the journal. A confirmed
zero tariff is explicit; missing pricing or usage is not zero. Persist tariff and
rounding version with every reservation/settlement. Top-ups are prepaid; no
automatic overdraft or recurring subscription is implied.

Choose and verify a payment provider, merchant setup and launch region before
paid activation. Support one working top-up channel plus idempotent refund and
chargeback handling at launch; do not advertise payment methods not integrated.
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

### Money scenarios and worked checks

SC-03 expands the existing journal/reservation rules into customer-visible cases;
it does not select a merchant, real tariff, tax treatment or commercial permission.
All amounts below are synthetic test fixtures in USD, calculated as integer
microUSD. Each independent case starts with available `10.000000`, reserved zero;
the test hold is `2.000000`, not a required minimum balance or a model-price quote.

| Case | Ordered domain actions | Expected available / reserved / net request charge | Customer explanation |
| --- | --- | --- | --- |
| Normal completion | Reserve 2; observe supported final usage priced at 1.2; close once | `8.800000 / 0.000000 / 1.200000` | Two dollars were held; actual cost was 1.2; unused 0.8 became available |
| Proven not dispatched | Reserve 2; cancel before any dispatch; commit release | `10.000000 / 0.000000 / 0.000000` | No generation was sent; release is shown only after commit |
| Disconnected, outcome unknown | Reserve 2; disconnect after possible execution; await evidence/deadline | `8.000000 / 2.000000 / pending` | The hold is not a final charge; disconnection alone proves neither success nor free use |
| Waiver at deadline | Previous uncertain case has no substantive forwarded output and no usable usage; apply launch waiver | `10.000000 / 0.000000 / 0.000000` | Reservation released by policy; provider expense may still be unknown/nonzero |
| Bounded estimate at deadline | Previous uncertain case has substantive output, no usable final usage, and approved estimator yields 0.6; settle | `9.400000 / 0.000000 / 0.600000` | Charge is explicitly estimated under the saved policy, not labelled exact |
| Late lower usage | After the 0.6 estimate, authoritative charge under the original tariff is 0.4; credit 0.2 once | `9.600000 / 0.000000 / 0.400000` | Original charge remains visible together with a separate correction |
| Late higher usage | Alternative to the previous row: after the 0.6 estimate, later priced evidence is 0.8 | `9.400000 / 0.000000 / 0.600000` | No additional customer debit; 0.2 is operator exposure, not hidden debt |

For each closure assert `available_after = available_before + hold - charge`,
`reserved_after = reserved_before - hold`, `0 <= charge <= hold`, balanced postings
and one operation identity. For the normal case, `8 + 2 - 1.2 = 8.8`; the initial
10 equals available 8.8 plus net charge 1.2. Repeat each closure/correction and drop
its first acknowledgement: all amounts must remain unchanged on recovery.

Payment steps are similarly explicit:

1. Validate the caller, funding policy, accepted quote and unique operation. Persist
   an intent before contacting the payment provider; no credit yet.
2. Return the existing intent on repeated same-operation creation. Changed amount
   or currency conflicts; it cannot silently reuse or create another payment.
3. Verify the provider event and match merchant/reference/amount/currency to the
   original intent. Unverified redirect, message or screenshot remains no evidence.
4. Commit one balanced credit and receipt association. A synthetic 5 USD top-up
   changes 10 to 15 once; three valid deliveries of that effect still leave 15.
5. Resolve delayed/unknown payment state by querying the same operation/provider
   reference. Notification failure never reverses a confirmed credit.

For a voluntary refund, first validate the original payment's refundable remainder
and currently unreserved funds. Reserve the refund amount atomically under its own
refund operation before the external call so it cannot be spent concurrently.
Do not reuse an inference reservation that requires a request/usage tariff. The
`payment_refunds` inventory owns this separate encumbrance; HD-04 fixes its postings. On
confirmed provider success, finalize once; on proven failure, release once; an
unknown result remains a visible bounded reconciliation case, not a blind second
refund. The provider-specific escalation/deadline is HD-02, not the inference-waiver
deadline. Customer refund requests alone never reserve/debit/refund automatically.

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

Diagnostics are a first-release product capability, not a development build or a
public debug console. A user should understand what happened, whether output or
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
requests. Paired Desktop clients may read only their explicitly granted diagnostic
scope; starting extended capture requires the owner's fresh passkey verification.

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
| `summary_key`, `safe_parameters` | RU/EN explanation template and bounded allowlisted parameters; no arbitrary provider/body interpolation |
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
the UI owns translation, not a separate classification algorithm. An unfamiliar
code displays "Unclassified error" with a safe reference, not a guessed account fault.

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
and show unavailable when empty. This does not redefine the existing completion
rate or classify every failed request as an internal service defect.

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

| Observed situation | Example RU / EN summary | Required explanation/action |
| --- | --- | --- |
| Client connection closed, completion unknown | «Соединение прервано; итог запроса уточняется» / "Connection closed; request outcome is being checked" | Show uncertainty and real hold status; refresh status rather than suggest blind resend |
| Invalid upstream response shape | «Не удалось обработать ответ модели» / "Could not process the model response" | Preserve stable adapter code; owner gets safe stage/shape evidence, not a fabricated cause or raw body |
| Capacity unavailable before dispatch | «Сейчас нет свободной мощности» / "No capacity is currently available" | Retry guidance only with proven non-dispatch; show release only after committed closure |
| Upstream credential requires renewal | «Модель временно недоступна» / "The model is temporarily unavailable" | Owner receives connection-specific renewal guidance; customer sees no account identity |
| Completed output with pending settlement | «Ответ завершён; списание уточняется» / "Response completed; charge is being reconciled" | Distinguish completed execution from pending money; do not offer another generation as repair |

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
browser network error alone does not establish whether the network, proxy or server
failed. Deliberate navigation/request aborts are not service incidents.

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
notification destination before enabling each alert. An acknowledged alert remains
an acknowledgement, not proof that the underlying fault is fixed.

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

This is the recommended implementation/deployment baseline, not an inventory of
installed services. Optimize for a complete, supportable first paid product and
reuse of Relay, not maximum theoretical scale. Start in one region on one supported
Linux host with Docker Engine and Compose, persistent storage, off-host backups,
and an independent availability check. This topology has a single-host failure
domain; it does not promise high availability or zero-downtime maintenance.

| Layer | Recommended choice | Responsibility and boundary |
| --- | --- | --- |
| Shared model engine | Existing Rust `relay-core` | Reuse discovery, protocols, routing, scheduling, quotas and usage semantics; no parallel Hosted codec engine. |
| Hosted application | Rust, Axum and Tokio; one modular backend with bounded workers | Own customer identity, management, admission, accounting and jobs. Keep Hosted composition separate from Desktop; do not make Desktop a required hop. |
| Durable database | PostgreSQL | Authoritative identities, grants, tariffs, wallets, journal, reservations, payments, durable jobs and usage; never replace this authority with a cache. |
| Rust database access | SQLx with PostgreSQL, Tokio and TLS support | Explicit SQL, bounded connection pools, transactions and append-only migrations. This is a new Hosted dependency, not an existing SQLite migration. |
| Default customer website | React, TypeScript and Vite in `zenith-relay/web/` | Separately built/deployed, configurable landing page/cabinet in the same repository, using the public Hosted customer API; replaceable by an operator's own site. No Tauri or backend dependency in the browser. |
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

These are implementation/acceptance requirements, not a deployment recipe for
an already shipped Hosted backend:

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
7. Exercise sign-in, top-up, key issuance, real inference and correct settlement;
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
| 5. Money | Publish complete tariffs and configure one approved funding/refund adapter | Approved pricing/rounding/reversal policy and verified merchant binding; no default invented prices or fake payment success |
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

One installation-level `service_control` record owns intended service mode and its
revision. It is not a replacement for workspace/key/model policy or runtime health.
Every admitting path checks effective mode; hiding frontend buttons is insufficient.

| Mode | New registration / funding / inference | Existing work and customer access |
| --- | --- | --- |
| `setup` | Closed to public activity; only owner provisioning and non-paid configuration checks | Owner can correct setup; no customer money is manufactured for demos |
| `qualification` | Only explicitly allowlisted verified test identities/workspaces, with normal auth, money and route limits | Authorized integration tests and their real settlement/reconciliation; no customer/admin role shortcut |
| `live` | Open subject to all ordinary eligibility, capacity and money checks | Normal usage, history, billing, support and recovery |
| `maintenance` | No new registration, payment intents or inference dispatch | Drain/reconcile existing work; authenticated history/status and approved refund handling remain available when their stores are healthy |
| `closing` | No new sales, registration, keys or inference dispatch | Resolve current work, return eligible unused funds, provide receipts/export and controlled support |
| `closed` | Sales and inference remain disabled | Retention/reconciliation and the published support/archive access continue; this is not permission to erase records or ignore late payment events |

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
   reason rather than silently reserve a higher price. No substitution of another
   model/provider family merely because it is cheaper or available.
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
and a financial reconciliation path for missing recent events. Keep ingress closed
when the evidence is insufficient. An RPO tolerance is not permission to silently
lose customer funds, resurrect deleted data or reactivate old access.

## Implementation contract map

All Hosted paths, database objects and interfaces below are **planned**, not
implemented. This map fixes vocabulary and ownership; the detailed work packages
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

Choose `relay-hosted/` as a separate Rust crate in this repository, package/binary
`zenith-relay-hosted`. Desktop keeps its local SQLite/authentication contracts;
it does not become a mode-switch container for customer money. Hosted calls
`relay-core`, not Desktop, and does not copy the provider codec engine. This is one
modular backend, not a new service per module.

| Planned path | Responsibility |
| --- | --- |
| `relay-hosted/Cargo.toml`, `src/lib.rs`, `src/main.rs` | Package, composition and CLI; independent existing crate builds |
| `relay-hosted/src/app.rs`, `config.rs`, `http.rs`, `state.rs` | Startup/shutdown, validated config, registered routes and shared handles |
| `relay-hosted/src/db.rs`, `db/`, `migrations/` | SQLx pool, transaction/context helpers, append-only migrations |
| `relay-hosted/src/identity.rs`, `identity/` | Users/workspaces, identity linking, registration, recovery and suspension |
| `relay-hosted/src/auth.rs`, `auth/` | Sessions, login attempts, WebAuthn, fresh verification, CSRF and Desktop pairing |
| `relay-hosted/src/access.rs`, `access/` | Key verifiers, grants/limits and authenticated request context |
| `relay-hosted/src/management.rs`, `management/` | Revisioned inventory/pool changes, safe projections, transfers and audit |
| `relay-hosted/src/management/service.rs`, `management/readiness.rs` | Service-mode fencing, staged changes/closure and setup evidence; not a second admission or health authority |
| `relay-hosted/src/catalog.rs`, `catalog/` | Customer projection over the core catalog and qualified routes |
| `relay-hosted/src/accounting.rs`, `accounting/` | Exact money/tariffs, journal, wallet/hold projections and settlement |
| `relay-hosted/src/payments.rs`, `payments/` | Merchant adapter, intents, events, refunds and reconciliation |
| `relay-hosted/src/inference.rs`, `inference/` | Admission/reservation, core context, attempt observation and cancellation |
| `relay-hosted/src/continuations.rs`, `continuations/` | Scoped checkpoints and optional encrypted payload lifecycle |
| `relay-hosted/src/usage.rs`, `usage/` | Safe reports, aggregates, filters, details, diagnostic projections and bounded exports |
| `relay-hosted/src/usage/diagnostics.rs`, `management/diagnostics.rs` | Shared diagnostic mapping/read projections and owner-only capture lifecycle; reuse domain evidence, no separate billing or inference engine |
| `relay-hosted/src/notifications.rs`, `notifications/` | Domain outbox, own-user notices/preferences and optional qualified delivery; reuse durable jobs, never own money or identity verification |
| `relay-hosted/src/vault.rs`, `jobs.rs`, `jobs/` | Versioned secret references, vault integration, durable bounded work and fencing |
| `relay-hosted/openapi.yaml` | Dedicated Hosted schema |
| `relay-hosted/tests/`, `tests/support/` | Real-PostgreSQL and synthetic integration/crash fixtures |
| `relay-hosted/README.md`, `deploy/` | Future operator guide and pinned Compose/Caddy/monitoring/backup artifacts |
| `web/package.json`, `web/src/`, `web/tests/`, `web/README.md` | Independent website build, typed client, UI/tests and frontend guide |
| `web/src/api/`, `web/src/features/`, `web/src/locales/` | Transport boundary, feature UI and RU/EN copy; no database/vault access |

The customer cabinet and `/admin` may share a static artifact, never a session realm
or authorization check. Rendering a hidden admin page is not access control.

Existing reuse anchors: `crates/relay-core/src/gateway.rs`, `runtime.rs`, `protocol/`,
`scheduler/`, `accounts/`, `sources/`, `pricing/`, `usage/`; Desktop reference flows:
`src-tauri/src/local_pool/{state.rs,store/vault.rs,accounts/authority.rs,usage_writer.rs}`.
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
| Retail rate | Exact `numeric(30,12)` USD per million tokens; canonical decimal string, explicit component/band/tier |
| Revision | Nonnegative `bigint`; decimal string on wire; owner transaction increments, mutations compare expected revision |
| Status/code | Finite validated set; compatibility/migration review before introducing a value |
| Digest/verifier | Versioned binary digest and algorithm/key version; no plaintext token or unkeyed low-entropy request hash |
| Metadata | Typed columns first; JSONB only for a versioned allowlisted bounded schema, not arbitrary headers/provider payloads |

Rate precision is not an actual tariff. Reject out-of-range precision rather than
silently round it. Checked exact wider intermediates and the existing single final
rounding rule own money. Browser `Number` may produce bounded chart coordinates,
never authoritative wallet/settlement arithmetic.

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

Use ordered SQL files under `relay-hosted/migrations/`, beginning with
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
| `management_devices` | `owner_user_id`, `device_label`, `credential_verifier`, `scope`, `expires_at`, `revoked_at?` | Explicit owner-approved Desktop pairing; no permanent master-token login |
| `inference_keys` | `workspace_id`, `assigned_user_id?`, `public_prefix`, `secret_verifier`, `verifier_version`, `status`, `expires_at?`, `last_used_at?`, `authorization_epoch` | Secret shown once; does not authenticate customer/admin APIs |
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
| `connections` | `owner_scope`, `workspace_id?`, `source_kind`, `core_connection_id`, `secret_ref`, `status`, `authorization_epoch` | Inventory independent of pool membership; explicit installation/private ownership |
| `pools` | `owner_scope`, `workspace_id?`, `core_pool_id`, `status`, `policy_version`, `core_policy` | Reuse core validated pool policy; no new parallel scheduling semantics |
| `pool_members` | `pool_id`, `connection_id`, `enabled`, `drain_state`, `core_policy_version`, `core_policy` | Physical identity shared across aliases/protocols, not multiplied capacity |
| `route_qualifications` | `connection_id`, `public_model_id`, `upstream_model_id`, `operation`, `protocol`, `credential_class`, `evidence_version`, `status`, `expires_at?` | Exact-route permission/usage/bounds evidence, not trust in model names |
| `model_grants` | `workspace_id`, `public_model_id`, `route_scope`, `allowed_operations`, `status`, `authorization_epoch` | Can only narrow eligible capacity; no client-selected foreign source |
| `configuration_revisions` | `scope`, `subject_id`, `previous_revision?`, `schema_version`, `safe_configuration`, `actor_id` | Validated monotonic configuration with no secrets |
| `requests` | `workspace_id`, `key_id`, `assigned_user_id?`, `endpoint`, `public_model_id`, `execution_state`, `state_version`, `terminal_reason?`, `admitted_at?`, `dispatched_at?`, `terminal_at?`, `reconciliation_deadline?` | Logical execution/minimal financial anchor; no prompt/answer column |
| `request_details` | `workspace_id`, `request_id`, `safe_labels`, `user_agent_label?`, `timings`, `safe_error?`, `expires_at` | Short-lived detail separate from retained settlement evidence |
| `request_attempts` | `workspace_id`, `request_id`, `attempt_number`, `connection_id`, `route_revision`, `execution_certainty`, `state`, `observed_usage`, `usage_completeness`, `expense_evidence`, `started_at`, `ended_at?` | Unique request/attempt number; physical expense separate from logical charge |
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
| `tariff_versions` | `public_model_id`, `currency`, `version`, `effective_at`, `status`, `rounding_version` | Immutable published revision; price edits do not rewrite existing holds |
| `tariff_components` | `tariff_version_id`, `component`, `context_band`, `service_tier`, `usd_per_million` | Explicit non-overlapping supported components; missing rate is not zero |
| `wallets` | `workspace_id`, `currency`, `available_micro_usd`, `reserved_micro_usd`, `version`, `spending_state` | Unique workspace/currency; nonnegative journal-rebuildable projections; reserved total includes separately disclosed inference/refund holds |
| `ledger_accounts` | `workspace_id?`, `account_kind`, `currency` | Explicit customer available/reserved and installation clearing/revenue/adjustment accounts |
| `journal_transactions` | `operation_kind`, `operation_id`, `workspace_id?`, `currency`, `reason_code`, `actor_id?`, `request_id?`, `payment_id?`, `reverses_transaction_id?` | Unique operation; append-only balanced group; original corrections remain visible |
| `journal_postings` | `transaction_id`, `account_id`, `amount_micro_usd`, `sequence` | Signed exact amounts sum to zero per transaction/currency; no isolated posting commit |
| `reservations` | `workspace_id`, `wallet_id`, `request_id`, `hold_micro_usd`, `state`, `tariff_version_id`, `sizing_snapshot`, `policy_version`, `closed_at?` | One per logical request; bounded hold and atomic once-only closure |
| `settlements` | `workspace_id`, `request_id`, `reservation_id`, `charge_micro_usd`, `basis`, `usage_snapshot`, `journal_transaction_id?`, `settled_at` | Unique reservation closure; explicit zero/waiver without fictitious money postings |
| `payment_intents` | `workspace_id`, `wallet_id`, `provider_id`, `merchant_reference`, `quoted_currency`, `quoted_amount`, `credit_micro_usd`, `state`, `expires_at` | Server fixes workspace/amount; redirect alone cannot credit funds |
| `payment_events` | `provider_id`, `provider_event_id`, `merchant_reference`, `event_kind`, `verified_at`, `processing_state`, `safe_evidence`, `processed_at?` | Unique provider/event; allowlisted verification evidence, not raw webhook archive |
| `payment_refunds` | `workspace_id`, `wallet_id`, `payment_intent_id`, `operation_id`, `amount_micro_usd`, `provider_reference?`, `state`, `reservation_state`, `hold_micro_usd?`, `reserve_transaction_id?`, `close_transaction_id?`, `reason_code` | Unique operation; refund reservation is `none`, `held`, `consumed` or `released`, independent of inference; unknown provider effect cannot release spendable funds |
| `durable_jobs` | `kind`, `dedupe_key`, `safe_payload`, `state`, `not_before`, `attempt_count`, `max_attempts`, `lease_owner?`, `fencing_token`, `lease_until?`, `last_error_code?` | Bounded at-least-once work with idempotent effects, never a generic inference retry job |
| `usage_daily` | `workspace_id`, `day_utc`, `model_id`, `key_id`, `metric_version`, `coverage`, `counts`, `tokens`, `net_charge_micro_usd`, `latency_summary` | Rebuildable adjustment-aware projection; no body/User-Agent dimensions |
| `notification_outbox` | `event_key`, `event_kind`, `workspace_id?`, `subject_kind`, `subject_id`, `subject_revision`, `audience`, `template_key`, `safe_parameters`, `state`, `not_before`, `expires_at` | Unique domain effect/revision; commit with the source transition; bounded fanout, no raw message or copied financial authority |
| `notifications` | `workspace_id`, `user_id`, `event_key`, `template_key`, `safe_parameters`, `read_at?`, `superseded_at?`, `expires_at` | Unique recipient/event; current authorization rechecked on read/action; opaque event key survives outbox cleanup |
| `notification_preferences` | `workspace_id`, `user_id`, `event_preferences`, `low_balance_micro_usd?`, `balance_episode_id?`, `balance_below_threshold?`, `observed_wallet_revision?`, `external_channel_id?`, `protected_recipient_ref?`, `recipient_revision`, `recipient_verified_at?`, `consent_at?` | Unique workspace/user; durable threshold episode/rearm state; external delivery requires explicit consent and verified channel-specific binding, never just a login claim |
| `notification_deliveries` | `workspace_id`, `user_id`, `event_key`, `channel_id`, `recipient_revision`, `operation_id`, `state`, `attempt_count`, `provider_reference?`, `next_attempt_at?`, `expires_at`, `last_error_code?` | Unique recipient/event/channel; bounded queued/sent/failed/unknown/suppressed delivery, no exactly-once claim where provider cannot deduplicate |
| `service_control` | `installation_id`, `mode`, `admission_epoch`, `qualification_scope_ref?`, `operation_id`, `reason_code`, `changed_by`, `changed_at` | One row per installation; revision-CAS mode + admission fence; qualification scope is protected verified-subject/workspace policy, never a public bypass flag |
| `readiness_results` | `check_id`, `scope`, `status`, `required`, `config_revision`, `artifact_revision`, `checked_at?`, `valid_until?`, `evidence_ref?`, `reason_code?` | Safe check history; only current applicable evidence can authorize activation; invalidated/expired evidence is not green |
| `service_announcements` | `kind`, `state`, `target_kind`, `target_id?`, `target_revision?`, `affected_public_models`, `effective_at`, `ends_at?`, `template_key`, `safe_parameters`, `published_at?`, `operation_id`, `actor_id` | Draft/published/applied/cancelled/superseded change; authority remains in the referenced tariff/model/service owner, not a notice button |
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

Refund/chargeback after funds are spent needs an explicit H04/H08 policy: record
the externally imposed obligation separately, restrict new spending and reconcile;
never delete historical spend, make reserved funds silently negative or ignore the
external reversal. The receivable/posting representation and customer disclosure
are blocking finance decisions, not assumed implementation details.

### Transaction, external-effect and restart matrix

| Operation | Atomic durable boundary | Failure/duplicate rule |
| --- | --- | --- |
| Registration | Identity claim + user + workspace + empty wallet/account bindings | Duplicate callbacks resolve one identity; no transaction during external verification |
| Key issuance | Verifier/scope + audit | Lost response means revoke/reissue, never plaintext recovery from storage |
| Request reserve | Idempotency claim + guarded wallet update + postings + request/hold/price snapshot | Commit-ack loss triggers same-operation lookup, not new generation |
| Dispatch | Fenced intent/attempt record before external send | Crash around send is uncertain unless non-send is proved; expired lease cannot authorize resend |
| Terminal observation | Versioned execution/usage evidence + required continuation checkpoint | Already streamed output cannot be recalled; failed commit cannot advertise resumable success |
| Settle/release | Conditional held-state closure + settlement + balanced postings + wallet projection | Duplicate closer reads original outcome, never a second debit |
| Payment credit | Verified event claim + intent transition + journal credit + receipt reference | Duplicate/out-of-order events cannot credit twice; external reconciliation outside locks |
| Refund | Durable operation claim, external action, verified completion posting | Stable provider reference/idempotency; timeout requires reconciliation before resend |
| Secret create/rotate | Durable encrypted version, then DB reference/revision/fence | Unreferenced ciphertext reclaimable; failed commit preserves old live secret |
| Secret removal | Fence/revoke references and jobs, then eligible ciphertext cleanup | Late refresh cannot restore a source; backup expiry remains disclosed |
| Configuration edit | Expected revision + validated replacement + audit | Stale writer gets conflict; failed runtime activation stays explicit |
| Retention/export | Scoped cutoff/snapshot and idempotent progress | No deletion of unsettled dependencies or export across changed authorization |

One lock order applies to all accounting writers: unique operation claim; relevant
workspace/key policy/counter rows in stable ID order; wallets in stable ID order;
request/reservation closure rows; journal append. Omit unneeded classes, never
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
`relay-hosted/openapi.yaml`; it describes future Hosted interfaces, not Desktop routes.

The following are target route names. H02 fixes shared envelopes; each domain
package must specify every request/response field, permission, limit, pagination,
state/error and synthetic example before implementing its routes. No endpoint is
complete merely because this inventory names it. Below, paths omit the prefix.

| Method and path | Authority / behavior owner |
| --- | --- |
| `GET /public/config` | Anonymous allowlisted branding, supported API version and enabled login capabilities; no prices/secrets/private config |
| `GET /public/status` | Allowlisted public condition, observation/freshness and announcement/support link; no private capacity/customer detail and no active probe |
| `GET /public/models` | Anonymous bounded public model/tariff publication, revisions/effective time and freshness; no customer grants, upstream inventory or credential/capacity disclosure; H07 owns explicit publication and fields |
| `POST /auth/customer/{provider_id}/begin` | Bounded login/link attempt; provider registration selected from server allowlist |
| `GET /auth/customer/{provider_id}/callback` | Configured authorization-code callback, verified one-use attempt; provider-specific contract revalidated in H06 |
| `POST /auth/customer/logout` | Customer session + CSRF; server revocation, browser state cleared |
| `POST /auth/admin/sign-in/options`, `POST /auth/admin/sign-in/verify` | Bounded WebAuthn ceremony; verified owner role before admin session |
| `POST /auth/admin/logout` | Separate admin session + CSRF; no customer-session substitution |
| `GET /account` | Current user/workspace, session capabilities and account restrictions |
| `GET /account/identities`, `DELETE /account/identities/{id}` | Own verified identity links; fresh proof and remaining-method guard for removal |
| `GET /account/sessions`, `DELETE /account/sessions/{id}` | Own customer sessions/revocation; never administrator session inventory |
| `GET /account/notifications`, `POST /account/notifications/{id}/read` | Own bounded in-app notice list and idempotent read marker; CSRF on mutation, no acknowledgement of money/incident resolution |
| `GET /account/notification-preferences`, `PATCH /account/notification-preferences` | Own revision-checked event/threshold/consent policy; cannot select another user's recipient or an unqualified channel |
| `POST /account/notification-recipient/begin`, `POST /account/notification-recipient/verify` | Optional qualified channel only; fresh customer proof + CSRF, bounded single-use verification and destination controls under HD-13 |
| `GET /models` | Workspace-authorized model/pricing projection, revisions and evidence-backed availability |
| `GET /keys`, `POST /keys`, `PATCH /keys/{id}`, `DELETE /keys/{id}` | Own inference keys, narrowing policies, one-time issue and revocation |
| `GET /wallet`, `GET /wallet/entries` | Authoritative current balance and paginated posted history; holds distinguished from charges |
| `POST /payments`, `GET /payments`, `GET /payments/{id}` | Server-priced payment intent and own status/history |
| `GET /payments/{id}/receipt`, `POST /payments/{id}/refund-requests` | Own receipt or refund request; customer cannot post a refund/credit directly |
| `POST /webhooks/payments/{provider_id}` | Selected provider's signature/merchant verification; no browser session trust |
| `GET /usage/summary`, `GET /usage/timeseries`, `GET /usage/breakdown` | Authorized filters/snapshot/coverage; independent current wallet/activity |
| `GET /usage/requests`, `GET /usage/requests/{id}` | Own paginated safe detail, localized-message key, stable code, coverage, execution/accounting status and safe next action; never the owner diagnostic DTO |
| `POST /usage/exports`, `GET /usage/exports/{id}` | Bounded scoped CSV job/status/download; authorization checked again on download |
| `GET /requests/{id}/status` | Original workspace/key inference authority; safe duplicate/recovery status, no response replay |
| `/admin/*` | Owner session or explicitly scoped paired device; action matrix and fresh-proof requirements below |
| `GET /admin/usage/errors` | Authorized owner/scoped device, bounded filters/cursor and canonical logical-request counters; recovered attempts and confirmed cancellation are separate filters |
| `GET /admin/usage/requests/{id}` | Owner/scoped device request timeline, nested attempts, safe evidence and live domain-derived accounting projection; GET never probes/reexecutes |
| `GET /admin/operations/status`, `GET /admin/operations/events` | Owner/scoped device current condition/coverage and paginated retained job/component events; absence of events is not a health proof |
| `GET /admin/operations/diagnostic-sessions`, `GET /admin/operations/diagnostic-sessions/{id}` | Owner/scoped device metadata: scope, original expiry, budget consumption, state and stop reason; no raw log download |
| `POST /admin/operations/diagnostic-sessions` | Owner browser session + CSRF + fresh passkey; audited reason, finite scope/budgets and idempotent start operation; repeated operation returns the same unextended session |
| `POST /admin/operations/diagnostic-sessions/{id}/stop` | Owner browser session + CSRF; idempotent stop, no capture extension or inference/payment effect; automatic expiry/revocation also enforced server-side |
| `GET /admin/operations/readiness`, `GET /admin/operations/service` | Owner/scoped device current checklist/mode/revision and missing prerequisites; reads do not run qualification checks |
| `POST /admin/operations/readiness-checks` | Owner + fresh passkey + CSRF; named allowlisted check, scoped operation and explicit approval of any cost/external effect; no arbitrary command/URL |
| `POST /admin/operations/service/transitions` | Owner + fresh passkey + CSRF; expected mode/revision, operation ID and reason; backend enforces readiness/admission fence and closure rules |
| `GET /admin/operations/announcements`, `POST /admin/operations/announcements` | Owner list/create a bounded staged change referencing a valid domain revision; create requires CSRF, no automatic publication |
| `POST /admin/operations/announcements/{id}/publish`, `POST /admin/operations/announcements/{id}/cancel` | Owner + fresh proof + CSRF; revision/operation guards and notice-period checks, no generic rewrite of published tariffs |
| `GET /admin/settings/notifications`, `PUT /admin/settings/notifications` | Owner-only qualified channel policy, masked configuration and vault references; mutation requires fresh proof/CSRF/outbound-policy validation |

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
never lives in `web/`, images, backups of plaintext config or command-line arguments.
Rotating a secret creates a new reference/version before changing live consumers.
Provider/login/payment secrets are delivered through the audited vault boundary,
not copied into the public runtime configuration.

### Design decisions that must not be guessed

These are required decisions/evidence, not a progress log. H00 assigns an accountable
owner; the cited packages cannot implement/activate the affected contract until its
design gate is satisfied. Independent specifications can proceed meanwhile.

| Decision | Required answer/evidence | Blocks |
| --- | --- | --- |
| HD-01 | Operator-approved commercial use, region and exact credential/route classes; source references and review criteria | H07 paid qualification, H17 launch |
| HD-02 | Actual merchant/payment adapter, supported currency/quotes, verification, provider idempotency, refunds/disputes and receipt fields | H08 adapter specification, H17 launch |
| HD-03 | Maintained identity/WebAuthn libraries and revalidated Google/Telegram/OIDC flows, origin/cookie/CSRF ceremonies and supported authenticators | H05/H06 authentication implementation |
| HD-04 | Complete DDL/index/delete matrix, ledger accounts/postings, spent-funds reversal/receivable policy and migration compatibility | Each affected persistence package; H04/H08 money |
| HD-05 | Existing vault reuse/extraction boundary, versioned durability, key/digest rotation and compatible recovery manifest | H03 protected persistence, H16 restore |
| HD-06 | Input/media/tool sizing, finite route output bounds, usage overlap, estimate policy and customer disclosure | H09/H10 paid admission/settlement |
| HD-07 | Measured global/member/body/duration/job limits, report/export bounds and operating workload | H10/H12 enforcement, H16/H17 capacity claims |
| HD-08 | Full API DTOs/errors, exact route permissions, browser cookies and supported client/version matrix | Domain API packages and H13/H14 compatibility |
| HD-09 | Financial/legal retention, deletion obligations, operator identity/support/refund terms and launch jurisdiction | H12 financial cleanup design, H17 commercial launch |
| HD-10 | Pinned component/artifact versions, deployment paths, trusted edge/network policy and secret delivery | H02/H16 production packaging |
| HD-11 | Verified off-host backup/key location, recovery/revocation replay and payment reconciliation after DB rollback | H16/H17 reopening after restore |
| HD-12 | Existing local-pool parity gaps, exact supported account families and Desktop management/pairing compatibility | H01/H07/H15 promised controls |
| HD-13 | Optional external notice channel, verified-recipient ceremony, consent, safe template fields, provider delivery/idempotency contract and finite retry/retention settings | H06/H12 external notification delivery only; in-app notices and existing operator alerts do not require it |

Completeness review walks every user/owner journey across UI → API → authorization
→ domain transaction → persistence → external effect → report → recovery. Every
arrow needs an owner and a failure case. A named dependency, table or endpoint is
an index for that review, not evidence the implementation problem is solved.

## Delivery and acceptance

The order is in the [Hosted roadmap](HOSTED_RELAY_ROADMAP.md#delivery-stages). Paid launch remains
blocked until acceptance establishes:

- A new customer can sign in through each supported Google/Telegram/configured
  OIDC integration, pay, create a key, call an enabled model and inspect the correct
  charge; linking/recovery/suspension never creates another wallet or admin role.
- Operator inventory/pool controls match Desktop semantics, including parking,
  enable/disable, prices and live activity; customer screens never expose inventory.
- User-Agent labels remain untrusted and bounded; usage attribution comes from
  authenticated identities. Retention, deletion and restored revocation are tested.
- Versioned installation/upgrade/rollback, TLS/SSE/WS, alerting, backup and isolated
  restore meet the documented launch objectives under the qualified workload.
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
- The [diagnostics contract](#errors-and-production-diagnostics) is implemented in
  both owner and customer surfaces: cancel/recovered-attempt noise is distinguished
  from failed logical execution, and completed output can still have pending money.
- Unknown causes, unavailable/expired detail and dropped telemetry are explicit;
  malicious upstream errors cannot inject secrets/content into UI, logs or support
  summaries. Foreign request IDs do not disclose a customer's records.
- Error reads, alert acknowledgement and browser recovery never probe, regenerate
  or mutate money. Temporary capture enforces owner proof, finite budgets, expiry,
  revocation and restart-safe limits without exposing a public debug console.
- SC-01–SC-08 have step-level contract and acceptance evidence. A stale readiness
  check cannot open sales; maintenance/closure fence new effects while preserving
  existing settlement, refunds, history and signed callback handling.
- Synthetic money examples reconcile exactly, including refund/spend races and
  repeated acknowledgements. Notice failure/read state cannot change money or grants.
- Scheduled price/model changes preserve held tariffs and announce customer impact;
  missing activation evidence blocks affected new work rather than silently using
  an obsolete price. Service migration/closure preserves obligations and revocation.
- Abuse, key revocation, cross-workspace access, outbound destination changes and
  notification-recipient verification have explicit negative tests before exposure.
