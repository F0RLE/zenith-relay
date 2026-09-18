# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

### Changed

- Pool recovery is automatic: all compatible members are considered, temporary
  failures pause for at least five seconds, and repeated failures increase the
  cooldown. Unavailable models, rejected credentials, and opaque gateway
  rejections no longer prevent fallback to another eligible API or account.
  Removed the manual retry-count and last-candidate cooldown controls.

- Rewrote English and Russian Help around the current connection workflow,
  operating modes, pool rotation, quotas, balances, and recovery. Added a linked
  contents list and corrected obsolete settings paths and behavior descriptions.
  Help now has a reading column, a sticky section index with the current section,
  numbered setup steps, and error rows that fit narrow windows. The index becomes
  a compact menu on small screens.
  The error reference now groups exact codes with causes and concrete recovery
  steps, with expandable categories and search by code or symptom.

- Weekly quota reset uses a compact action row with a separate credit count
  in account and pool cards.

- Pool member rules open with a compact model list and one switch per model.
  Expandable provider groups retain disabled models; long lists support search.
  API prices share aligned columns beside each model, including 5-minute and
  1-hour cache writes; narrow windows use labeled price rows. Secondary settings use aligned rows
  on their own tab. Account and API model order now follows metadata and saved manual order
  without moving disabled models or prioritizing price overrides.

- Accounts and API providers now share three pool rotation modes: Smart,
  In order, and Round robin. One reorderable list replaces separate API roles,
  with member weights and shared request limits. Settings apply without
  interrupting active requests; unavailable members do not block the rest.
  Concurrent edits are detected before saving, and presets retain the mixed order.
  The editor uses one scrollbar, short status labels and mode-specific controls;
  centered fields use Request share and Concurrent requests, with Unlimited
  shown for an unset concurrency cap. Detailed explanations are in Help.
  Automatic modes show ready members first,
  while In order preserves your manual queue. Modes support keyboard selection.
  Smart ignores manual order, includes all similarly suitable members instead
  of limiting selection to three, and stops preferring stale quota readings.
  Recent failures lose their scheduling penalty within a minute, so recovered
  members can return automatically. Finishing an older request cannot release
  another recovery probe; occupied probes allow a bounded wait. Server pool
  membership changes immediately apply saved order and request limits.
  The next-candidate hint follows the scheduler and is omitted when the choice
  depends on the model or format. Stale activity events cannot overwrite a
  newer runtime's state.

- Pool controls now use a compact toolbar, a separate current/next route line,
  and a framed panel with a shaded status strip and dividers between counters.
  Connections uses the same panel styling for search, filters, and account actions.
  Both summaries keep the total provider credits when available. Full route
  and model names wrap on narrow screens; icon actions retain their tooltips.
- Pool request speed is now a single draggable, three-position control for
  Standard, Fast, and Ultrafast instead of a menu and a separate switch.
  Its compact label shows only the selected mode, with smooth transitions
  that respect reduced-motion preferences. Keyboard and touch selection are
  supported; dragging saves on release.

### Added

- Pools serve Responses, Chat Completions, Messages and Gemini concurrently,
  with native paths and supported conversions for JSON and streaming requests.
  Chat Completions supports function tools and their result history.
- New API sources determine formats automatically from provider declarations
  and endpoint settings. Unknown catalogs stay available for manual setup;
  generation is tested only with the explicit check button. Existing sources
  retain their manual routes. Model compatibility shows formats, capabilities
  and reasoning levels, distinguishing declared, verified and unknown support.
- OpenCode uses protocol-specific SDK groups and retains working model IDs and
  user options during catalog refresh. Codex uses HTTP streaming when a model
  needs conversion, while native WebSocket connections remain supported.

- API cards recognize Sub2API, New API, compatible One API billing, DeepSeek
  and SiliconFlow balances. OpenRouter statistics work with ordinary inference keys. Cards
  distinguish wallets, key allowances, subscriptions, currencies and Relay's
  own usage estimate, omit internal adapter labels and missing request counts, and mark failed refreshes
  without discarding the last known balance. Refresh progress uses the button
  animation without adding a duplicate status line below the counters.

- Failed request details now show the provider's original error code, type,
  message, and HTTP status separately from Relay's category. Messages can be
  copied; sensitive content is hidden and long messages are bounded. Older
  records explicitly show when no provider message was saved.
- **Diagnostics** in Settings. Relay keeps separate, size-limited and
  redacted logs for errors, crashes, and important operation stages; each
  folder can be opened directly from the app. Detailed operation logging is
  off by default and can be enabled there when troubleshooting.
- Local Cockpit-compatible account exports now preserve a safe account name
  and bounded, de-duplicated tags. Import accepts the same metadata without
  exposing credentials; existing Relay tags remain authoritative on reimport.

### Fixed

