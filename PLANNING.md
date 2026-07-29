# Zenith Relay Planning

Last reviewed: 2026-07-29.

This document describes the implementation that exists today, its boundaries,
and the design rules for compatible integrations. It is not a historical task
log; completed detail belongs in code, tests, and Git.

## Product boundary

Zenith Relay manages a user's own accounts and compatible API sources behind a
single OpenAI-compatible endpoint. It has three explicit modes:

| Mode | Runtime | Data ownership |
| --- | --- | --- |
| Choose API | A selected compatible hosted API. | The selected source and its local secret reference. |
| Computer | The desktop process and a loopback endpoint. | The user's computer and OS credential store. |
| On your server | A user-managed Relay server. | The user's server, encrypted vault, and SQLite database. |

The desktop app is not a public account marketplace, a Zenith billing backend,
or a way to move a user's accounts into Zenith inventory. Secrets remain on the
chosen device unless the user explicitly transfers them to their own connected
server through a management operation.

## Component ownership

| Component | Responsibility |
| --- | --- |
| React and Vite | Render typed snapshots, collect user intent, and localize the interface. |
| Tauri host | Native lifecycle, OS credential store, OAuth return, local endpoint, and reversible profile changes. |
| relay-core | Shared canonical account/source state, eligibility, scheduler, gateway execution, protocol translation, quota normalization, and usage math. |
| relay-server | Persistent personal runtime, encrypted vault, SQLite storage, migrations, retention, backup/restore, and management API. |

React never reads a secret or provider file directly. The desktop and server
both consume the shared runtime model rather than maintaining separate routing
rules.

## Connections, quotas, and routing

### Connections

Current account intake supports ChatGPT OAuth, an existing local profile, and
compatible imported session material. Compatible API sources are independent
records with their own address, protocol, credentials, models, priority,
recovery delay, and optional model-price overrides. A proxy is optional and
may be shared; there is no one-proxy-per-account rule.

Every candidate has a stable record, credential availability, health, model
availability, and a user-facing operational state. A missing secret, revoked
session, unavailable proxy, or reauthentication requirement is recorded as
its actual cause rather than silently treated as exhausted quota.

### Monitoring

Quota monitoring eligibility is intentionally different from routing
eligibility. Enabled local accounts can be queued for a safe quota/model
refresh even when they are not in the pool, are draining, or have an unknown
quota. Refresh results update quota, subscription, model health, error state,
and UI state together. Failures retain a safe error and timestamp, use
backoff, and do not remove the account from future checks merely because a
parser or transport attempt failed.

Quota windows are provider-reported and may have arbitrary labels and reset
times. Relay displays the reported window rather than inventing a fixed
five-hour, weekly, or subscription duration. A successful refresh can restore
health and clear a model-level transient restriction.

### Scheduler and execution

The scheduler evaluates factual conditions for the requested model:

1. the candidate is enabled and in the pool;
2. it is not draining and its credential and proxy are available;
3. its account and requested model are healthy and not cooling down;
4. the account has usable quota or the source is otherwise usable;
5. the client key and pool model rules allow the model.

It preserves response ownership affinity where a protocol response requires the
same upstream account. Prompt affinity is a preference guarded by capacity and
quota, not a way to force an unhealthy account. The user-managed server can
apply a provider/model storm breaker to prevent repeated 429 waves from
spreading across candidates.

An execution may refresh one credential and retry a compatible candidate after
an authentication failure or safe transient upstream failure. It must never
retry transparently after response bytes have been sent to the client. Stream
timings and terminal errors are recorded for diagnostics.

## Models and client visibility

The runtime model registry is built from connection capabilities and pool
model rules. A model appears through the endpoint only when at least one active
pool member can serve it and the applicable client key permits it. A model
rule can hide a model without deleting the source capability.

The current client configuration is intentionally conservative: Relay does not
ship a large hard-coded list of models into a profile. The next catalog phase
will be generated from the local or remote pool's live
<code>/v1/models</code> response:

1. **Native client models** remain in the user's existing client provider
   section.
