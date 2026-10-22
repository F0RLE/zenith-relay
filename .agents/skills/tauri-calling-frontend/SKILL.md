---
name: tauri-calling-frontend
description: Implement Relay Tauri Rust-to-frontend events or progress with typed payloads and listener cleanup. Excludes web-only UI changes and frontend-to-Rust commands.
---

# Rust to Tauri frontend

Read the installed Tauri version, existing event names/DTOs, subscriptions, and
the receiving component before editing. Use the version-matching official
[communication guide](https://v2.tauri.app/develop/calling-frontend/) or available
Context7 for API details; do not copy an unrelated tutorial implementation.

- Use existing events for notifications or channels for operation-specific
  streaming. Prefer typed payloads over interpolating data into JavaScript.
- Keep side effects, secret access, and validation in Rust. Emit only the
  redacted state required by the intended frontend/window.
- Propagate or classify delivery errors through the existing error contract;
  production event delivery must not add `unwrap`/panic paths.
- Clean up subscriptions when components unmount, including when async listener
  registration completes after unmount. Avoid duplicate handlers on remount.
- Preserve operation IDs/revisions where concurrent updates need ordering.
  Events are not durable persistence or authorization evidence.
- Do not interpolate untrusted text into `eval`. If evaluation is genuinely
  necessary, use a fixed script and safe serialization, not string concatenation.

Select checks from [CONTRIBUTING.md](../../../CONTRIBUTING.md#verification)
for the changed serialization or listener lifecycle, within task and harness
permissions. Reuse existing coverage;
delivery, remount, or cancellation changes need a regression case only when
existing coverage leaves a meaningful gap.