- ChatGPT account selection and quota reserve share a compact panel; feature
  switches use flat rows with concise descriptions. Pool member settings no
  longer add a second frame inside the dialog. The OpenCode tab subtitle now
  says "Use the pool in OpenCode". ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- GPT model names in ChatGPT and Codex keep their original IDs even when the
  selected account has no matching catalog card. Explicit key prefixes and
  existing aliases remain supported; capabilities come from the matching model.
  ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Responses tool-history recovery preserves call/result links when a client
  references an item ID or omits a call ID. Results are matched by kind and
  namespace without deleting history or guessing between parallel calls.
  HTTP, streaming and WebSocket use the same bounded recovery. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Pool rotation now saves mode, order and member settings immediately. Dragging
  works consistently, concurrent membership updates refresh the editor, and
  conflicting saves retry without restoring removed members. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Model rule actions share one aligned group with consistently sized format,
  reasoning, speed and enable controls. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Provider price fields use one rounded outline, a separated currency marker
  and numbers aligned to the right. Focus and invalid values highlight the
  whole field consistently in both themes. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Pool speed labels and connection summaries fit wider system fonts without
  clipping or unnecessary line breaks on desktop screens. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Multiple routes for one member share rotation weight and request capacity.
  An unsupported endpoint does not disable the member's other formats; quota
  and authentication failures remain shared. Continuation retries preserve
  complete history, tool results and opaque ownership.
- Converted requests reject unsupported parameters instead of dropping them.
  Schemas keep their constraints, per-turn instructions do not persist into
  later turns, and reasoning levels are offered only where they can be mapped.
  Streamed tools retain their IDs and order, and usage reflects the actual
  upstream format without inventing missing counters.

- Smart cache affinity no longer selects a member outside the best available
  group or bypasses its rotation accounting.
- Provider retry delays are respected for overloaded and unavailable models
  and other service errors, including when a shorter recovery delay is configured.

- Updated TLS handling to rustls 0.23.45 to address RUSTSEC-2026-0285.

- Pool no longer reports that every member is unavailable when routing telemetry
  is missing or stale but ready members remain. When none are ready, the warning
  separates quota waits, unavailable members, and disabled members, with readable
  error reasons. Pool and Connections group accounts by availability while
  preserving routing order within each group.
- Provider failures inside HTTP 200 JSON or streaming responses remain failures
  in usage history. Rate limiting and spend-limit errors stay distinct, and
  explicit request-validation errors do not put an account into cooldown.
- A provider's generic request rejection or disabled model no longer blocks
  fallback to another compatible member. Service errors on account routes keep
  their account attribution without marking the account unhealthy.
- Tool continuations validate the whole history, including calls beyond the
  first sixteen. Completed historic calls no longer select the owner of a new
  tool result, and outputs from different owners cannot be mixed implicitly.
  Examples in tool schemas and nested result data no longer affect routing.
- Account model refreshes no longer erase an independent block, sign-in, or
  verification failure, or downgrade a terminal catalog error after a temporary
  network failure. Quota-monitoring errors no longer hide the reason an account
  is unavailable in Connections and Pool.
- Pool activity matches the last-used account or API source by identity, so
  requests with identical timestamps cannot highlight the wrong member.
- Recovery preserves the original response binding for other chat branches
  and retries, and uses the current request options without reviving old
  instructions or transport settings. Provider-side
  conversation and item references, or tool results without their calls, no
  longer count as a complete local replay.
- Chats with a supplied compacted context can continue without a stale response
  reference over HTTP or WebSocket. The compacted window and retained tool
  results stay intact; unreadable compaction is no longer silently discarded.
- Chat recovery now requires a saved conversation chain before removing an
  earlier response reference unless a complete compacted window is supplied,
  preventing silent loss of prior context.
- Manual credential refresh and observed client logins restore account routing
  health. Remote account cards now reflect runtime cooldowns and unavailable routes.
- Recovered pool accounts remain available without removing another member.
  Delayed authentication updates no longer undo a newer account disable, and
  account cards reflect temporary runtime unavailability during cooldowns.
- Importing an account or API source without adding it to the pool no longer
  restarts the live local gateway. This prevents an unrelated listener restart
  from interrupting Relay during inventory-only imports.
- After an interrupted launch or import, Diagnostics now records the last
  redacted operation stage on the next start, even when detailed debug logging
  was disabled.
- A chat whose original account has run out of quota can now continue through
  the next compatible healthy account or API source. This works for both
  ordinary and WebSocket Responses requests.
- Chats can recover from an expired response reference using Relay's locally
  retained history, including on an already-open WebSocket. Complete tool
  history can rotate with the pool; incomplete tool state is no longer silently
  removed during recovery.
- Unknown response references are rejected before reaching an unrelated
  account. Clients can resend complete history without the old reference;
  otherwise Relay reports that the continuation context is unavailable.
- An account that needs sign-in, is cooling down, or has an error now disables
  only that candidate, including when its status changes while the local gateway
  is already running. The rest of the pool and its available models keep
  rotating normally.
- Account import and OAuth cleanup no longer leave stale temporary state that
  can close Relay or block the next import. A JSON account can be added to
  Relay without adding it to the pool.
- Refreshed account credits and quota limits are applied to the running pool,
  including when Relay is open only in the tray. Only real provider credits
  are shown as available balance.
