# P0 Baseline

Verified on 2026-07-10 from the work branch later renamed to
`relay/local-pool-product-modes`, before local-pool implementation.

## Checks

- `cd src && bun run verify`: passed.
- TypeScript check and Vite production build: passed.
- Rust suite: 24 passed, 0 failed.

## Existing Zenith API Mode

The current Rust commands still own API-key validation/storage, stats, usage
history/version, top-up intent creation, Codex config attach, backup, restore,
launch, and redaction. Their existing tests passed unchanged.

## Reproducible Blockers

None in the shipped Zenith API mode. Before P0 there was no local-pool module,
versioned store, migration harness, typed local-pool state/error contract, or
generic secret namespace.
