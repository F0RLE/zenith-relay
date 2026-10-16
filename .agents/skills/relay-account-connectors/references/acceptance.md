# Account connector acceptance

Select the cases affected by the change and inspect existing coverage first.
Use synthetic callbacks, credentials, and provider responses. These are
observable acceptance criteria, not a claim that every connector implements
them. Run or extend checks only within task and harness permissions.

## Authorization completion

| Case | Required observation |
| --- | --- |
| Valid callback | One exchange and one committed connection, with the correct provider and issuing client. |
| Wrong/missing state, duplicate code/state, foreign redirect, or oversized callback | Rejected before exchange and persistence; an unrelated callback cannot consume a valid pending attempt. |
| Matching provider denial | A specific redacted terminal outcome; no saved credential replacement. |
| Callback after cancel/expiry, duplicate completion, or token reply after cancellation | No new account, membership, or overwritten credential; temporary attempt material is cleaned up. |
| Restart/resume | Only an unexpired, valid checkpoint resumes; another flow cannot reuse its verifier or callback. |
| Required callback port unavailable | Use only a provider-supported fallback or a clear retryable failure; do not change a fixed registered redirect. |

## Device authorization

| Case | Required observation |
| --- | --- |
| `authorization_pending`, including an endpoint that reports it in HTTP 200 | Continue at the allowed interval; no "signed in" state. |
| `slow_down` | RFC 8628 flows add five seconds to this and later poll intervals; provider-specific extensions retain their documented contract. |
| Denial, expiry, cancellation, or another terminal error | Polling stops; no automatic new authorization attempt. |
| Timeout or token-endpoint throttle | Poll rate decreases and respects deadlines; no tight loop. |
| Success | Validate required token fields before storage; a user code or device code is never treated as an access token. |

## Reauthentication and import

| Case | Required observation |
| --- | --- |
| Same principal/client, changed email | Update the same connection using stable identity; retain notes, proxy, settings, and membership. |
| Same principal, different issuing client | Separate connection and refresh flow; never relabel one token set as the other. |
| Targeted reauthentication with another principal/client | Clear conflict; the selected account and its credentials stay intact. |
| Remote-owned or trashed target | Respect the owning management/trash flow; do not recreate a local active registration. |
| Unsupported provider/format or malformed credential | Specific redacted rejection before credential mutation. |
| Import missing refresh token or expiry | Retain an existing value only under the same credential identity and provider contract; otherwise expose access-only/unknown state. |
| Partial import followed by cancellation | Report committed versus skipped/failed items accurately; do not silently roll back a newer login. |
| Secret write, metadata commit, or runtime activation failure | Existing checkpoint/conditional rollback preserves prior state; no false success and no overwrite of newer work. |
| Login succeeds but discovery/quota times out | Preserve the validated login; represent monitoring failure separately without inventing model access or quota. |

Inspect the existing `accounts/oauth/tests.rs`, `oauth_flow/tests.rs`,
`commands/oauth/tests.rs`, import reconciliation, and persistence coverage.
Document any changed auth grant, storage format, or user step in its existing
owner. Leave an unimplemented connector or unperformed live acceptance in
ROADMAP rather than describing it as shipped.