- Background ChatGPT wake checks now use the live gateway runtime, including
  its token refresh, cooldown, usage, and diagnostics paths, while staying
  pinned to the account that the scheduler selected.
- A model can be returned to Standard speed while its route is temporarily
  unavailable. Fast and Ultrafast are recalculated for every retry candidate,
  so an unsupported speed is not carried to the next account or API source.
- Model Rules now shows only models that have a usable route. Stale ownership
  is released when a route becomes unavailable, so a compatible replacement
  can serve the chat.
- Saving a partial Model Rules reorder now preserves unavailable and
  binding-only pool models instead of treating them as removed.
- Pool and Connections cards use consistent account information and
  reauthentication controls. Unavailable accounts remain visible with their
  status instead of making the whole pool look unavailable.
- Closing the main window now hides Relay in the tray and reopens the same
  window instead of destroying it. The OAuth completion page no longer shows
  a close button that cannot work.

## [1.1.3] - 2026-09-10

<!-- relay-notes:en -->

Zenith Relay 1.1.3 improves account recovery, native ChatGPT integration,
model discovery, streaming resilience, and local data safety.

### Fixed

- Relay storage now uses separate durable database, vault, catalog, migration,
  temporary WebView/import, export, and recovery folders on Windows, macOS, and
  Linux. Startup safely migrates only Relay-owned flat files, removes the old
  legacy-marker file after relocating it, and refuses to overwrite a conflicting
  durable copy.
- ChatGPT history transfer now reconciles the desktop's local chat catalog as
  well as its state database, clears stale visibility markers, and can complete
  a prior partial transfer without rewriting already-correct rollout files. It
  keeps conversation chronology, handles legacy catalog rows with no host id,
  and includes the catalog in the existing rollback backup.
- A stale account import no longer overwrites a newer in-memory OAuth rotation.
  Relay persists and restarts from the authoritative credential state, while
  preserving a separate credential update that completed meanwhile.
- Relay-owned ChatGPT account discovery and requests now start with a stable
  Codex fallback and asynchronously follow the newest published stable Rust
  Codex release from the official OpenAI GitHub feed. Prerelease versions are
  ignored, and the selected version stays in process memory instead of being
  written to a local JSON cache; identity headers from a directly connected
  client remain preserved.
- Automatic Responses Lite now stays off unless every configured fallback route
  has confirmed native Lite support for the selected model. Mixed or partially
  known pools therefore preserve one full-Responses tool and reasoning-context
  contract across HTTP, WebSocket, and account-only retries; explicit client
  Lite requests are unchanged.
- Relay no longer exits during startup when the client-login watchdog starts
  before the desktop state is available; optimized desktop builds now open
  reliably.
- Launching a direct ChatGPT account now removes a previously managed Relay or
  external model catalog for that session. Codex can again load the account's
  native model names and its own available speed controls; the prior catalog is
  restored when switching away.
- Responses WebSocket streams no longer switch upstream or append a synthetic
  Relay failure after output has reached the client, including HTTP/SSE
  fallback. Disconnecting the client cancels fallback waits and releases the
  occupied route. Enabled ChatGPT recovery can keep waiting for a temporary
  route recovery without the former 30-second deadline.
- Compact Usage tables now keep Russian column headings readable while
  preserving timing and request identifiers, and keyboard focus remains
  visible on shared navigation and action controls.
- Requests carrying complete tool history can now leave a failed account after
  a pre-output streaming or transport error, instead of being blocked by their
  old tool affinity. Stateful continuations retain their original owner.
- Desktop and tray icon artwork now uses more of its transparent canvas, so
  Zenith Relay appears visually consistent with neighboring system icons.
- Codex profile attachment now removes the unsupported `persistent` reasoning
  effort from the desktop setting while Relay is active; profile restore keeps
  the user's original configuration intact.
- Reasoning catalogs now match decimal and dashed model versions, so Claude
  releases such as Fable 5.1 and Opus 4.8 retain the full OpenRouter effort
  enum instead of falling back to partial LiteLLM flags. Native ChatGPT
  `ultra` levels are preserved, while Messages bridges expose `ultra` only as
  the supported Codex alias translated to upstream `max`.
- Model capabilities now come from the merged public metadata catalog for both
  API sources and accounts, including ChatGPT/Codex and OpenCode profiles.
  Unknown models accept text and image input with text output, without invented
  reasoning, tools, or limits. Incomplete provider metadata no longer excludes
  image requests from pool routes. Native account Responses Lite settings
  remain isolated between accounts.
- Retryable 502/503 failures move to the next eligible pool route before response
  output begins; removed providers do not remain eligible for new requests.
- Usage history now attributes upstream overload and server failures on OAuth
  routes to the selected account instead of misidentifying it as an API
  provider. Provider-neutral category text remains accurate for both route
  kinds.
- Model Rules now shows the selected global Standard or Fast pool policy when
  an OpenAI-family model has no per-model override. Fast sends the upstream
  `priority` request tier; it is not a capability check or a speed guarantee.

