---
name: relay-siwc-development
description: Research, implement, or diagnose Relay's Sign in with ChatGPT (SIWC) dynamic-client integration, including authentication, public Responses transport, and usage evidence. Excludes native Codex OAuth, Excel/BPS, production Zenith, and cosmetic UI-only changes.
---

# Relay SIWC development

Use this workflow for SIWC-specific research and authorized changes in Relay.
Identify the issuing OAuth client and upstream route first: a reference to
"Sign in with ChatGPT" or "SIWC" in another project does not prove it uses the
dynamic-client integration rather than native Codex authentication.

## Choose the affected boundary

[PLANNING.md](../../../docs/project/PLANNING.md#connections-discovery-and-metadata)
owns Relay's maintained integration contract; [ROADMAP.md](../../../docs/project/ROADMAP.md#future-connectors)
separates unfinished acceptance from implemented behavior.

- For registration, callback, import, or reauthentication, combine this skill
  with [account connectors](../relay-account-connectors/SKILL.md). Start under
  `src-tauri/src/local_pool/accounts/oauth/` and `commands/oauth/`.
- For renewal, admission, quota, or removal, use
  [pool lifecycle](../relay-account-pool-lifecycle/SKILL.md) and the existing
  credential authority, rotation checkpoints, and account fences.
- For request conversion, streaming, tools, or continuation, use
  [protocol adapters](../relay-protocol-adapters/SKILL.md). The SIWC request
  policy starts at `crates/relay-core/src/providers/chatgpt/siwc.rs`.
- For source research alone, use the relevant entries in
  [sources.md](references/sources.md); do not load every lifecycle or protocol
  reference, change code, or probe stored accounts merely to compare projects.

## Establish the exact contract

Fetch the official SIWC page for the affected operation before adopting another
proxy's behavior. Distinguish documented support, an inspected implementation,
and observed live behavior. Reuse current evidence from the task; refresh it
when a claim depends on changed provider behavior. Record the inspected revision
and relevant symbol when comparing repositories, not just a search snippet.

SIWC plan usage uses its own grant and the public `/v1` API. Native Codex,
ChatGPT website, and BPS endpoints do not become supported by reusing their
headers or because another repository calls them SIWC. Verify endpoint,
operation, account/workspace permission, transport, and feature independently.
Inspect current licensing before copying code; source availability is not a
permission grant. Keep findings in chat unless an existing contract needs updating.

## Preserve identity and renewable sessions

- Keep the stable host identifier separate from the issued client registration
  and verified identity. Reuse the selected registration on reauthorization;
  email equality does not merge registrations or establish workspace identity.
  Use the bootstrap client only for initial registration, never token exchange.
- Preserve an issued registration across a failed code exchange without making
  an unverified account active. Bind callbacks and any login hints to the selected
  attempt; retain the exact loopback redirect and validate state, PKCE, and nonce.
  System-browser sign-in is the documented path. Qualify embedded WebView
  compatibility separately rather than treating it as guaranteed.
- Distinguish verified sign-in from permission to use the plan. Inspect granted
  scopes and identity-only responses; do not invent credentials or enable inference
  to make a declined permission look successful. Request renewed consent only
  through the user's explicit action and the currently supported parameters.
- Validate token-response shapes and time units against current documentation
  and reference code. Preserve omitted-on-refresh fields according to OAuth
  semantics; do not infer grant permissions or quota from opaque access-token data.
- Serialize rotating refreshes through the existing authority. Retain a received
  replacement in a protected recovery checkpoint before further fallible identity
  verification, without authorizing it before verification. A cancellation,
  restart, or unavailable key service must not cause reuse of a consumed grant.
  Bind recovery to the still-current registration and credential generation.
- Cache verified JWKS/discovery metadata with bounded refresh and key-rotation
  handling when useful. A retrieval outage differs from an invalid signature.
  Keep signature, issuer, audience, expiry, and identity checks intact. Authorization
  URLs containing identity hints need redaction, not routine diagnostic logging.

## Preserve inference and usage meaning

- Discover models with the selected SIWC grant; model names and public inventory
  do not establish that account's entitlement. Refresh account-specific choices
  when their authorization changes.
- Check the current SIWC limitations for tools, input items, and request options.
  Preserve supported semantics or return a precise incompatibility; do not silently
  remove reasoning, history, tool restrictions, or client cache policy.
- Keep stateless HTTP history distinct from same-authenticated-connection
  WebSocket continuation. Preserve the originating account and validated replay
  ownership. Treat completed, incomplete, failed, and interrupted streams separately;
  a stream opening or a text delta is not successful completion or safe retry evidence.
- Keep app limits, shared subscription limits, credits, local usage, cost estimates,
  and remaining quota distinct. Qualify a telemetry endpoint for this exact grant
  before polling it; a missing observation stays unknown. Classify structured
  provider errors separately from HTML challenges and unstructured admission failures.
  Neither a 403 nor temporary infrastructure failure alone proves token revocation.

## Establish the result

Use the affected owner's synthetic acceptance cases and
[repository checks](../../../CONTRIBUTING.md#verification). Select cases for the
changed boundary: returning login and declined permission; rotation interruption
and stale completion; or tool call/result/next-turn and failed-stream semantics.
Do not run generation, change account state, or restart the installed Relay
without task authorization. Measure authentication latency, generation latency,
wire bytes, token usage, and cache hits separately when optimizing.

Report verified behavior, intentional differences, and remaining live acceptance.
Skill/documentation changes need frontmatter, links, selection-case structure,
and diff checks rather than an application rebuild or provider request.
