# Relay command contract example

Use the [official Tauri v2 command guide](https://v2.tauri.app/develop/calling-rust/)
for framework behavior and the current source for Relay's command contract.

Version check on 2026-10-23: Relay's installed `@tauri-apps/api` and the Rust
`tauri` package in [Cargo.lock](../../../../src-tauri/Cargo.lock) both resolve
to `2.12.1`. Check the installed package and lockfile again when implementing
a change; a manifest's compatible version range is not its resolved version.

## Trace an existing read and mutation

The network settings path shows the complete ownership chain:

1. [Native command owner](../../../../src-tauri/src/local_pool/commands/gateway/network.rs):
   `get_local_network_access` and `update_local_network_access`.
2. [Desktop command registration](../../../../src-tauri/src/desktop.rs):
   `desktop_commands()` and its `tauri::generate_handler!` list.
3. [Typed frontend surface](../../../../src/src/features/relay/api/commands.ts):
   `LocalNetworkAccess`, `relayCommands.localNetworkAccess`, and
   `relayCommands.setLocalNetworkAccess`.

The existing wrapper has this shape:

```ts
localNetworkAccess: () => invoke<LocalNetworkAccess>("get_local_network_access"),
setLocalNetworkAccess: (enabled: boolean, expectedEnabled: boolean) =>
  invoke<LocalNetworkAccess>("update_local_network_access", {
    enabled,
    expectedEnabled,
  }),
```

The `expectedEnabled` key crosses IPC in camelCase; Rust owns the matching
command parameter and conflict check. Keep that expected-state argument when
changing the UI; dropping it is not a harmless wrapper simplification.

The local `invoke` wrapper also records sanitized diagnostics and rethrows the
original rejection. New Relay commands should use it rather than bypassing it
with scattered direct imports of Tauri's raw `invoke`.

## Bound the verification

For a changed command, check registration, serialized arguments/results,
Rust-side validation, and the affected error/conflict branch. Use the existing
mock or real IPC boundary appropriate to the claim. A mocked call cannot prove
native credential-store or profile behavior.

These source pointers are examples, not commands to run against the user's
settings. Do not toggle network access, restart the API, or change permissions
just to validate this documentation.