2. **Zenith Relay pool models** are written into a separate, Relay-managed
   provider section with clear labels.
3. Only models that the selected pool and key can serve are emitted. Disabled,
   unsupported, or currently absent pool models are not advertised.
4. A pool model change regenerates only that managed section. User-owned
   providers, custom models, and unrelated profile settings stay untouched.
5. Every regeneration runs through inspect, snapshot, apply, verify, and
   restore, so the model catalog is reversible.

This keeps the model chooser understandable in Codex: a user can distinguish
native models from models offered by the local Relay pool, while Relay never
advertises a model that its endpoint would reject.

## Usage and economics

Usage records safe operational metadata: request id, selected candidate, model,
request mode, success or classified error, status, token split, time to first
output, end-to-end time, and stream speed. It does not retain prompt or
response bodies in ordinary telemetry.

API-equivalent is an informational estimate, never a routing input:

- a source-specific model price override wins;
- otherwise the verified bundled OpenAI price catalog is used;
- input, cached input, cache writes, and output retain separate price buckets;
- unknown or incomplete token splits remain explicitly unpriced;
- Fast and Standard request modes are recorded as observed service tiers, not
  multiplied by a universal hard-coded factor.

Account quota potential is learned from observed quota movement and measured
usage. Completed, uncontaminated window cycles can contribute to a
provider/plan/window benchmark; externally consumed or incomplete cycles do
not pretend to be precise. This makes estimates adapt as provider limits
change. The estimate never decides whether a request may use an account.

## Profiles and recovery

Profile operations follow one reversible transaction:

~~~text
inspect -> create or reuse snapshot -> apply managed configuration -> verify
restore -> verify restored state
~~~

Recovery lists snapshots, opens their real location when appropriate, restores
a selected snapshot, and removes only Relay-managed configuration if no
snapshot is available. It must not overwrite a user login or configuration
that changed after Relay attached the profile.

## User-managed server

The server is a personal single-deployment runtime:

- its management API uses a management token;
- clients call <code>/v1</code> with separate scoped pool keys;
- encrypted secrets live in the server vault, while operational state and
  redacted usage live in SQLite;
- migrations are append-only and protect interrupted upgrades with a
  pre-migration backup;
- backup and restore validate the database and encrypted references before
  activation;
- retention keeps operational usage bounded without discarding the information
  needed for current totals and diagnostics.

The desktop client negotiates protocol capabilities before it performs a
remote management action. It can manage accounts, sources, proxies, keys,
model/routing settings, usage, backups, and profile attachment through that
contract. The server is not yet documented as a multi-replica service:
distributed leases and cross-node prompt affinity remain future work.

## Future compatibility design

### Other subscription account systems

Relay should be able to accept other user-owned subscription systems, such as
Kiro or Antigravity, but no provider is assumed compatible just because it has
a browser login. Each proposed account connector must first prove:

1. a permitted and maintainable authentication path;
2. a way to keep credentials in the existing secret boundary;
3. normalized capabilities and model availability;
4. quota or health semantics that can be represented without false precision;
5. safe execution, refresh, revocation, deletion, and recovery behavior;
6. tests with provider-specific fixtures that contain no real secrets.

After those checks, the connector maps provider details into the existing
canonical account, capability, quota, and execution contracts. It does not add
a second scheduler or a provider-specific UI routing rule.

### Other client applications

Codex profile attachment is the current supported client integration. Future
client adapters are selected by user need and only when their configuration can
be inspected, changed reversibly, verified, and restored. An adapter owns
client-specific file discovery and managed configuration; the pool endpoint,
client keys, usage, and scheduler remain shared.

## Known limits

- Only the ChatGPT account connector is shipped today.
- The Computer mode stops with the desktop process.
- A self-hosted server requires real production acceptance with live accounts,
  proxy routing, streaming, and restart/recovery before it can be claimed as
  production-ready.
- No distributed multi-server lease system is implemented.

The ordered acceptance and future work are in [ROADMAP.md](ROADMAP.md).
