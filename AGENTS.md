# Relay instructions

Relay is a separate local-first desktop and personal-pool product. Apply the
workspace [AGENTS.md](../AGENTS.md) when present; production Zenith rules do not
define Relay.

## Sources of truth

- Source code and focused tests define implemented behavior.
- Use [PLANNING.md](docs/project/PLANNING.md) for the affected current contract
  and [ROADMAP.md](docs/project/ROADMAP.md) for planned work or acceptance gates.
- Localized Help owns user steps; `CONTRIBUTING.md` owns development and
  release procedures. Plans and memory do not prove implemented behavior.

When sources conflict, inspect the owning code and correct each affected
document in scope. Avoid copying architecture details into multiple files.
For API integrations and protocol adapters, follow the New API reference
workflow in [CONTRIBUTING.md](CONTRIBUTING.md#api-and-protocol-references).

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

Check branch, status, and local changes before editing. Complete the requested
local change in the owning layer and update callers when its contract changes.
Choose validation from [CONTRIBUTING.md](CONTRIBUTING.md#verification) for the
affected behavior, respecting user and harness permissions. Contributor docs
need content/link and diff checks; a text edit does not require the complete
desktop, server, and browser suites. Review and release gates still apply when
preparing those actions. Do not commit, push, create PRs, merge, deploy, or
publish without explicit current authorization.