- Responses tool continuations now retain the physical route that created both
  function and custom-tool calls. A transient owner failure no longer replays
  an orphaned tool output to another provider and surfaces a misleading
  tool-call mismatch.
- Reopening an older Codex chat on another model now recovers a stale
  custom-tool or function call that has no recorded output. Relay removes only
  the incomplete historical call and retries the new-model request instead of
  exposing the provider's `No tool output found` error.
- Responses bridges reject opaque compaction history with a specific
  compatibility error, allowing an eligible native Responses route to be
  tried without penalizing the bridge. A bridge also rejects a non-empty
  context-management request instead of silently dropping the client's
  compaction settings. Native requests preserve client-owned settings;
  experimental compaction is not enabled automatically.
- Hover hints now use Relay's themed tooltips throughout the interface instead
  of browser popups. Redundant hints are hidden when the full value is visible;
  truncated values and disabled-control explanations remain available.
- Source launch buttons now ask whether to open the selected API source in
  ChatGPT or OpenCode. OpenCode receives that source's exact endpoint,
  credential, and verified native Responses models instead of silently using
  the pool connection.
- Proxy assignment is now available from each account card's three-dot menu;
  the lower action bar no longer shows an edit pencil for this secondary action.
- Reset-quota refresh failures remain visible even when the consumed credit was
  the last available one, and reset diagnostics now use the shared redaction
  path for provider errors, URL credentials, and quoted secret fields.
- Pool profile switching no longer terminates the process tree of ChatGPT or
  OpenCode. Relay stops only the identified desktop process, so unrelated
  child processes cannot close Relay or another active desktop session.
- Model Rules and source pricing now show one group per company instead of
  separate family subgroups. Default model order puts newer releases first
  across families while preserving manually saved ordering.
- Model Rules is now limited to pool management: provider-dependent prices are
  no longer shown or edited there. Per-source pricing remains available in the
  source editor, where each API can keep its own prices.
- Relay now keeps setup-only streaming frames internal until a route produces
  real output or completes. A provider failure before that point falls back to
  another eligible route instead of appearing as a separate user-visible
  failed request.
- Responses custom-tool continuations now stay bound to the route that created
  the tool call. If a conversation is switched to another model or provider,
  Relay refuses the unsafe continuation locally instead of forwarding it to a
  provider that cannot recognize the tool-call id.
- Unknown pooled models whose provider does not advertise reasoning metadata can
  now be configured manually with Low, Medium, High, Extra high, and Max.
  Relay sends no reasoning setting until the user explicitly enables one.
  Models whose provider explicitly reports no reasoning support remain
  unavailable for configuration.
- Moved the update notification to the sidebar update row so it aligns with the
  application controls in both expanded and compact navigation.
- Automatic weekly quota reset now recovers when the secondary window was
  already exhausted before the refresh that detected it. Missing reset-credit
  metadata no longer prevents the authoritative reset-credit endpoint from
  checking availability.
- Pool routing status now keeps the last account that actually handled a
  request visible alongside the next eligible route, with a distinct card
  highlight for the last-used member.
- Runtime activity overlays are reconciled with fresh snapshots so an old
  release event cannot hide a newly active account or mislabel the next route.
- Changing Usage filters while a report is still loading can no longer replace
  the current report with a stale result or leave it empty. The API page also
  remains usable when a partially saved gateway address is malformed.
- Direct provider switching no longer rejects every routed model as having no
  compatible text models after Relay stops publishing a synthetic truncation
  limit. Provider-supplied truncation policies remain validated when present.

<!-- relay-notes:ru -->

Zenith Relay 1.1.3 улучшает восстановление аккаунтов, нативную интеграцию с
ChatGPT, обнаружение моделей, устойчивость потоковой передачи и безопасность
локальных данных.

### Исправления и улучшения

- Хранилище Relay разделено на устойчивые папки для базы, vault, каталогов,
  миграций, временных данных, экспорта и восстановления на Windows, macOS и
  Linux. Миграция переносит только файлы Relay, не перезаписывает конфликтующие
  данные и удаляет старый маркер после успешного переноса.
- Перенос истории ChatGPT теперь обновляет и локальный каталог чатов клиента,
  включая старые строки без `host_id`, очищает устаревшие маркеры видимости и
  сохраняет резервную копию для отката.
- Устаревший импорт аккаунта больше не затирает более новые OAuth-учётные
  данные из работающего Relay; актуальная авторитетная версия сохраняется и
  используется при перезапуске маршрута.
- Relay наблюдает фактический переход клиента на страницу входа и показывает
  предупреждение только у связанного аккаунта, не блокируя переключение
  аккаунтов по локальному сроку действия токена.
- Для собственных запросов ChatGPT Relay сразу использует стабильную резервную
  версию Codex, а после загрузки и затем раз в час асинхронно проверяет
  официальный GitHub-релиз Rust Codex. Бета-, альфа- и другие prerelease-версии
  игнорируются; выбранная версия хранится только в памяти процесса, без
  локального JSON-кэша. Идентификационные заголовки напрямую подключённого
  клиента сохраняются.
