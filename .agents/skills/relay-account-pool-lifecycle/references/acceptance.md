# Account pool acceptance

Inspect existing coverage and select affected transitions. Use synthetic
accounts, token generations, quota observations, and request ownership.
Run or extend checks only within task and harness permissions. These cases
define acceptance; they do not claim that live compatibility was established.

## Admission and observations

| Case | Required observation |
| --- | --- |
| New login | Saved outside the pool; explicit membership follows the connector's policy. |
| Native ChatGPT versus Excel/BPS | Separate issuing-client refresh and generation paths. Plan names do not block admission; actual provider access and quota checks still apply. |
| Unknown/missing plan, balance, quota, or reset | Retain unknown state; do not fabricate paid access, zero usage, or a reset window. |
| Discovery without access evidence | Inventory remains visible without making unsupported account/model routes executable. |
| Same ledger through two login clients | One credit total, separate auth connections and routing ownership. |
| Account disabled, draining, or remotely owned | Automatic recovery cannot re-enable it, change membership, or admit remote-owned secrets to the local pool. |
| Late model/quota reply after relogin, configuration change, trash, or delete/re-add | Revision mismatch prevents updating the replacement registration. |

## Renewal races

| Case | Required observation |
| --- | --- |
| Concurrent requests plus monitoring/renewal | One coordinated refresh for the current credential; waiters use its validated result. |
| Rotating refresh grant | New token set/generation is persisted before durable success; the obsolete grant is not used again. |
| Persistence failure after rotation | A later attempt retries pending persistence, not another exchange with the consumed grant. |
| Missing refresh token | Usable access remains usable; expiry exposes the existing access-only/reauthentication path. |
| Timeout, 429, malformed JSON, or invalid token-client hints | Saved credentials survive; classification and retry policy remain specific to the provider failure. |
| New login while an old refresh succeeds or returns `invalid_grant` | The new login wins; old success cannot overwrite it and old failure cannot require its reauthentication. |
| Delayed generation 401 after rotation or relogin with identical token text | Invalidate only the rejected incarnation; retain the current grant. |
| Removal/re-add with the same account ID | No old authority slot, refresh response, or rollback writes into the new registration. |
| Terminal grant error for the current login | Clear redacted reauthentication state; no fabricated recovery or automatic new login. |

## Execution and removal

| Case | Required observation |
| --- | --- |
| Account throttled with reset/Retry-After | Correct account/model scope and deadline; other connections retain their own policy. |
| Auth recovery after a generation failure | Recover credentials only when allowed; no replay/fallback after unknown or completed execution. |
| Opaque continuation, unresolved reference, or unpaired tool result | Keep account/route ownership unless validated local replay materializes complete history; unavailable ownership returns the specific recovery outcome. |
| Complete portable call/result history without an opaque predecessor | Allow normal eligible-member selection; soft affinity must not become a hard account pin. |
| Completed response fails conversion | Terminal conversion error; do not blame credentials, refresh, or send another generation. |
| Membership commit/runtime update fails | Existing rollback restores only its own mutation; newer user changes survive. |
| Trash during monitoring or dispatch preparation | Routing/monitoring eligibility ends and pending work is fenced; credentials remain recoverable in trash. |
| Restore/permanent delete | Restore follows the existing disabled/outside-pool contract; permanent delete removes owned secrets and late work cannot recreate them. |

Existing coverage starts in `accounts/token_authority/tests/`, desktop
`accounts/authority/tests.rs`, `accounts/operation_tests/`, import persistence,
`runtime/tests/` and scheduler ownership tests.
For account routes requiring conversion, also use the protocol skill's
text/tool/result/continuation acceptance.
