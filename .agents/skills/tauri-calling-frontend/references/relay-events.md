# Relay event lifecycle example

Use the [official Tauri v2 communication guide](https://v2.tauri.app/develop/calling-frontend/)
for event/channel APIs. Relay's current event names and lifecycle live in its
own source:

- [Typed subscriptions](../../../../src/src/features/relay/api/commands.ts):
  `onStateChanged`, `onRuntimeActivity`, and `onUsageRecorded`.
- [Runtime event hook](../../../../src/src/features/relay/state/useRuntimeEvents.ts):
  active-state guards, delayed refresh, subscription completion and cleanup.
- [Native state notifications](../../../../src-tauri/src/local_pool/state/adapters.rs)
  and [background loops](../../../../src-tauri/src/local_pool/background/loops.rs).

`zenith-state-changed` is a notification to reread state, not a replacement
snapshot. `zenith-runtime-activity` carries a typed activity payload. Preserve
that distinction and inspect the emitting owner before changing either contract.

## Cleanup when registration finishes after unmount

This minimal form illustrates the existing hook's `active`/`unlisten` pattern.
The callbacks come from the component's existing refresh and feedback owners;
do not add this as a second subscription to the same event.
The relative import below assumes a hook under the feature's `state/` directory.

```ts
import { relayCommands } from "../api/commands";

function watchStateChanges(
  refresh: () => void,
  reportError: (error: unknown) => void,
): () => void {
  let active = true;
  let stop: (() => void) | undefined;

  void relayCommands.onStateChanged(() => {
    if (active) refresh();
  }).then((unlisten) => {
    if (active) stop = unlisten;
    else unlisten();
  }).catch((error: unknown) => {
    if (active) reportError(error);
  });

  return () => {
    active = false;
    stop?.();
  };
}
```

The containing React effect returns the cleanup function. If the component
unmounts before `listen` resolves, the late `unlisten` is still called. Preserve
the actual hook's batching, revisions and generation checks as well; the small
example intentionally does not duplicate that orchestration.

Check remount, late registration, late payload, and registration failure when
those branches change. A listener test is not evidence of native event delivery
or live-provider acceptance. Ordinary subscription work does not require a
new `eval` bridge or expanded Tauri capabilities.
