# Zenith Relay Documentation

Read documents by ownership instead of reading every file as one specification.

| Document | Owns |
| --- | --- |
| [product-direction.md](product-direction.md) | Product modes, public/private boundary, and user-facing scope |
| [project-structure.md](project-structure.md) | Canonical repository tree, package names, module ownership, tests, and runtime data paths |
| [local-pool-final-planning.md](local-pool-final-planning.md) | Unfinished implementation order and release gates |
| [local-pool-runtime-contract.md](local-pool-runtime-contract.md) | Local/server runtime targets, public self-host protocol, gateway and failure contracts |
| [local-account-auth-architecture.md](local-account-auth-architecture.md) | Account/source import, OAuth, tokens, quota, profiles, backups, and repair |
| [local-gateway-architecture.md](local-gateway-architecture.md) | Rust/Tauri modules, storage, scheduler, execution, telemetry, and local HTTP runtime |
| [app-ux-flow-spec.md](app-ux-flow-spec.md) | Screens, navigation, controls, states, design, accessibility, and button behavior |
| [local-pool-ui-notes.md](local-pool-ui-notes.md) | Reference UI/backend observations; not a product contract |
| [full-implementation-agent-prompt.md](full-implementation-agent-prompt.md) | Ready-to-use handoff prompt for implementing and verifying P0-P6 |

## Reading Order

For implementation:

1. `product-direction.md`;
2. `project-structure.md`;
3. `local-pool-final-planning.md`;
4. only the owning architecture/UX document for the current phase.

For review:

1. verify public/private boundaries in `product-direction.md`;
2. verify the task exists in the implementation roadmap;
3. verify exact behavior in one owning specification;
4. reject duplicate or conflicting definitions in other files.

## Rules

- Future planning is not shipped behavior.
- `zenith-account-pool` private server logic must not enter this public app.
- Zenith backend pricing, balances, provider routing, and customer debit remain
  backend-owned.
- User-owned local accounts stay on the device by default.
- User-owned server accounts stay on the server selected by that user.
- Exact DTOs and commands belong to architecture/runtime specs, not every
  product document.
- Exact repository paths and package names belong only to
  `project-structure.md`.
- Exact controls and visual behavior belong to `app-ux-flow-spec.md`.
- Completed roadmap tasks are removed; Git and tests keep history.
