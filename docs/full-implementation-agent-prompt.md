# Zenith Relay Full Implementation Agent Prompt

Use this prompt in a fresh Codex task rooted at the `zenith-codex` repository.

```text
Implement Zenith Relay completely, end to end. Do not stop after analysis,
planning, scaffolding, mock UI, or one isolated phase. Continue through working
runtime code, UI integration, tests, cross-platform CI, documentation updates,
commits, and final verification.

Before changing code:

1. Read the repository `AGENTS.md` and
   `C:\Users\FORLE\.codex\skills\ponytail\SKILL.md` in full.
2. Inspect the current branch and dirty worktree. Preserve user and other-agent
   changes; never reset, checkout, or rewrite unrelated work.
3. Read in this order:
   - `docs/product-direction.md`
   - `docs/project-structure.md`
   - `docs/local-pool-final-planning.md`
   - the owning architecture/UX document for the active phase from
     `docs/README.md`
4. Run the current baseline checks before implementation.
5. Keep the public product name `Zenith Relay`. The repository/package may
   remain `zenith-codex` until a rename is separately approved.

Product objective:

- A Tauri desktop app for Windows, macOS, and Linux.
- Users can sign in to their own Codex/OpenAI OAuth accounts, add compatible API
  sources, combine them behind one local OpenAI-compatible endpoint, generate
  local client keys, and connect Codex, OpenCode, or another compatible client.
- The same personal runtime can be deployed to a user-managed server and remain
  available while the desktop is closed.
- Zenith API is only a recommended ready-API preset, not the foundation of the
  local account pool.
- Users can view account plan, health, quota windows, reset times, models,
  request usage, errors, profiles, and backups.
- Quota wake automation can select accounts/windows/models and start a new
  rolling countdown after quota fully restores, with cycle dedupe, natural-use
  suppression, minimal requests, countdown verification, and redacted history.

Hard boundaries:

- Do not copy private `zenith-account-pool` selling capacity, provider routing,
  pricing, customer debit, inventory, or admin logic into this public app.
- User-owned credentials remain on their device or selected server.
- Never store or log plaintext secrets, prompts used for wake automation, or
  generated wake responses.
- Local API keys authenticate only the local/server gateway and are never sent
  upstream.
- Keep business/runtime logic in Rust. React renders typed commands and state;
  it does not read secrets or edit client files directly.
- Follow `docs/project-structure.md` for exact paths and package names. Do not
  invent a second tree or commit empty scaffolding.
- Use append-only migrations and preserve existing profile backups.
- No Windows-only paths or behavior in shared code.

Implement the active roadmap in order and remove completed items from the
roadmap as each phase is proven:

P0. Baseline verification, module boundaries, persisted-store versioning,
    migrations, platform adapter, typed state/errors, and test harness.
P1. One compatible API source through one local `/v1` endpoint, local key auth,
    model discovery, streaming, usage capture, and client attach/restore.
P2. Unified multi-source scheduler with hard filters, priority, quota checks,
    cooldown, LRU, weight tie-break, affinity, bounded retry, and model registry.
P3. Codex/OpenAI OAuth login/import, token authority, quota/subscription refresh,
    account executor, account health, and quota wake automation.
P4. User-managed remote runtime with encrypted storage, management protocol,
    capabilities/version negotiation, local/server parity, and independent
    operation after the desktop closes.
P5. Replace mocks with the complete UI defined in `docs/app-ux-flow-spec.md`,
    including onboarding, Sources, Accounts, Automations, Pool, Endpoint, Usage,
    Profiles, Recovery, states, accessibility, and RU/EN localization.
P6. Full release verification and documentation reconciliation.

Cross-platform requirements:

- Windows x64 and ARM64.
- macOS Intel and Apple Silicon.
- Linux x64 and ARM64.
- Use one platform abstraction for config/profile paths, native secret storage,
  OAuth callback/browser launch, process detection, file locking, folder-open,
  and autostart/background capability.
- Native secret storage: Windows credential store, macOS Keychain, Linux Secret
  Service. If unavailable, require an encrypted vault or disable persistence;
  never fall back to plaintext.
- Produce EXE/Setup/MSI, DMG/app bundle, and AppImage/DEB/RPM artifacts through
  the existing GitHub Actions matrix.
- A platform may not silently omit a core feature.

Engineering rules:

- Follow existing patterns before adding abstractions or dependencies.
- Use the smallest correct implementation, but do not simplify away security,
  validation, recovery, accessibility, streaming correctness, or data safety.
- Keep scheduler, executor, quota refresh, and wake-job queues bounded.
- Use provider/account adapters for quota-window semantics; never hardcode a
  provider name as five-hour or weekly behavior.
- Natural client use after a full quota transition cancels the pending wake.
- One account/window cycle can produce at most one confirmed wake request.
- Update docs when implementation proves a contract wrong; do not leave code
  and canonical docs conflicting.

Required verification:

- Unit tests for validation, migrations, quota transitions, scheduler order,
  cooldown, affinity, token refresh locking, wake dedupe, and redaction.
- Integration tests for local key auth, `/v1/models`, non-stream responses,
  streaming boundary, fallback, usage persistence, profile backup/restore, and
  remote protocol negotiation.
- Playwright tests and screenshots at `1160x760` and `840x560` for every main
  screen in local, remote, and ready-API modes, light and dark themes.
- Verify no overlap, clipped controls, mixed-language screens, or secret leaks.
- Run `bun run check`, `bun run build`, and
  `cargo test --manifest-path ../src-tauri/Cargo.toml --locked` from `src`.
- Run the repository's complete verification command and GitHub matrix for all
  six OS/architecture targets.
- Use fixtures for normal development. Request real user credentials only when
  the real-provider end-to-end gate is reached. Never print, commit, or retain
  those credentials.
- With real credentials, verify OAuth renewal, quota parsing, primary/secondary
  windows, one actual wake cycle, client request routing, streaming, and usage.

Git workflow:

- Stay on the explicitly assigned branch unless the user approves a switch.
- Commit each completed and verified phase with focused messages.
- Do not stage unrelated dirty files. Review `git diff --cached` before every
  commit.
- Push the work branch after verified milestones.
- Open a PR into `main` only after P0-P6 and all available checks pass.
- Do not create a release tag unless explicitly requested.

Completion response must include:

- implemented phases and important architecture choices;
- exact checks run and results;
- platform/CI results;
- real-credential tests performed or the precise remaining external gate;
- commits and pushed branch/PR;
- any residual risk. Do not call the project complete while required work or
  verification remains.
```
