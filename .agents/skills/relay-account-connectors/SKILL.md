---
name: relay-account-connectors
description: Add or repair Relay account sign-in, including OAuth, device authorization, reauthentication, and credential imports. Excludes generic API-key sources, UI-only changes, and production Zenith user authentication.
---

# Relay account connectors

Implement a supported provider account connection from authorization through
durable credential storage. Successful sign-in and pool admission are separate
operations.

Use [PLANNING.md](../../../docs/project/PLANNING.md#connections-discovery-and-metadata)
for the current contract and
[CONTRIBUTING.md](../../../CONTRIBUTING.md#account-connector-references) for
development guidance. For provider API work, also follow its official contract
workflow.

## Establish the provider contract

- Identify the provider, issuing OAuth client, principal/workspace identity,
  supported grant or credential type, scopes, endpoints, redirect rules, expiry,
  and renewal method. Distinguish documented support from observed private
  behavior; a client ID found in another app does not establish permission.
- Read the matching provider implementation in
  [implementations.md](references/implementations.md#login-and-import).
  Compare it with the provider's official contract and record the inspected
  commit and paths. New API's product-user OAuth is unrelated to provider
  account credentials.
- Trace `src-tauri/src/local_pool/accounts/oauth/`, `oauth_flow/`, and
  `commands/oauth/{flow,checkpoint,completion,account}.rs`. Imports belong to
  `accounts/import_orchestrator/` and the shared `crates/relay-core/src/accounts/import/`.
  These paths are currently ChatGPT-specific: reuse lifecycle primitives, but
  introduce a typed provider credential when needed instead of pretending a
  new provider is a Codex or Excel client.

## Extend the existing lifecycle

- Bind each attempt to its provider/client, random state, PKCE S256 verifier,
  redirect, expiry, and optional target account. New native authorization-code
  flows follow RFC 8252's external-user-agent contract. Treat an existing
  provider-specific flow separately; do not broaden its behavior implicitly.
- Validate completion in Rust. Loopback listeners bind only loopback; a random
  port fallback is valid only when the provider permits it. Manual callbacks
  pass the same validation as browser callbacks, including duplicate parameters,
  exact redirect and state. Cancellation, expiry, and an already consumed
  attempt fence later callbacks and token replies.
- Device authorization has its own state machine. Respect the provider interval
  and lifetime, distinguish pending from success, and increase the interval on
  `slow_down`. Stop on denial, expiry, or cancellation. Do not apply one
  provider's HTTP-status polling convention to another.
- Validate the token response before replacing credentials. Retain the issuing
  client for refresh, import, and export. Decoded JWT claims
  are unverified hints unless validated under the provider's identity contract.
  Missing expiry or refresh support stays explicit.
- Reauthenticate only the intended principal and client. A stable provider user
  ID takes precedence over a changed email. Preserve notes, account ID, proxy,
  model rules, operator settings, and membership; apply credentials through
  the existing lock, checkpoint, revision, and conditional rollback owners.
- Store secrets through the Desktop credential store and protected vault.
  Renderer state and diagnostics receive redacted status. An import consumes
  an explicitly selected document; it does not scan browser profiles or harvest
  cookies. Keep profile configuration and backups with the existing profile
  integration rather than inventing another store in the client's directory.

## Establish acceptance

Use [acceptance.md](references/acceptance.md) for the affected login or import
path. Inspect existing coverage; run or extend checks only within task and
harness permissions. Local checks do not prove live-provider acceptance.

When pool execution or renewal changes, use the
[pool lifecycle skill](../relay-account-pool-lifecycle/SKILL.md).
Update the existing contract and localized Help only for implemented behavior.
Report adopted behavior, intentional differences, source revisions, and actual
checks; preserve source attribution and license terms for reused code.
