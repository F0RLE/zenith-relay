## Target Branch

- [ ] This PR targets `nightly`, or
- [ ] This is a release promotion PR from `nightly` to `main`.

## Checks

- [ ] `cd src && bun run check`
- [ ] `cd src && bun run build`
- [ ] `cd src && cargo test --manifest-path ../src-tauri/Cargo.toml --locked`

## Notes

Describe what changed and whether this affects packaging, updater behavior, or Codex config writes.
