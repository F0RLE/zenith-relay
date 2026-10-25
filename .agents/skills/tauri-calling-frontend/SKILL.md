---
name: tauri-calling-frontend
description: Implement Relay Tauri Rust-to-frontend events or progress with typed payloads and listener cleanup. Excludes web-only UI changes and frontend-to-Rust commands.
---

# Rust to Tauri frontend

Inspect the affected event/DTO, subscription, and receiving component. For an
unresolved Tauri API question, check the installed version and the official
[communication guide](https://v2.tauri.app/develop/calling-frontend/) or available
Context7; do not replace established local behavior with an unrelated tutorial.

For the current subscriptions and the late-registration cleanup pattern,
use the focused [Relay event example](references/relay-events.md). Reuse the
existing hook and event owners instead of adding another subscription manager.

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

Use [the affected checks](../../../CONTRIBUTING.md#verification) for changed
serialization, delivery, remount, or cancellation behavior. Add regression
coverage only where existing checks leave a meaningful gap.
