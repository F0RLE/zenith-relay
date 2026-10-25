---
name: tauri-calling-rust
description: Implement Relay Tauri frontend-to-Rust commands with typed wrappers and Rust validation. Excludes web-only UI changes and Rust-to-frontend events.
---

# Tauri frontend to Rust

Inspect the affected command registration, DTOs, and wrappers. For an unresolved
signature or API question, check the installed Tauri version and the official
[command guide](https://v2.tauri.app/develop/calling-rust/) or available Context7
documentation. Do not expand permissions to bypass a command error.

For the repository's actual registration, wrapper and argument conventions,
use the focused [Relay IPC example](references/relay-ipc.md). Reuse its owners
rather than introducing a parallel invocation or diagnostics layer.

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

Use [the affected checks](../../../CONTRIBUTING.md#verification) for changed
serialization, errors, or registration. Verify the real IPC or existing mock
boundary; internal calculations alone do not cover it. Add regression coverage
only where existing checks leave a meaningful gap.