- При прямом подключении ChatGPT Codex снова получает нативные названия
  моделей и собственное управление скоростью; временный каталог Relay при
  этом корректно снимается и восстанавливается при смене профиля.
- Метаданные моделей обновляются из публичных каталогов для API-источников,
  аккаунтов, ChatGPT/Codex и OpenCode. Неизвестные модели не скрываются и не
  получают вымышленных лимитов, инструментов или reasoning-возможностей.
- Автоматический режим Responses Lite включается только при подтверждённой
  нативной поддержке на каждом запасном маршруте. Явный запрос клиента Lite
  остаётся без изменений.
- После появления ответа поток больше не меняет upstream и не добавляет
  искусственную ошибку Relay. До первого байта доступны безопасный failover,
  отмена ожидания при отключении клиента и ожидание временного восстановления
  ChatGPT без прежнего ограничения в 30 секунд.
- Продолжения с function/custom tools сохраняют физический маршрут-владельца.
  Неполные старые вызовы инструментов можно безопасно восстановить при смене
  модели, а небезопасное продолжение блокируется локально.
- Ошибки 502/503 до начала ответа переходят на следующий подходящий маршрут;
  удалённые источники не остаются доступны для новых запросов. Ошибки OAuth
  корректно относятся к аккаунту, а не к несуществующему API-провайдеру.
- Правила моделей показывают выбранную общую политику Standard или Fast для
  OpenAI-моделей без отдельного переопределения. Fast отправляет upstream
  режим `priority`, но не является проверкой поддержки или гарантией скорости.
  Порядок и группировка моделей стали стабильнее, а цены редактируются только
  у конкретного источника.
- Мониторинг кредитов, квот и еженедельного сброса устойчивее к временным
  ошибкам; наличие подтверждённого положительного или безлимитного кредита
  допускает аккаунт в пул без подмены расчёта стоимости.
- Статус пула сохраняет последний использованный маршрут рядом со следующим
  кандидатом и не теряет актуальную активность при поздних событиях. Таблицы
  Usage не затираются устаревшими ответами при смене фильтра.
- Интерфейс получил русские компактные заголовки, тематические подсказки,
  видимый keyboard focus, аккуратные действия прокси и обновлённые иконки.

## [1.1.2] - 2026-09-03

Zenith Relay 1.1.2 improves provider failover, usage visibility, application
integration, recovery, and the local-first desktop workflow.

### Security

- Local and user-managed server API keys are fetched only after an explicit
  **Copy API key** action. Relay does not render or retain them in the desktop
  interface.
- API keys can be reissued from the API page. The replacement is copied
  directly, and the previous key is invalidated only after the new one is ready.

### Routing, availability, and usage

- Relay preserves a ChatGPT session's preferred healthy route to retain upstream
  prompt-cache affinity, while still allowing bounded fallback when capacity,
  quota, or health requires it.
- Pool activity now follows the runtime's reported candidate, including a
  lower-priority stabilizer, and remains visible across concurrent state
  refreshes instead of predicting a "next choice" from the card order.
- A confirmed quota exhaustion cools down only the affected account/source and
  model route instead of taking the whole provider out of rotation.
- Namespaced API models stay on their declared API source; Relay no longer
  silently falls back to a ChatGPT subscription route for those requests.
- Unclassified provider rejections before response data reaches the client now
  cool only the affected source/model route and continue with the next eligible
  source. Known request, tool-call, context, and continuation errors remain
  terminal, and an active response is never switched mid-stream.
- Source capability refreshes use the live upstream catalog. OAuth refresh-token
  reuse is treated as a temporary recovery state rather than forcing an
  unnecessary sign-in.
- Adding an account or refreshing quotas now updates its available models in
  the same operation. API-source refresh updates balance, models, prices, and
  reasoning capabilities together, including an authoritative empty catalog.
- Temporary network and provider failures no longer label an account as
  blocked. Usage history keeps a stable, redacted label for removed accounts
  and reports the service tier observed from the upstream response.
- The pool now exposes two request-speed states: Standard and Fast. Pool-managed
  OpenAI requests follow the selected state, while external API clients retain
  their explicit tier; the upstream-reported tier remains a diagnostic value.
  The speed control and persisted per-model policy are limited to OpenAI-family
  models; legacy Fast overrides for other families are ignored safely.

### Pricing

- Replaced the hand-maintained OpenAI price file with one validated LiteLLM
  catalog shared by desktop and Relay Server. Account estimates use only an
  exact record in the account's declared official family; API sources resolve
  provider evidence, LiteLLM exact, declared-family canonical, then a manual
  source value.
- Kept input, cache-read, cache-write (5m/1h), output, and image/request
  tariffs independent. Missing components remain unpriced instead of falling
  back to another tariff or displaying `$0`.
- Usage now normalizes OpenAI-style inclusive cached input and
  Anthropic-style separate cache reads/writes into one breakdown. Cached and
  reasoning tokens are shown as subsets where required, zero-only rows are
  omitted, and totals no longer double-count those components.
