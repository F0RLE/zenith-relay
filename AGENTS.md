# Relay instructions

Relay is a separate local-first desktop and personal-pool product. Read the
workspace [AGENTS.md](../AGENTS.md); production Zenith rules do not define Relay.

## Sources of truth

- Source code and focused tests define implemented behavior.
- [PLANNING.md](docs/project/PLANNING.md) describes current contracts;
  [ROADMAP.md](docs/project/ROADMAP.md) describes open work and acceptance.
- Localized Help describes user steps; `CONTRIBUTING.md` owns development and
  release procedures. The pool rotation core is connected; its design includes
  unfinished host/acceptance gates tracked in ROADMAP, not a completed contract.
- Memory is recall context, never authority. Explicit user requests take
  precedence over skill recommendations.

When sources conflict, inspect the owning code and correct each affected
document in scope. Avoid copying architecture details into multiple files.

## Ownership

| Area | Owns |
| --- | --- |
| `src/src` | React rendering, i18n, UI state, typed Tauri wrappers |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, scheduling, protocols, gateway, quota, usage |
| `relay-server` | User-managed runtime, encrypted vault, persistence, management API |

Keep validation and side effects in Rust. Keep hosted API, local pool, and
user-managed server distinct. Never import production credentials, customer
inventory, or internal Gateway/Control logic. Accounts stay on the user's device
by default. Store secrets only in the credential store or that user's encrypted
server vault; transfer requires explicit confirmation. Management tokens and
pool request keys are distinct. Redact snapshots, logs, exports, and diagnostics.

Preserve account inventory; resolve model meaning from Relay's validated
reference catalog, not participant capability declarations. Price evidence,
quota monitoring, and route eligibility are separate concerns; detailed rules
live in `PLANNING.md`. Retry only after a proven pre-execution rejection or
not-sent outcome, before response bytes reach the client, and with preserved
ownership. Complete history does not make unknown execution safe to repeat.
Server migrations are append-only. Profile
recovery follows inspect, snapshot, attach, verify, and restore, preserving
newer user logins.

Check branch, status, and local changes before editing. Change the owning layer,
update callers when its contract changes, and run the relevant `CONTRIBUTING.md`
checks. Do not commit, push, create PRs, merge, deploy, or publish without
explicit current authorization.

Develop on `release/1.1.3` by default. Leave `main` untouched unless the user
explicitly directs otherwise.
