---
name: tauri-calling-rust
description: Implement Relay Tauri frontend-to-Rust commands with typed wrappers and Rust validation. Excludes web-only UI changes and Rust-to-frontend events.
---

# Tauri frontend to Rust

Inspect the existing command registration, DTOs, wrappers, and installed Tauri
version. Use the version-matching official
[command guide](https://v2.tauri.app/develop/calling-rust/) or available Context7
for exact signatures; do not expand permissions to bypass a command error.

- Add behavior to its Rust owner and wire it through the existing registration
  path. Keep the TypeScript wrapper and serialized argument/result shape aligned.
- Validate runtime input in Rust; a TypeScript type assertion is not validation
  or permission. Reuse safe structured errors rather than exposing raw I/O,
  provider responses, or secrets.
- Keep credentials, files, network calls, process control, and profile writes
  out of React. Return redacted DTOs needed by the UI.
- Match argument names to the command's serialization convention; use owned
  data for spawned async work and release state locks before network awaits.
- Preserve cancellation, duplicate-submit, and expected-revision semantics for
  mutations. Route destructive actions through their existing confirmation
  boundary, not a newly unrestricted generic command.

Select validation from [CONTRIBUTING.md](../../../CONTRIBUTING.md#verification)
for the changed IPC contract, within task and harness permissions. When
serialization, errors, or registration
change, verify the real IPC or existing mock boundary; internal calculations
alone do not cover it. Reuse existing coverage unless a meaningful gap needs
a regression case.