- Added immutable catalog snapshots, local ETag/Last-Modified cache metadata,
  stale/offline fallback, atomic replacement, and revision-aware invalidation
  for usage and API-equivalent totals. Pricing remains informational and never
  changes routing or quota decisions.

### Model catalog

- Added a separate `models.dev` metadata catalog for provider, family, display
  name, release dates, and safe descriptive capabilities. LiteLLM remains the
  only source of pricing; metadata never adds models, enables routing, or
  changes reasoning and availability decisions.
- Model metadata is loaded from the local cache at startup and refreshed in the
  background with conditional HTTP validation. Stale or unavailable metadata
  remains usable, and backend-owned family/order data is shared by all model
  pickers without frontend name heuristics.

### Desktop UI

- Account cards show a **Credits** row and the pool shows **Total credits**
  when the connected provider explicitly reports a positive or unlimited
  ledger. Values use at most one decimal place; zero or missing ledgers stay
  hidden and do not affect billing or Relay's API-equivalent calculation. When
  quota headroom is otherwise tied, the pool prefers the larger fresh ledger
  before continuing its normal fair rotation.
- The Overview application launcher now only starts an already connected
  application; the connect-time launch preference is shown only during setup.
- ChatGPT recovery provides named, protected snapshots of `config.toml` and
  sign-in state with confirmed restore and deletion. Managed profile switching
  still preserves unrelated settings, rejects a newer manual sign-in, and uses
  reversible history repair in both directions.
- Pool connections now include OpenCode. Connecting writes a managed
  `zenith-relay` OpenCode provider with the currently enabled pool models and
  preserves the previous OpenCode configuration for one-click recovery. An
  authoritative empty pool now replaces stale OpenCode models with a zero-model
  catalog instead of leaving an obsolete selection behind.
- Relay-managed OpenCode models now advertise image attachments, so image
  inputs can be selected and forwarded through the pool.
- OpenCode reasoning variants follow the model's explicit Relay reasoning
  policy, so unsupported effort choices are not advertised to the client.
- The application picker remembers whether a connected application should be
  launched immediately, and its compact layout remains consistent across
  desktop window sizes.
- API setup separates the local endpoint, ChatGPT, and OpenCode responsibilities
  while keeping address, port, and copy-only key actions together. ChatGPT and
  OpenCode launch only when the user enables the remembered launch option.
- Tooltips now wait briefly for pointer hover, appear immediately for keyboard
  focus, and remain available for disabled actions without browser-native
  `title` popups.
- The startup screen and application chrome no longer allow accidental text
  selection, while editable fields remain selectable.
- Usage refreshes no longer wait on an extra UI delay, while sign-in, setup,
  and snapshot countdowns share one deadline-based clock instead of separate
  polling intervals.
- The Usage view warms its default report after the runtime is ready and keeps
  a small per-query result cache, so returning to the page renders immediately
  while the latest aggregates refresh in the background.
- Update discovery starts with the application instead of an arbitrary startup
  delay, and compact icon-only controls keep their accessible action names.
- Runtime and usage refreshes are limited to the pages that consume them;
  revision checks, coalesced events, cached aggregates, and bounded clocks avoid
  rebuilding long request tables or blanking Overview during background work.
- OpenCode recovery now mirrors the ChatGPT recovery view with an explicit
  original-config snapshot, creation date, path, and confirmed restore action.
- Relay-owned recovery files now use an application-first layout under
  `recovery/applications`, while legacy recovery directories migrate safely on
  startup without overwriting conflicts.
- Pool configuration can be exported without credentials, previewed as a diff,
  and applied only to unambiguous existing accounts and sources. Desktop and
  user-managed server imports share the same validation and legacy-field merge
  behavior.
- Help, planning, and release documentation now describe the current ChatGPT
  and OpenCode integrations and the actual local storage boundaries.

## [1.1.1] - 2026-08-28

Zenith Relay 1.1.1 is the maintenance release after 1.1.0. It improves
multi-protocol reliability, account discovery, cache-aware routing, quota
visibility, and the everyday desktop workflow without changing the product's
local-first security boundary.

### 1.1.0 -> 1.1.1 at a glance

| Area | Changes in 1.1.1 |
| --- | --- |
| Account access | More reliable ChatGPT subscription discovery, exact-account reauthentication, and preserved catalogs during temporary checks |
| Routing | Stable source ownership for continuations, safer WebSocket recovery, and cache affinity that survives restarts |
| Providers | More complete Responses bridges for Anthropic Messages and Gemini, including tools, streaming, images, thinking, and usage |
| Quotas | Weekly reset-credit automation and clearer separation between provider quota and Standard/Fast request speed |
| Usage | API-equivalent remaining estimate, request-count and E2E-speed analytics, and safer route diagnostics |
| Desktop UI | Clearer source tabs, reliable drag-and-drop policy editing, compact model rules, responsive dialogs, and refreshed help/screenshots |

### Account discovery and recovery

- Newly added ChatGPT subscriptions discover their model catalog with the same
  registered Codex authorization used by the runtime. Accounts that require
  Agent Identity now show quota and models together, with OAuth Bearer kept as
  a safe fallback when no Agent Identity is available.
