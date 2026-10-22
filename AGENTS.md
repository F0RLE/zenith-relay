# Relay instructions

Relay is a separate local-first desktop/personal-pool product. Apply the workspace
[AGENTS.md](../AGENTS.md) when present; production Zenith contracts do not define Relay.

## Sources of truth

- Code and focused tests define behavior; plans and memory are not evidence.
- [PLANNING.md](docs/project/PLANNING.md) maps current contracts;
  [BASIS_POINTS.md](docs/project/BASIS_POINTS.md) owns BPS executor details.
- [ROADMAP.md](docs/project/ROADMAP.md) owns open Desktop work and acceptance;
  the [Hosted roadmap](docs/project/HOSTED_RELAY_ROADMAP.md) owns planned Hosted delivery.
- Localized Help owns user steps; `CONTRIBUTING.md` owns development/releases.

Resolve conflicts at the owning code and document; do not duplicate contracts.
Load guidance for the affected task only:

- API/protocol work: [official references](CONTRIBUTING.md#api-and-protocol-references)
  and [protocol skill](.agents/skills/relay-protocol-adapters/SKILL.md).
- Sign-in/import/reauthentication: [connectors](.agents/skills/relay-account-connectors/SKILL.md).
- Pool admission/renewal/quota/removal: [lifecycle](.agents/skills/relay-account-pool-lifecycle/SKILL.md).
- Frontend-to-Rust commands: [Tauri commands](.agents/skills/tauri-calling-rust/SKILL.md).
- Rust-to-frontend events/progress: [Tauri events](.agents/skills/tauri-calling-frontend/SKILL.md).
- Agent instructions or GPT-6 prompting: [agent guidance](CONTRIBUTING.md#agent-guidance).

Read only the skills relevant to the affected boundary. Reuse the workspace's
shared provider-contract, browser, build, and CLI guidance when present; do not
apply production Zenith billing, route-evidence, or operations skills to Relay.

## Ownership

| Area | Owns |
| --- | --- |
| `src/src` | React rendering, i18n, UI state, typed Tauri wrappers |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, scheduling, protocols, gateway, quota, usage |

Keep validation/side effects in Rust and Hosted/local pool boundaries distinct.
No production credentials, customer inventory, or Gateway/Control business logic
in Relay. Accounts stay local by default; secrets use the credential store or
protected local storage. Transfers need explicit confirmation;
credential exports need explicit user action. Provider credentials and pool keys
are distinct. Redact snapshots, logs, support bundles, and diagnostics.

Preserve account inventory. Model meaning comes from Relay's validated catalog,
not participant declarations. Price evidence, quota, and route eligibility are
separate; `PLANNING.md` owns their contracts. Retry only after proven
pre-execution rejection or not-sent, before client output, preserving ownership.
History does not make uncertain execution safe to repeat. Database migrations are
append-only. Profile recovery uses inspect/snapshot/attach/verify/restore and
preserves newer user logins.

Check branch/status and preserve unrelated work. Implementation requests authorize
scoped local edits and non-destructive checks; review requests authorize inspection.
Continue through the requested result and fix regressions caused by the change.
Ask only when missing information changes correctness, scope, or safety.

Use the affected [checks](CONTRIBUTING.md#verification); broaden or repeat them
only for new changes, failures, or unresolved concerns. Docs need content/link
and diff checks, not full runtime suites. Release gates remain separate. Commit,
push, PR, merge, deploy, publish, external messages, and subagents require explicit
current authorization.
