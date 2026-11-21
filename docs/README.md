# Zenith Relay documentation

Relay ships a local-first Desktop application. The independent Hosted server
currently supports a closed installation, not public paid service.

## Find the right document

| Subject | Owner |
| --- | --- |
| Product, download, first run | [Project README](../README.md) |
| Desktop user steps and recovery | [English Help](help/en/README.md), [Russian Help](help/ru/README.md) |
| Development, checks, cleanup, releases | [Contributor guide](CONTRIBUTING.md) |
| Provider API references and OpenAI migration requirements | [Protocol contracts](../.agents/skills/relay-protocol-adapters/references/contracts.md), [OpenAI migrations](../.agents/skills/relay-protocol-adapters/references/contracts.md#openai-model-migrations) |
| Private vulnerability reports | [Security reporting](../.github/SECURITY.md) |
| Repository instructions and safety boundaries | [AGENTS.md](../AGENTS.md) |
| Current architecture and implementation contracts | [Architecture](project/PLANNING.md) |
| Excel / Basis Points executor contract | [BPS](project/BASIS_POINTS.md) |
| Open Desktop work and acceptance | [Desktop roadmap](project/ROADMAP.md) |
| Catalog policy and versioned export | [Catalog](../crates/relay-core/catalog/README.md) |
| User-visible release history | [English changelog](releases/CHANGELOG.md), [Russian changelog](releases/CHANGELOG.ru.md) |

`PLANNING.md` is the current architecture reference despite its historical
filename. Start with its [contract map](project/PLANNING.md#contract-map).
Help contains task instructions followed by an error reference; the application
renders the reference separately. Both are maintained in each localized guide.

### Hosted: implementation and target service

| Subject | Owner |
| --- | --- |
| How the service works, from setup to closure | [Hosted architecture](hosted/HOSTED_RELAY_ARCHITECTURE.md#step-by-step-scenario-map) |
| Customer and owner screens, actions and states | [Hosted web design](hosted/HOSTED_RELAY_WEB_DESIGN.md#screen-inventory) |
| Implementation order, tests and release criteria | [Hosted roadmap](hosted/HOSTED_RELAY_ROADMAP.md#delivery-stages) |
| Existing web preview, launch commands and integration limits | [Web preview](../hosted/web/README.md) |
| Install, migrate, provision the first owner, run and test the closed service | [Server guide](../hosted/server/README.md) |

Update the owning document for each subject. Current code and focused tests
define implemented behavior; roadmaps describe planned work and acceptance.
Legal terms remain in the [license](../LICENSE) and
[contributor agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

## Repository tree

The [architecture ownership map](project/PLANNING.md#scope-and-ownership) describes
each source directory's contracts.

```text
zenith-relay/
|-- README.md                    Product overview and first run
|-- AGENTS.md                    Repository instructions
|-- package.json                 Root setup, development, build and verification commands
|-- rust-toolchain.toml          Pinned Rust toolchain
|-- .agents/skills/              Task-specific repository guidance
|-- .codex/environments/         Explicit local development actions
|-- .github/                    CI, dependency updates, ownership and security reporting
|-- crates/relay-core/           Shared Rust runtime and its tests
|   `-- catalog/                Bundled model policies and export schemas
|-- desktop/                    Downloadable, independently built local application
|   |-- frontend/               React UI, tests, assets and locked dependencies
|   `-- src-tauri/              Native Rust package, permissions, icons and installers
|-- hosted/                     Independent Hosted product
|   |-- server/                 Rust server, PostgreSQL migrations and tests
|   `-- web/                    Production entry and development dashboard preview
|-- scripts/
|   |-- build/                  Tauri and Cargo launchers
|   |-- setup/                  Explicit dependency preparation and start
|   |-- check/                  Documentation, source and policy checks
|   |-- maintenance/            Scoped build-artifact cleanup
|   |-- release/                Packaging and release helpers
|   `-- lib/                    Helpers shared between command groups
`-- docs/
    |-- README.md               Documentation navigation and repository tree
    |-- CONTRIBUTING.md          Development and release procedures
    |-- CONTRIBUTOR_LICENSE_AGREEMENT.md  Contributor terms
    |-- help/                   Localized, bundled Desktop user guides
    |-- project/                Current Desktop architecture, BPS and roadmap
    |-- hosted/                 Future Hosted architecture, roadmap and web design
    |-- releases/               User-visible changelog
    `-- screenshots/            Committed product screenshots
```

Desktop is distributed as release binaries/installers or built from source. Its
build does not require Hosted. The web preview builds without Tauri; it reuses
pure presentation helpers and styles, not Desktop IPC or credentials.
The existing server crate is under `hosted/server/`; deployment artifacts are planned.
Shared runtime code stays in `crates/relay-core/`. See the
[Hosted roadmap](hosted/HOSTED_RELAY_ROADMAP.md) for service delivery.

## Local artifacts and private data

| Local paths | Treatment |
| --- | --- |
| `desktop/src-tauri/target/`, `crates/relay-core/target/` | Rust build outputs. Cleanup also removes the locally built executable. |
| `desktop/frontend/node_modules/`, `hosted/web/node_modules/` | Installed frontend dependencies. |
| `desktop/frontend/.build/`, `desktop/frontend/dist/`, `hosted/web/dist/` | Frontend build outputs. |
| Test reports and browser captures | Generated verification artifacts. |
| `.build/desktop/` | Desktop build log and MSBuild temporary files; included in full cleanup. |
| `.build/output/` | Local exports and saved outputs; preserved by cleanup. |
| Other root `.build/` contents | Local build worktrees and tool caches; preserved by cleanup. |
| `.dev-data/` | Private profile data, used only when explicitly configured. |

Use the [documented cleanup command](CONTRIBUTING.md#build-artifacts) to remove
build outputs while preserving profiles, credentials, installed applications, and
committed screenshots.

## Check documentation

From the repository root:

```powershell
bun run --cwd desktop/frontend check:docs
```

The offline check validates owned Markdown documents, required files, local links,
heading anchors, and filename case. Validate external URLs, providers, installed
applications, and Hosted separately.