- A temporary quota or model-discovery failure keeps the last usable catalog
  instead of making an account appear empty.
- Reauthentication can target the exact expired ChatGPT account. A fresh OAuth
  login keeps local routing and settings, and does not invent an expired
  subscription date without new provider metadata.

### Routing and provider compatibility

- Tool-call continuations from Messages and Gemini sources stay on the exact
  source route that created the response, preventing rotation from breaking an
  active task.
- Native Responses WebSocket requests recover strict provider-owned message
  identifiers in the same bounded way as HTTP. Parallel account-backed
  sessions keep their leases and response affinity independent.
- Relay-owned WebSocket timeouts and stream-size failures are reported as Relay
  errors instead of being attributed to a provider.
- Responses Lite follows the provider tool contract with explicit serial tool
  execution and rejects malformed values before forwarding them.
- Responses bridges now cover function, namespace, and direct custom tools
  across Anthropic Messages and Gemini, including tool-choice filtering,
  JSON/SSE continuations, multimodal input, thinking metadata, and normalized
  usage.

### Cache, quota, and usage

- Prompt-cache affinity keeps the original account preferred until a real
  failure, while source priority, exhaustion, health, and bounded spillover
  still apply. Opaque prompt/session bindings persist across Relay restarts,
  and rotating headers no longer split one session into separate cache keys.
- Server pools can automatically redeem an available reset credit when a
  configured weekly quota reaches zero, with per-account locking and cycle
  deduplication.
- The lifetime-based monetary "Potential" estimate is replaced by **API equiv.
  left**, shown only when Relay has complete priced usage for the current
  provider quota window. Activity outside Relay is excluded.
- Overview adds request-count and end-to-end output-speed charts for every
  selected period.
- Usage diagnostics show the attempt number, safe route kind, and endpoint
  route for each request, including failed attempts, without recording hosts,
  credentials, prompts, cookies, headers, or provider response bodies.
- Official OpenAI reference prices for GPT-5.6 Sol, Terra, and Luna now include
  cached-input and cache-write rates.
- Standard/Fast request speed is no longer presented as a second user-facing
  quota. Provider priority metadata does not create another quota meter.

### Desktop workflow

- Reopening ChatGPT and switching within the same adapter no longer rescans the
  full history. History repair now runs only when a profile crosses between
  OAuth, Relay, and API sources.
- API-source editing separates connection settings, model and format routing,
  and per-source pricing into focused tabs. Refresh checks only the saved
  source and stays beside its connection summary.
- Source policies support pointer-based reordering within roles and direct
  drops onto API-first, stabilizer, or last-resort roles. Saving closes the
  editor immediately while Relay persists the policy in the background.
- Pool model rules support pointer dragging with wheel scrolling, visible drop
  targets, and collapsible provider groups. Reasoning dialogs remain readable
  in compact and full-size windows with all backend-provided modes visible.
- README, localized Help, and Overview, Connections, Pool, and Usage screenshots
  were refreshed to match the current three-mode product.

## [1.1.0] - 2026-08-23

Zenith Relay 1.1.0 is the first complete Relay release after Zenith Codex
1.0.5. It changes the product from a small desktop API client into a
local-first personal relay for a user's own ChatGPT accounts and compatible API
sources. Relay is separate from the production Zenith Gateway, Control API, and
account pool.

### 1.0.5 -> 1.1.0 at a glance

| Area | 1.0.5 | 1.1.0 |
| --- | --- | --- |
| Product | Desktop client focused on a single API-key workflow | Local-first desktop relay with a private OpenAI-compatible endpoint |
| Operating modes | One desktop experience | This computer, Choose API, and My server |
| Accounts | Profile recovery and basic local state | ChatGPT OAuth, profile import, account health, quotas, pool membership, and routing |
| API sources | Limited source configuration | Responses, Messages, Chat Completions, and explicitly assigned Gemini routes |
| Models | Basic model presentation | Discovery, semantic ordering, capability-aware reasoning, and price provenance |
| Quotas | Status display | Provider windows, weekly reset credits, scheduled refresh, and confirmation-safe reset actions |
| Usage | Basic timing history | Token/cache/reasoning details, generation speed, E2E speed, and incremental totals |
| Recovery | Configuration repair | Snapshots, verified full restore, OAuth rotation recovery, and portable history repair |
| Deployment | Desktop release only | Cross-platform installers, signed updates, portable Windows replacement, and an optional user-managed server |

### Product and account management

- Added the three explicit Relay modes with local-first state, a generated
  loopback key, and capability-gated management of a server owned by the same
  user.
- Added ChatGPT OAuth sign-in, existing-profile import, account identity and
  availability state, pool membership, configured routing order, proxies, and
  reliable response continuity.
- Added provider quota windows in Connections and Pool. Provider quota,
  direct API-equivalent usage, and optional purchase-cost payback remain
  separate values; a quota percentage is never treated as money.
