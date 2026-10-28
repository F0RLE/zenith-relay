# Zenith Relay documentation

Relay's current product is the local-first Desktop application. Hosted documents
describe planned work; the independent web preview demonstrates its interface.

## Find the right document

| Subject | Owner |
| --- | --- |
| Product, download, first run | [Project README](../README.md) |
| Desktop user steps and recovery | [English Help](help/en/README.md), [Russian Help](help/ru/README.md) |
| Development, checks, cleanup, releases | [Contributor guide](../CONTRIBUTING.md) |
| Private vulnerability reports | [Security reporting](../.github/SECURITY.md) |
| Repository instructions and safety boundaries | [AGENTS.md](../AGENTS.md) |
| Current architecture and implementation contracts | [Architecture](project/PLANNING.md) |
| Excel / Basis Points executor contract | [BPS](project/BASIS_POINTS.md) |
| Open Desktop work and acceptance | [Desktop roadmap](project/ROADMAP.md) |
| Future Hosted product contract | [Hosted architecture](hosted/HOSTED_RELAY_ARCHITECTURE.md) |
| Future Hosted delivery and acceptance | [Hosted roadmap](hosted/HOSTED_RELAY_ROADMAP.md) |
| Future Hosted presentation | [Hosted web design](hosted/HOSTED_RELAY_WEB_DESIGN.md) |
| Independent Hosted UI preview | [Web preview](../web/README.md) |
| Catalog policy and versioned export | [Catalog](../catalog/README.md) |
| User-visible release history | [Changelog](releases/CHANGELOG.md) |

Update the owning document for each subject. Current code and focused tests
define implemented behavior; roadmaps describe planned work and acceptance.
Legal terms remain in the [license](../LICENSE) and
[contributor agreement](../CONTRIBUTOR_LICENSE_AGREEMENT.md).

## Repository tree

Source directories:
The [architecture ownership map](project/PLANNING.md#scope-and-ownership) describes
the contracts behind these directories.

```text
zenith-relay/
|-- README.md                    Product overview and first run
|-- CONTRIBUTING.md              Development and release procedures
|-- AGENTS.md                    Repository instructions
|-- package.json                 Root setup, development, build and verification commands
|-- rust-toolchain.toml          Pinned Rust toolchain
|-- .agents/skills/              Task-specific repository guidance
|-- .codex/environments/         Explicit local development actions
|-- .github/                    CI, dependency updates, ownership and security reporting
|-- catalog/                    Model reference data and export policy
|-- crates/relay-core/           Shared Rust runtime and its tests
|-- src/                        Desktop frontend package and locked dependencies
|   |-- src/                    React application, IPC wrappers and styles
|   |-- tests/                  Unit and browser tests
|   `-- public/                 Static frontend assets
|-- src-tauri/                  Native Desktop package and locked dependencies
|   |-- src/                    Desktop I/O, credentials and process lifecycle
|   |-- examples/               Explicit development and diagnostic examples
|   |-- capabilities/           Tauri permission configuration
|   `-- icons/                  Desktop application icons
|-- web/                        Independent Hosted UI preview and tests
|-- scripts/
|   |-- build/                  Tauri and Cargo launchers
|   |-- setup/                  Explicit dependency preparation and start
|   |-- check/                  Documentation, source and policy checks
|   |-- maintenance/            Scoped build-artifact cleanup
|   |-- release/                Packaging and release helpers
|   `-- lib/                    Helpers shared between command groups
`-- docs/
    |-- README.md               Documentation navigation and repository tree
    |-- help/                   Localized, bundled Desktop user guides
    |-- project/                Current Desktop architecture, BPS and roadmap
    |-- hosted/                 Future Hosted architecture, roadmap and web design
    |-- releases/               User-visible changelog
    `-- screenshots/            Committed product screenshots
```

`src/src/` is intentional: `src/` is the frontend package, while its nested
`src/` contains application code. Moving either affects build, test, and Tauri
paths; do not rename them just to make a local file listing shorter.

## Local artifacts and private data

| Local paths | Treatment |
| --- | --- |
| `target/`, `src-tauri/target/`, `crates/relay-core/target/` | Rust build outputs, not source. Cleanup also removes a locally built executable. |
| `src/node_modules/`, `web/node_modules/` | Installed frontend dependencies, not source. |
| `src/.build/`, `src/dist/`, `web/dist/` | Frontend build outputs. |
| Test reports, browser captures, `output/` | Generated verification artifacts, not product documentation. |
| Root `.build/` | May contain local build worktrees; the cleanup command deliberately leaves it alone. |
| `.dev-data/` | Persistent private data if explicitly created; not a build cache and not used automatically by Dev. |

Use the [documented cleanup command](../CONTRIBUTING.md#build-artifacts), not a
recursive removal of every ignored directory. Working profiles, credential data,
installed applications, and committed screenshots are not build artifacts.

## Check documentation

From the repository root:

```powershell
bun run --cwd src check:docs
```

The check covers owned Markdown documents, required files, local links, heading
anchors, and filename case. It does not fetch external URLs or establish live
provider, installed-application, or Hosted acceptance.
