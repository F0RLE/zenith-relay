# SIWC sources

Read the section relevant to the requested operation. Official SIWC documentation
sets the integration contract; repository examples supply implementation ideas
and regression scenarios. A pinned revision is reproducible evidence, not a claim
about the latest release. Fetch the relevant page or file rather than relying on
search excerpts, and retain its URL and inspected revision in the review evidence.

## Official contract

| Question | Source |
| --- | --- |
| Which integration is this, and what are a client and host? | [Overview](https://developers.openai.com/siwc/token-sharing-open-source) |
| Registration, loopback callback, identity, returning sign-in | [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in) |
| Profiles, rotation, revocation, usage settings | [Accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions) |
| Token fields, expiry, opaque claims | [Token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference) |
| Account catalog and completed inference | [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) |
| Allowed request options, tools, inputs, and continuation | [Preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations) |
| Permission refusal, usage limits, routing, and refresh errors | [Errors and recovery](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery) |
| Using an app-server with a SIWC grant | [Codex app-server](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server) |
| Sign-in and usage controls | [UI/UX guidelines](https://developers.openai.com/siwc/ui-ux-guidelines) |

Use the general Responses, prompt-caching, reasoning, or WebSocket documentation
only alongside the SIWC-specific restrictions for that feature. Do not freeze
model names, eligibility plans, token lifetimes, unsupported-field lists, or
telemetry availability in this skill; verify their current contract when needed.

## Official implementation examples

### OpenAI Sign-in with ChatGPT DevKit

- [Repository](https://github.com/openai/sign-in-with-chatgpt-devkit) and
  [integration cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt).
- `packages/local/src/oauth.ts`: callback, identity verification, token shapes,
  registration persistence, rotation recovery, and revocation.
- `packages/local/src/index.ts`: profile selection, refresh timing, and ownership.
- `packages/local/src/models.ts` and `responses.ts`: account-specific discovery
  and stream completion/error handling.
- `packages/react` and `examples/paste-perfect`: UI/IPC separation and user controls;
  these do not justify replacing Relay's Rust credential owner or established UI.

Inspect the revision's [LICENSE](https://github.com/openai/sign-in-with-chatgpt-devkit/blob/main/LICENSE)
and dependency/asset notices before reuse. Do not assume an OpenAI repository is
MIT- or Apache-licensed. Documentation and sample code can differ in optional
choices, including browser hints; evaluate privacy and behavior rather than
treating every sample detail as a provider requirement.

### OpenAI Codex

[Repository](https://github.com/openai/codex). Resolve the actual authentication
mode before interpreting any `SIWC` symbol. Useful research locations include
`codex-rs/model-provider/src/models_endpoint.rs` and
`codex-rs/backend-client/src/client/chatgpt_turn_cost.rs`.

An internal Codex/WHAM usage-estimate endpoint is not evidence of a public quota
API for a dynamic SIWC client. Check its credential, workspace, request IDs, units,
settlement completeness, and missing-value semantics before proposing adoption.

## Third-party comparison targets

| Project | Targeted question |
| --- | --- |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) | `internal/auth/codex/siwc_auth.go`, management `oauth_siwc*`, and `docs/management-api-v8.md`: binding, cancellation, persistence, and non-secret status. Its nested Codex/SIWC credential layout is not Relay's account model. |
| [Cockpit Tools](https://github.com/jlcodes99/cockpit-tools) | `src-tauri/src/modules/codex_native_api/chatgpt_plan_request.rs`: request-local compatibility, reasoning updates, header isolation, and regression cases. Verify current provider support before adopting a transform. |

For each comparison, inspect the file at an identified commit, its callers/tests,
and applicable license. Compare the exact behavior with Relay's current owner;
do not import a parallel auth manager, proxy stack, or unsupported normalization.
Search additional repositories only to answer a specific remaining question.
Repository installation, executing downloaded code, and authenticated provider
probes are separate operations, not prerequisites for reading public sources.