- Added explicit account export in several transfer formats. Account exports
  contain the OAuth credentials required for the selected import and must be
  handled as secrets. Diagnostics, snapshots, support bundles, telemetry, and
  usage history remain redacted: prompts, response bodies, cookies,
  authorization headers, and raw keys are not recorded there.

### Sources, models, and routing

- Added support for Responses, Messages, Chat Completions, and validated
  Responses-to-Gemini compatibility, including tool-call continuations.
- Added source model discovery, clear price provenance, image generation/edit
  prices, semantic model ordering, and declared reasoning catalog modes.
- Catalog refresh runs as an asynchronous conditional check at startup and
  approximately every 24 hours during an active app session. A deterministic
  spread prevents synchronized daily checks, while 5/30/120-minute retry
  deadlines remain exact after failures. Catalog failures stay visible after
  restart; reasoning modes remain catalog metadata and changing a reasoning
  setting does not probe a provider.
- Reasoning policies apply only to pooled API sources. Native OAuth models keep
  their provider capabilities unchanged.
- Added native WebSocket support and an HTTP/SSE compatibility path for
  providers that do not expose WebSockets.
- Routing follows the configured source order while keeping protocol
  continuations on the correct account.

### Quotas, resets, and usage

- Added explicit weekly reset-credit status and a simple Yes/No confirmation
  flow for an available reset. The automation path is weekly-limit aware; it
  does not confuse a five-hour window with the weekly reset.
- Background quota, model, and wake workers run only while an active Relay
  session is open. Tray-only startup does not perform provider checks.
- Added cache and reasoning token details, requested versus applied reasoning
  effort, provider generation speed, and full-request response speed.
- Usage totals remain available even after detailed request history is cleaned
  up according to its retention policy.
- Pool service tiers now use Standard/Fast terminology and synchronize Codex's
  official priority setting with the selected tier.

### Profile recovery and persistence

- Added reversible Codex profile attachment with one first-launch original
  snapshot, named snapshots, full restore verification, and a visible Yes/No
  confirmation. Hidden pre-restore copies are not created.
- OAuth rotation recovery adopts a newer token for the same account before
  restoring the profile, avoiding false restore failures.
- History repair updates only affected conversations and keeps recovery paths
  portable on Windows.
- Snapshot deletion and history-repair backups use bounded cleanup and explicit
  confirmation safeguards.

### Interface and desktop experience

- Added responsive English and Russian UI coverage for compact and desktop
  windows, a static startup screen, compact tables/cards, and shared dialogs for
  confirmations and errors.
- Model-availability and catalog errors remain visible instead of disappearing
  after a failed check. Global errors open in a centered details dialog and can
  be copied in a redacted form.
- Improved OAuth completion layout, source-route editing, usage request details,
  model price editing, pool card sizing, and semantic model-family ordering.
- Added signed in-app updates, in-place replacement and rollback for the
  portable Windows executable, and release artifacts for Windows, Linux, and
  macOS on x64 and ARM64.

### Optional Relay Server

- Added a standalone user-managed server with encrypted vault storage, durable
  state, management API, protocol negotiation, backup/restore, and strict
  redaction.
- The server is an optional personal deployment. It is not a connection to
  Zenith production systems, and live server acceptance remains a separate
  deferred gate.

### Security boundary

- Relay never receives Zenith production credentials, customer API keys,
  backend tokens, account-pool inventory, provider cabinet credentials, or
  internal Gateway/Control API business or routing logic.
- User-owned credentials can move only after an explicit confirmed transfer to
  that user's own server. Desktop secrets stay in the operating-system
  credential store; server secrets stay in the encrypted user-managed vault.

## [1.1.0-beta.1] - 2026-07-29

- First Zenith Relay product release, rebuilt from Zenith Codex 1.0.5.
- Added the local personal pool, compatible API sources, the three operating
  modes, reversible profile management, usage diagnostics, and the user-owned
  Relay Server.
- Added cross-platform desktop and server release artifacts, updater support,
  localized Help, and the initial production-readiness roadmap.
- This remains a beta: the real-account P0 acceptance path is not complete.

## [1.0.5] - 2026-07-07

- Added response timing to usage history.
- Fixed recovery from broken Codex configuration.
- Improved release version synchronization, updater-manifest publication, and
  release documentation.

## [1.0.4] - 2026-06-16

- Added API display balances.
- Standardized the main-branch and contribution flow for releases.

## [1.0.3] - 2026-06-09

- Published the third Zenith Codex release and fixed release-asset upload
  automation.

## [1.0.2] - 2026-06-07

- Published the second Zenith Codex release with the established desktop
  release artifacts.

## [1.0.1] - 2026-06-07

- Published the first maintenance release of Zenith Codex.

## [1.0.0] - 2026-06-06

- Initial Zenith Codex desktop release.

[Unreleased]: https://github.com/F0RLE/zenith-relay/compare/v1.1.3...main
[1.1.3]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.3
[1.1.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.2
[1.1.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.1
[1.1.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0
[1.1.0-beta.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0-beta.1
[1.0.5]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.5
[1.0.4]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.4
[1.0.3]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.3
[1.0.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.2
[1.0.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.1
[1.0.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.0
