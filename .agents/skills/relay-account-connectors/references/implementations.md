# Account implementation references

Use the relevant files at these inspected revisions, then verify the current
provider contract. Source examples establish implementation ideas, not provider
permission, account access, or compatibility. Read callers when a helper alone
does not establish its lifecycle.

| Repository | Inspected commit | License |
| --- | --- | --- |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) | `091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` | MIT |
| [OpenCodex](https://github.com/lidge-jun/opencodex) | `33b321853aff76045617824793cc7cbc1d4084e7` | MIT |
| [New API](https://github.com/QuantumNous/new-api) | `ba0e478e061ee0c824922d89c05620de878103cf` | AGPL-3.0 |
| [EasyCLIProxyAPI](https://github.com/router-for-me/EasyCLIProxyAPI) | `74ef3e16ab347161c7dbb7bddb531b0a3b950770` | MIT |
| [CLIProxyAPI-Rust](https://github.com/IuCC123/CLIProxyAPI-Rust) | `7a3c8c2e017fafe9a3dc9a72dba5697feffcb992` | Unlicense |

## Login and import

**CLIProxyAPI** separates provider authenticators from login persistence in
[`sdk/auth/interfaces.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/sdk/auth/interfaces.go)
and [`manager.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/sdk/auth/manager.go).
Use that ownership split without importing its file-based auth store.

- [`codex_device.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/sdk/auth/codex_device.go)
  separates device approval from the final authorization-code exchange. Its
  403/404 polling convention is specific to that endpoint, not RFC 8628.
- [`internal/auth/kimi/kimi.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/auth/kimi/kimi.go)
  shows device-code fields, cancellation and expiry, and parsing pending errors
  from HTTP 200. At this revision, `slow_down` returns "continue" while the caller
  keeps a fixed ticker; implement the interval increase rather than copying it.
- [`internal/auth/claude/anthropic_auth.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/auth/claude/anthropic_auth.go)
  shows PKCE exchange, account-profile reads, and refresh coalescing. Its
  `code#state` parser can replace the supplied state during exchange; a Relay
  callback must match the active attempt before exchange. Client impersonation,
  header mimicry, and serialized-byte mimicry are not reusable login policy.

**OpenCodex** has useful browser and manual-completion coordination in
[`callback-server.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/callback-server.ts),
[`login-flow-state.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/login-flow-state.ts),
and [`pkce.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/pkce.ts).
It distinguishes fixed redirects from random-port fallback and validates
state-bearing manual input. Relay currently requires a complete callback URL;
do not silently add the reference's bare-code exception.

For imports, inspect
[`account-import/service.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/account-import/service.ts)
and [`registry.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/account-import/registry.ts).
Provider/format admission precedes credential traversal; cancellation records
already committed outcomes. Adapt those distinctions to Relay's existing
preview, confirmation, reconciliation, and rollback flow.

## Refresh and pool ownership

**New API's provider credentials** are in
[`service/codex_oauth.go`](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/service/codex_oauth.go),
[`codex_credential_refresh.go`](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/service/codex_credential_refresh.go),
and [`codex_credential_refresh_task.go`](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/service/codex_credential_refresh_task.go).
Use them for request fields, response validation, proxy handling, and renewal
scheduling. Its `oauth/` and user-binding controllers authenticate New API
users. They are not account-pool connectors.

The inspected channel refresh writes by channel ID without a credential revision
predicate. The background task's running flag does not coordinate every manual
refresh. Keep Relay's shared authority, post-response reread, and revision fences.
Do not import channel billing, admin state, or plaintext key storage.

**CLIProxyAPI's**
[`auto_refresh_loop.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/sdk/cliproxy/auth/auto_refresh_loop.go)
uses a bounded worker queue and registration epochs.
[`conductor_persistence.go`](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/sdk/cliproxy/auth/conductor_persistence.go)
separates metadata saves from credential mutations and checks registration
ownership. Adapt the relevant concurrency boundary, not the whole conductor.

**OpenCodex's**
[`token-guardian.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/token-guardian.ts)
calls the existing renewal machinery, bounds concurrency, and respects paused
accounts and refresh policy. Its extra warmup traffic is not a Relay default.
[`anthropic-send-ownership.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/anthropic-send-ownership.ts)
captures account/login/credential ownership before sending and checks it without
adopting a replacement login. This is distinct from selecting a new pool member.
[`pool-kernel.ts`](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/oauth/pool-kernel.ts)
illustrates shared selection primitives; its rotation strategies do not replace
Relay's admission and execution-ownership contracts.

## Additional references

- **EasyCLIProxyAPI:**
  [`native_oauth.rs`](https://github.com/router-for-me/EasyCLIProxyAPI/blob/74ef3e16ab347161c7dbb7bddb531b0a3b950770/src-tauri/src/agents/native_oauth.rs)
  is relevant to restoring native application auth and ordering profile writes.
  Use it only when profile integration is affected; keep Relay recovery and
  backups in their existing owner.
- **CLIProxyAPI-Rust:**
  [`src/oauth.rs`](https://github.com/IuCC123/CLIProxyAPI-Rust/blob/7a3c8c2e017fafe9a3dc9a72dba5697feffcb992/src/oauth.rs)
  shows per-account locking and rechecking after acquisition. Token-text equality
  alone does not fence a newer login with identical token text. Its OAuth files
  are not a replacement for Relay's credential store or protected local vault.

## Protocol standards

- [RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html): PKCE verifier and S256
  challenge.
- [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html): native-app external
  user agents and registered redirect handling.
- [RFC 8628, section 3.5](https://www.rfc-editor.org/rfc/rfc8628.html#section-3.5):
  pending polling, persistent five-second interval increases on `slow_down`,
  timeout backoff, and terminal device-flow errors.
- [RFC 9700, section 4.14](https://www.rfc-editor.org/rfc/rfc9700.html#section-4.14):
  client/scope binding, refresh rotation, and protection of refresh credentials.

These sources do not define a provider's available scopes, client registration,
renewal cadence, or permission to reuse a subscription endpoint. Establish those
from its current official contract and permitted acceptance.
