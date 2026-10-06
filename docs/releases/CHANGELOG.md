# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

<!-- relay-notes:en -->

- Account import recognizes Cockpit Tools JSON, and account export can create Cockpit Tools files with safe account metadata.

- Direct ChatGPT OAuth clears Relay provider and catalog overrides from the root and named Codex profiles, then lets Codex discover its native models. Relay pool activation clears stale named-profile overrides and uses the validated catalog from Relay's live model endpoint. Profile backups and credential snapshots stay in Relay recovery storage and the OS secret store, outside the Codex directory; profile or account-token changes invalidate Codex's model cache.

<!-- relay-notes:ru -->

- Импорт распознаёт JSON Cockpit Tools, а экспорт создаёт файлы Cockpit Tools с безопасными метаданными аккаунта.

- Прямой вход ChatGPT OAuth очищает провайдеры и каталоги Relay в корневых и именованных профилях Codex, после чего Codex сам получает список своих моделей. При включении пула Relay удаляет устаревшие переопределения в именованных профилях и использует проверенный каталог из актуального списка моделей Relay. Резервные копии профиля и учётных данных хранятся в восстановлении Relay и системном хранилище секретов, вне папки Codex; при переключении профиля и обновлении токена сбрасывается отдельный кэш моделей Codex.

## [1.1.5] - 2026-10-05

<!-- relay-notes:en -->

Zenith Relay 1.1.5 changes pool rotation, keeps ChatGPT account continuity, labels API errors, and expands request usage details.

### Changed

- Automatic pool rotation spreads concurrent requests across the least-loaded eligible members, then ranks by fresh quota and provider credits. Session affinity stays until another equally loaded member leads by 15 credits; pool cards show active request counts.
- Pool rotation has Automatic and Manual modes. Manual cycles through the saved member order, skips unavailable members and concurrency limits, and wraps to the start. Automatic keeps request weights and per-member limits.
- In Automatic mode, pool cards with no positive quota window sort by fresh provider-reported credits. API source wallet balances do not affect the order.
- Quick Setup places the Relay mark between connection sources and compatible applications, with window controls in a frameless, draggable top area.
- The pool rotation dialog no longer repeats mode explanations. The behavior is in Help.
- Account login notes save in order, keep their fields through reauthentication, and do not show a success message for a failed or closed save. Reauthentication replaces the stored token, keeps local notes, and does not open new-account pool setup.
- Direct ChatGPT launch is disabled for accounts that need sign-in or have a terminal credential, proxy, or health failure. Relay validates credentials before stopping an existing ChatGPT session and restores the session after a failed profile change.
- Newly signed-in OAuth accounts stay outside the pool until explicitly selected. The pool option has no green selection border.
- If a ChatGPT account rejects encrypted reasoning or compaction context, Relay removes the rejected ciphertext and bound id on that same account, keeps the visible summary and remaining history, and retries once. If that account cannot accept the retry, Relay drops the temporary binding and continues with another eligible member.
- Basis Points honors `parallel_tool_calls: false`. If the provider returns multiple client tool calls, Relay asks it to regenerate once and returns an error if the next response still violates the serial setting.
- Chat API errors begin with an English `Account:`, `Provider:`, or `Relay:` prefix in JSON, Messages, Relay Server responses, SSE, WebSocket, route and method failures, and request details.
- Usage details show whether a request used HTTP or WebSocket and provide an informational API-equivalent estimate from published model rates, observed service tier, and input-context band. Missing prices remain unknown. Account plan labels recognize Pro 100, Pro 200, and Pro 500.
- Relay no longer applies fixed byte caps to JSON generation, image upload or edit, provider responses, SSE events, translated streams, or alpha search results. Providers may still reject large requests or responses.
- A stale credit reading no longer overrides a proven rate limit. Fresh provider credits can still keep an account with an empty quota window schedulable.
- Usage price refresh replaces an older row when sample or cache-write counters increase, not only when token counters do.

<!-- relay-notes:ru -->

Zenith Relay 1.1.5 меняет ротацию пула, сохраняет продолжение чатов ChatGPT, помечает источник ошибки API и добавляет сведения об использовании запросов.

### Изменения

- Автоматическая ротация распределяет параллельные запросы по наименее загруженным участникам, затем учитывает свежую квоту и кредиты провайдера. Привязка сессии сохраняется, пока другой участник с такой же загрузкой не опередит на 15 кредитов; карточки пула показывают число активных запросов.
- В ротации пула два режима: «Автоматически» и «Вручную». Ручной режим проходит сохранённый порядок по кругу, пропускает недоступных участников и достигших лимита запросов. Автоматический режим сохраняет долю запросов и индивидуальные лимиты.
- В автоматическом режиме карточки без положительной квоты сортируются по актуальным кредитам провайдера. Баланс API-источника на порядок не влияет.
- В быстрой настройке логотип Relay стоит между подключениями и совместимыми приложениями. Кнопки окна остаются в перетаскиваемой верхней области без полосы.
- В окне ротации нет пояснений режимов. Поведение описано в справке.
- Заметки входа сохраняются по порядку, не пропадают после повторного входа и не показывают успешное сохранение при ошибке или закрытии окна. Повторный вход заменяет сохранённый токен, сохраняет локальные заметки и не открывает настройку пула для нового аккаунта.
- Прямой запуск ChatGPT отключён для аккаунтов, которым нужен повторный вход или у которых есть окончательная ошибка учётных данных, прокси или состояния. Relay проверяет учётные данные до остановки уже запущенного ChatGPT и восстанавливает сессию после неудачной смены профиля.
- Новые OAuth-аккаунты остаются вне пула, пока их явно не выберут. У варианта пула нет зелёной рамки выбора.
- Если аккаунт ChatGPT отклоняет зашифрованный контекст рассуждений или сжатия, Relay удаляет отклонённый шифротекст и связанный id на этом же аккаунте, сохраняет видимое краткое содержание и оставшуюся историю и один раз повторяет запрос. Если этот аккаунт не может принять повтор, временная привязка снимается и запрос продолжается другим подходящим участником.
- Basis Points соблюдает `parallel_tool_calls: false`. Если провайдер возвращает несколько клиентских вызовов инструментов, Relay один раз просит сформировать ответ заново и возвращает ошибку, если следующее сообщение всё ещё нарушает последовательный режим.
- Ошибки Chat API начинаются с английской метки `Account:`, `Provider:` или `Relay:` в JSON, Messages, ответах Relay Server, SSE, WebSocket, ошибках маршрута и метода, а также сведениях о запросе.
- В сведениях об использовании указан транспорт запроса, HTTP или WebSocket, и приведена информационная оценка эквивалентной стоимости API по опубликованным ценам модели, сообщённому режиму обслуживания и размеру контекста на входе. Неизвестные цены остаются неизвестными. Метки планов аккаунта распознают Pro 100, Pro 200 и Pro 500.
- Relay больше не задаёт фиксированные ограничения размера для JSON-генерации, загрузки и редактирования изображений, ответов провайдера, событий SSE, преобразованных потоков и результатов alpha search. Провайдеры по-прежнему могут отклонить большой запрос или ответ.
- Устаревшие кредиты больше не отменяют подтверждённый лимит. Свежие кредиты провайдера по-прежнему оставляют аккаунт без окна квоты доступным для выбора.
- Обновление цен использования заменяет старую строку, когда растут счётчики семплов или записи кэша, а не только токенов.

## [1.1.4] - 2026-10-03

<!-- relay-notes:en -->

Zenith Relay 1.1.4 improves quota-based rotation, checks the model reported by ChatGPT account routes, adds images to Basis Points, and makes launcher controls more responsive.

### Changed

- Automatic rotation picks the member with the largest fresh quota remainder. One point is enough to switch. Load is compared only when the remainder is equal. A chat stays on its member when the saved history cannot be resent to a member with more quota left.
- OpenAI models stay in the order Astra, Sol, Terra, then Luna. Anthropic models stay in the order Fable, Opus, Sonnet, then Haiku. A newer version of the same family keeps that place.
- An API price uses the provider price first. If the provider does not send one, Relay uses the official family price: GPT, ChatGPT, and Codex use OpenAI; Claude uses Anthropic; Gemini uses Google; Grok uses xAI. A manual price is used only when neither exists.
- The API tab can block a ChatGPT account route when it reports an internal degrade id or a different model. Relay checks response.created, subsequent SSE events, buffered responses, and WebSocket turns, including reused connections. A rejection before generation can select another member; a mismatch after generation stops the response without replaying it. This checks reported identity, not the model's intelligence.
- The tool-optimization switch is removed. Tool lists leave Relay exactly as the client sent them. A previously saved optimized mode no longer changes new requests.
- Switching an account keeps the official ChatGPT catalog and its official models. A direct API connection uses the models reported by that provider. Known reasoning levels come from the catalog parser and are connected directly, without the pool rewriting them.
- The pool picker filters working, cooldown, unavailable, disabled, and server connections. The status color matches the account card. The automatic queue is the left-to-right order of the cards, not a text list.
- Model speed is shown as icons. A reasoning control appears only for a text model that has reasoning levels, and it sits after the speed icons.
- On macOS, close, minimize, and full screen are on the left and do what those buttons normally do. The name and logo stay on the right.
- A model switch saves without waiting for a full snapshot. Other switches keep their new value while a background refresh runs, so the rest of the window stays usable.
- ChatGPT account requests use current Chrome headers. Quota rejection keeps the account recoverable and applies to the open primary window instead of marking the account broken.
- Excel / Basis Points accepts an image inside a user message. A data URL is uploaded with the account and replaced by file_id. A remote image URL is rejected, and detail is not forwarded. One encrypted-content failure drops the foreign ciphertext and retries the same account.
- Excel / Basis Points keeps long history identifiers within the upstream limit, so a tool call still matches its output. A visible reasoning summary stays even without ciphertext. Maximum reasoning is sent as the extra-high level this route supports.
- A blank ChatGPT model-catalog response no longer removes models the account already reported. A failed model-list refresh says that the list could not be refreshed, and a later local failure does not hide the provider error already recorded for that account.
- Basis Points model-access and usage-policy refusals keep their original 403 status. A model-access refusal pauses that model on the affected member; it does not ban the whole account. A usage-policy refusal ends the request without marking quota exhausted or trying other accounts to bypass it.
- Identity checks run before protocol conversion, so an adapter cannot hide a different upstream model by writing the requested name into its response. Error codes and safe explanations survive conversion to the client's protocol.
- Buffered account streams finish as soon as the terminal response arrives, even when the provider keeps the connection open. Split SSE events are inspected together, and oversized uninspectable events stop the stream.
- A failed terminal response that already contains generated output cannot trigger history repair or another model execution. Unknown disconnects also stay failures instead of being silently replayed.
- The SSE text-delta path recognizes ordinary deltas without building full JSON trees. Terminal usage, reasoning tokens, cache counters, and provider-reported service tier still pass through the normal accounting path.
- Pool card order reflects eligibility for the selected model before quota ranking. An account that cannot serve that model does not appear ahead of a usable API source just because its general account status is healthy.
- Failed setting changes roll the switch back and release its busy state, including when saving throws before returning a promise. Tabs and other controls remain usable during independent background operations.
- Launcher keyboard focus no longer adds the unwanted outlines or inset shadows. Change notifications keep their compact box without an extra border or shadow.
- Runtime snapshots reuse account facts and shared projections instead of repeatedly copying credentials and rebuilding the same state. Desktop and Relay Server use the same core definitions for account status and source summaries.
- Both Help languages explain route mismatch, model-access refusals, usage-policy blocks, continuation failures, and the corresponding recovery steps. These release notes are included in the in-app updater in English or Russian according to the selected language.

<!-- relay-notes:ru -->

Zenith Relay 1.1.4 улучшает ротацию по квоте, проверяет модель в ответах аккаунтов ChatGPT, добавляет изображения в Basis Points и ускоряет реакцию элементов лаунчера.

### Изменения

- Автоматическая ротация выбирает участника с наибольшим свежим остатком квоты. Одного пункта достаточно для переключения. Нагрузка сравнивается только при равном остатке. Чат остаётся на своём участнике, если сохранённую историю нельзя переслать участнику с большим остатком.
- Модели OpenAI остаются в порядке Astra, Sol, Terra, затем Luna. Модели Anthropic остаются в порядке Fable, Opus, Sonnet, затем Haiku. Новая версия того же семейства занимает это же место.
- Цена API сначала берётся у провайдера. Если провайдер её не прислал, Relay использует официальную цену семейства: GPT, ChatGPT и Codex относятся к OpenAI, Claude к Anthropic, Gemini к Google, Grok к xAI. Ручная цена нужна только когда обеих нет.
- На вкладке API можно блокировать маршрут аккаунта ChatGPT, если он сообщает внутренний degrade-id или другую модель. Проверяются response.created, последующие события SSE, собранные ответы и ходы WebSocket, в том числе на повторно используемом соединении. Отказ до генерации позволяет выбрать другого участника; подмена после генерации останавливает ответ без повторного выполнения. Проверяется заявленная модель, а не её интеллект.
- Переключатель оптимизации инструментов убран. Список инструментов уходит так, как его прислал клиент. Сохранённый оптимизированный режим больше не меняет новые запросы.
- Переключение аккаунта сохраняет официальный каталог ChatGPT и его официальные модели. Прямое API-подключение берёт модели, которые сообщил этот провайдер. Известные уровни размышления берутся из разбора каталога и подключаются напрямую, без переписывания пулом.
- При добавлении в пул можно отфильтровать рабочие, в кулдауне, недоступные, отключённые и серверные подключения. Цвет состояния совпадает с карточкой аккаунта. Автоматическая очередь видна порядком карточек слева направо, а не текстовым списком.
- Скорость модели показана иконками. Выбор размышления появляется только у текстовой модели, у которой есть уровни, и стоит после иконок скорости.
- На macOS закрытие, сворачивание и полный экран стоят слева и выполняют свои обычные действия. Название и логотип остаются справа.
- Переключение модели сохраняется без ожидания полного снимка. Остальные переключатели сохраняют новое значение, пока идёт фоновое обновление, поэтому остальным окном можно пользоваться.
- Запросы аккаунта ChatGPT отправляются с актуальными заголовками Chrome. Отказ по квоте оставляет аккаунт восстановимым и применяется к открытому основному окну, а не помечает аккаунт сломанным.
- Excel / Basis Points принимает изображение в сообщении пользователя. Data URL загружается от имени аккаунта и заменяется на file_id. Внешняя ссылка отклоняется, поле detail не пересылается. Одна ошибка зашифрованного содержимого убирает чужой шифротекст и повторяет запрос тем же аккаунтом.
- Excel / Basis Points удерживает длинные идентификаторы истории в допустимом пределе, поэтому вызов инструмента по-прежнему совпадает со своим ответом. Видимое краткое содержание размышления сохраняется даже без шифротекста. Максимальное размышление отправляется как экстра-высокий уровень, который поддерживает этот маршрут.
- Пустой ответ каталога моделей ChatGPT больше не удаляет модели, которые аккаунт уже сообщил. Ошибка обновления списка моделей говорит, что список не удалось обновить, а поздняя локальная ошибка не скрывает уже записанную ошибку провайдера.
- Отказы Basis Points по доступу к модели и правилам использования сохраняют исходный статус 403. Отказ доступа временно приостанавливает эту модель у конкретного участника, а не блокирует весь аккаунт. Отказ по правилам использования завершает запрос, не обнуляет квоту и не перебирает другие аккаунты для обхода ограничения.
- Модель проверяется до преобразования протокола: адаптер не может скрыть другую модель провайдера, записав в ответ запрошенное имя. Код ошибки и безопасное объяснение сохраняются в протоколе клиента.
- Сборка ответа аккаунта заканчивается сразу после завершающего события, даже если провайдер держит соединение открытым. Разбитые на части события SSE проверяются целиком; слишком большое событие, которое нельзя проверить, останавливает поток.
- Ошибка завершения, в которой уже есть сгенерированный ответ, не запускает восстановление истории с повторной генерацией. Обрыв с неизвестным результатом также остаётся ошибкой, а не скрытым повтором запроса.
- Быстрый путь текстовых SSE-дельт распознаёт обычные события без построения полного JSON-дерева. Итоговое использование, токены размышления, счётчики кеша и сообщённый провайдером класс обслуживания по-прежнему учитываются обычным способом.
- Порядок карточек сначала учитывает доступность выбранной модели, затем квоту. Аккаунт без этой модели не оказывается впереди подходящего API-провайдера только потому, что в целом он исправен.
- Если настройку не удалось сохранить, переключатель возвращается назад и перестаёт висеть в загрузке — в том числе при мгновенной ошибке команды. Вкладки и остальные элементы остаются доступны во время независимых фоновых операций.
- Фокус с клавиатуры больше не добавляет нежелательные обводки и внутренние тени в лаунчере. Уведомления об изменениях сохраняют компактный блок без дополнительной рамки и тени.
- Снимки состояния повторно используют сведения об аккаунтах и общие представления вместо лишнего копирования учётных данных и одинаковых вычислений. Desktop и Relay Server используют общие определения статусов аккаунтов и сводок провайдеров.
- В справке на обоих языках описаны несовпадение модели, отказы доступа, блокировки по правилам использования, ошибки продолжения и способы восстановления. Описание этой версии попадает во встроенное обновление на русском или английском согласно выбранному языку.

## [1.1.3] - 2026-09-30

<!-- relay-notes:en -->

Zenith Relay 1.1.3 keeps a ChatGPT account available after a generic provider
403, restores Excel tool history, shows an approximate cache lifetime beside usage tokens, and
uses the same compact controls across the launcher.

### Changed

- ChatGPT sign-in opens in a Relay window instead of the system browser. The
  account proxy is used for that window and for the token exchange. A live
  ChatGPT session finishes sign-in by itself; otherwise it is finished in the
  window. Saved login notes are not filled in. The lock inside the new-sign-in
  button chooses a saved HTTP proxy for that first login; it does not pick one
  at random.

- Excel / Basis Points tool instructions follow adapter v0.2.8. Examples use
  only tools allowed in the current request and match their declared type and
  schema, including the two JSON layers for function arguments. A separate
  developer reminder repeats the transport rule and keeps custom-tool input
  raw. The one-shot retry hint is appended after the prepared input, before a
  compaction trigger. Structured `text.format` is rejected instead of being
  dropped. Ordinary `service_tier` values `auto`, `default`, and `standard`
  stay on this route; Fast still does not. Image upload from the upstream
  plugin is not copied.

- An image stream that ends incomplete, failed, or without a real completion
  is reported as a failure instead of a finished image.

- A failed provider-balance refresh keeps the last amount. The selected API
  overview marks it **Not refreshed**. A pool card keeps the amount without
  that label; the reason stays in diagnostics.

- The API tool optimization switch now says that schemas open on demand for a
  normal Responses route, while Excel and other routes still send the full list.

- The API switch for ChatGPT accounts in the pool is now **Use Basis Points**.
  It sends those accounts through Excel instead of Responses and may help a
  degraded account generate, without guaranteeing the result.

- Closing the main window now releases its WebView and leaves the pool running
  in the tray. Opening Relay creates the window again instead of reusing the
  hidden page.

- A generic provider 403 no longer marks a ChatGPT account as permanently
  blocked. The account stays available, the dialog uses the upstream access
  error, and a previously stored false block is cleared when desktop storage
  opens or a quota refresh succeeds without a new denial. An explicit disabled
  workspace still stays blocked.

- Anthropic models now follow the provider's family order: Fable, Opus, Sonnet,
  then Haiku. Newest releases remain first within each family, and unknown
  families remain visible after the known lineup.

- Pool and Connections now share a compact header with clear primary actions
  and wrapping tabs. Pool presets live in the overflow menu; participant
  status, account filters and routing controls use one consistent panel,
  with current activity alongside the pool controls.

- Named Responses WebSocket requests now validate the documented `stream_id`
  characters and length, and HTTP/SSE fallback events and Relay-generated
  request errors include their named stream ID. Invalid IDs return
  `invalid_stream_id`. A bare native Responses SSE `[DONE]` without a
  `response.completed` is now an incomplete stream rather than a successful
  WebSocket turn with no terminal event. SSE terminal markers from another
  protocol no longer report a completed request for Responses, Chat
  Completions, Messages or Gemini; a claimed Responses completion with an
  explicitly failed or incomplete status is no longer recorded as success.
  Full parallel WebSocket multiplexing remains open work.

- Accounts with both a remaining provider quota window and credits now retain
  the reported window for pool admission, including the protected Codex
  account reserve. A temporarily unavailable route without an account error
  no longer appears as an invented account failure.

- Usage request and error lists now accept a page number for direct navigation;
  previous and next controls remain available.

- Route recovery now lives under **API → API** and applies to text requests in
  Responses, Chat Completions, Messages and Gemini, including clients other
  than ChatGPT. Existing saved switches are preserved; the wait is live and
  still shares the request's send and queue budgets. Empty Gemini prompt blocks
  converted to Chat Completions now keep a null assistant content field.

- Source pricing and pool member rules show models directly under their provider,
  without extra family headings. The model list keeps the catalog order and the
  dialogs use tighter, consistent rows. Source pricing also shows explicit
  five-minute and one-hour cache-write rates even when the model catalog was
  discovered through another endpoint; prices without a stated TTL remain
  unknown. An unknown cache-read price no longer appears as the input price.

- Quick Setup now starts with a shared local or server pool. The local
  connection step accepts both ChatGPT accounts and API sources, including
  multiple connections before choosing a client. A new API source joins the
  pool when saved; importing the current ChatGPT profile stays on the same
  step so another source can be added. Existing direct **Choose API** mode
  remains available from the application mode menu.

- Light and dark themes now use the warm neutral surfaces and deep green accents
  from the Zenith website, while keeping warning, error and information colors
  distinct.

- Model Rules now use consistent compact controls for reasoning, request speed
  and model visibility. Selected reasoning levels have check marks, and narrow
  windows keep speed labels visible without horizontal scrolling.

- Usage now has separate summary cards, compact filters and clearer request
  details. Narrow windows show labeled report cards, while summary settings
  use the same switches as the rest of Relay.

- Relay dialogs now share one compact visual style and close when the free
  space outside the window is clicked. Action menus, option lists, the mode
  picker, context menus and the mobile Help contents use the same compact
  surface and close on an outside click as well. Escape and the close button
  keep the same behavior. Long forms now use consistent field widths, action
  areas, selected states and scroll boundaries across connection, proxy,
  automation and export windows.

- Pool now separates participant counts, routing controls and current activity
  in an open layout. Connections and Pool share a compact summary with inline
  counts. Account controls have a full-width search field in narrow windows.
  Request details still identify the transport used.

- Excel / Basis Points tool failures now identify the invalid envelope field
  without revealing tool arguments. An ambiguous unqualified namespace tool or
  a non-object function argument is rejected instead of dispatching the wrong
  tool. Responses output is unwrapped before conversion to Chat Completions,
  Messages or Gemini, including their SSE event formats. Malformed completed
  output gets one bounded regeneration before Relay returns a terminal 502; it
  does not cool down the account or retry again after that regeneration.

- Account routes using Excel / Basis Points now use the v0.1.14 tool envelope:
  the client tool name is carried in `references`, while `code` contains the
  function arguments or custom-tool input directly. Historical client calls,
  including calls whose catalog is no longer in the request, are restored to
  that envelope. A declared tool excluded by `tool_choice` is rejected as a
  choice error. Namespaced tools retain their fully qualified name.
  Requests also carry stable turn metadata and the exact Excel client headers,
  including the corrected Office header names, so the upstream endpoint does
  not reject an otherwise valid request with a generic 422.
  The strict Basis Points body now matches the upstream adapter: client tool
  definitions stay in developer instructions, `tools`/`tool_choice` are not
  forwarded, and the original stream flag is preserved.
  A malformed client-tool relay now gets the same single bounded regeneration
  used by the official adapter before Relay records a terminal 502.

- Disconnecting a managed ChatGPT profile now restores only Relay-owned
  settings and login fields. External `config.toml` edits and newer manual
  OAuth sign-ins are kept instead of blocking the disconnect; reconnecting
  still cannot silently replace that new sign-in.

- A changed ChatGPT model catalog file no longer makes its profile backup look
  corrupt. Disconnect restores the previous settings and sign-in without
  deleting the externally edited catalog; newer sign-ins remain protected.

- Converted tool turns keep parallel Responses function calls together for
  Chat Completions providers. Messages and Chat Completions refusals reach
  clients with the proper terminal reason, including empty refusals in Codex;
  an incomplete Messages answer no longer appears as a successful completion.
  Request details distinguish the model requested by the client from the ID
  sent to the selected source when they differ.

- macOS builds now use ad-hoc signing without an Apple certificate. The DMG
  and updater archive are checked before publication, and release files include
  checksums. First launch still needs one-time approval in macOS settings.

- Codex catalogs now retain Ultra for exact models supported by the installed
  Codex or a matching native account card when the pool can route Max and the
  subagent effort. Generic API/adapter Max no longer creates a misleading
  Ultra option. This Codex orchestration mode is separate from request speed.

- API-source model and balance refreshes now share bounded work between manual
  and background requests. Changing an API address, key or catalog discards
  late results. Reopening a page uses the current session's cached statistics;
  an explicit refresh fetches again. If that fetch fails, the last successful
  amount is shown as stale with the failure reason, without disabling routing.
  Background observations appear in Pool and Choose API without extra provider
  reads. Protocol aliases now activate the cadence of their physical API source
  instead of being mistaken for accounts.

- Pool and API cards keep model, quota and balance refresh evidence out of the
  normal presentation. Values, actionable failures and stale-reading warnings
  remain visible where they help the user; detailed refresh state and check
  times stay in diagnostics and do not affect routing.

- Desktop and Relay Server share account quota/model refresh work between
  background jobs and manual requests. Quota and model lists update independently,
  repeated refreshes respect provider pauses, and closing a waiting request does
  not cancel shared work. Initial quota/model and reset-credit authorization
  preparation joins a
  separate on-demand job with reserved capacity; credentials never enter the
  observation cache. Late results and errors cannot overwrite a newer
  sign-in, proxy setting or replaced account; newer quota data from model
  requests takes precedence over an older read. Fresh quota headers from an
  inference request can postpone the next automatic quota poll when subscription
  metadata is current; manual refresh and reset verification remain scheduled.
  Relay Server also checks quota again shortly after a reported future reset.
  A delayed server 401 from an old bearer cannot invalidate a newer token or
  replaced login during quota or model recovery. Late OAuth and Agent-task
  writes cannot replace credentials installed by a subsequent import; an old
  runtime build cannot publish after that import or account deletion. Server
  re-import and deletion now close the old account route before changing its
  saved credential; a failed vault deletion restores the record without
  reopening pending work on the old runtime. Provider
  management requests also have a shared bounded HTTP queue with reserved
  login/recovery capacity; a changed account or source cannot send an outdated
  queued read. An older desktop refresh or pending token write cannot recreate
  credentials after an account is deleted and added again.

- The 1.1.3 update converts existing pool profiles to the current rotation automatically
  at startup, preserving saved member settings and gateway enabled state. No
  migration confirmation, notification, manual stop/start or rollback UI is
  required. Obsolete failure thresholds and ranking options are discarded;
  older saved profiles and presets still open. Old servers cannot receive
  unsupported rotation settings.

- Moving accounts to a user-managed server now closes pending local dispatch
  through import and verified cleanup. An interrupted move or a server-owned
  account cannot reappear in local routing after a restart; failed local
  activation keeps ownership in recovery rather than enabling two copies.

- If deleting a local account fails and its credentials or profile cannot be
  restored, the local gateway now stops instead of serving a stale route.

- Capacity and recovery waits now share runtime/per-key count and retained-byte
  limits, event-driven wakeups and one accumulated wait budget across retries
  and transport handoffs. Busy capacity is assigned fairly between keys;
  cancellation releases queue accounting without penalizing a provider. When
  every physical slot is busy, new requests no longer rescan the entire wait
  queue; unsupported routes still fail without joining it.

- Pool selection now uses normalized local load in automatic mode, with
  physical members sharing capacity across protocol aliases. Retries share one
  request budget through transports and repairs; an uncertain provider outcome
  is not silently replayed. Mandatory provider pauses are installed before a
  slot becomes available to another request. Server API sources no longer have
  a second, independently timed storm block outside pool rotation. Pending sends
  cannot use an old desktop account/source route while its membership, endpoint,
  proxy, login or permissions are being replaced; already started work may finish.

- Usage details put an approximate cache lifetime in parentheses beside the
  cache-read count, or beside the cache-write count when there is no read.
  A window reported by the provider is used as given. GPT-5.6 and later fall
  back to OpenAI's documented 30-minute minimum when the response omits one.
  Other missing windows stay marked unreported. The note is estimated from that
  request's latest cache write or read; it is not a live provider expiry.

- Catalog ordering now keeps model families from the same numbered generation
  together and uses stable family IDs for their order. A later release date
  alone no longer moves GPT-6 Sol above GPT-6 Astra.

- Background refresh can no longer replace saved settings with an older
  snapshot. Switching away from a connection and back also clears pending
  refresh work, so a slow earlier request cannot stall the current view.
  Visible remote pool pages now pick up server changes periodically and when
  returning to the window without requiring a local state-change event.

- API settings now offer a small standard/automatic tool optimization switch
  for local and compatible server pools. Standard mode forwards the complete
  catalog; automatic mode applies native Responses deferred tool search to
  every eligible request, with a compatibility retry when the selected
  endpoint does not support the standard fields. Relay no longer exposes
  thresholds or name-based allow/deny lists. The switch saves immediately; the
  full behavior is in Help.

- Pool catalog refresh now resolves each member's routes once for all its models,
  avoiding repeated discovery-policy work as model inventories grow. Offline
  models retain their editable metadata and cache-price fields. (`3e6e174`)
- Codex tool schemas preserve reference scope, validation constraints and literal
  data while limiting reference expansion, preventing excessive memory growth
  from deeply nested or branching definitions. (`3e6e174`)
- Converted Chat Completions responses retain reasoning text alongside tool
  calls in JSON, streams and follow-up history. Opaque reasoning state continues
  to require its native owner. (`e211560`)
- Retrying source setup after a failed snapshot or pool-membership update reuses
  the saved source instead of creating a duplicate. Removed unused controls and
  frontend helpers for the retired API-role ordering and manual route editor.
  (`54b0c75`)

- Long generations no longer end because Relay's HTTP, SSE or WebSocket timers
  expire. Relay waits for the provider, keeps streaming connections alive and
  preserves client cancellation and retries for real failures before output.
  (`3e6e174`)

- Reduced memory retained by reference catalogs between updates. Source data
  stays compact, cached derived data is skipped when loading, and saving a
  catalog no longer reads another full copy of the previous file into memory.
- Reduced startup and catalog refresh work by indexing model matches once and
  sharing immutable reference data. Refresh no longer merges the same catalog
  twice or duplicates its JSON tree in memory.
- Fixed a Windows crash when refreshing pool members. Bulk quota refresh keeps
  bounded concurrency and returns each account's result without overflowing
  the desktop command stack.
- Codex launch now applies pending model catalog updates before opening the
  client. Connecting a local pool checks its catalog before closing Codex, and
  starting the client no longer waits for an intermediate interface refresh.
  Profile preparation failures reliably reopen a previously running client.
  Account catalog requests run concurrently within a shared time limit, and
  history synchronization avoids rereading every file just to fingerprint it.
  A successful connection reuses its catalog when launching, and an OpenCode
  configuration error no longer prevents Codex catalog updates.
- Reconnecting Codex no longer fails because already-matching chat history
  exceeds the history-repair size limit. Only files requiring a provider change
  count toward the rewrite budget, so model and speed updates can complete
  without backing up unchanged conversations.

- Model details now come from a shared reference catalog for accounts and API
  providers, with consistent defaults when metadata is missing. Participant
  capability fields no longer hide reasoning or tools, and redundant metadata
  polling has been removed. Provider prices still take precedence when supplied.
- OpenAI models offer Standard, Fast and Ultrafast by Relay policy, including
  models whose account or provider returns no speed fields. Explicit speed
  choices survive rotation and override the pool default.
  Speed catalog entries include descriptions so Codex's Ultrafast option no
  longer has an empty explanation.

- API sources, stored proxies and quota automations share a more readable layout
  and compact editors. Proxy import can check new addresses immediately and show
  the actual exit IP, country and response time. Failed checks keep saved proxies
  and assignments; declared location is shown separately from observed results.
  Quota rules run automatically without a separate start button or mode selector.
  Existing manual rules are upgraded while disabled rules remain disabled.
  Default rule names now distinguish starting a quota countdown from resetting
  the weekly limit, including existing rules. Custom names remain unchanged.
  The editor starts with the automation type and its relevant fields; a custom
  rule name is optional. General automation labels no longer refer only to quotas.
  The Automations tab omits the shared search and refresh toolbar; rule changes
  update the list automatically.
  Frequent row actions use icons with tooltips. API source rows place launch
  before edit, with additional actions last. (`0b65f07`, `54b0c75`)

- Source creation keeps provider choices visible above a single column of key,
  address and name fields. Quick setup now groups progress, choices and navigation
  in a compact workspace without repeating the window's logo. Custom API setup
  exposes all required fields and can continue after they are filled; reselecting
  the active provider preserves edits. (`54b0c75`)

- The API tab brings status, pool counts, address and key into one connection
  panel with labelled copy actions. Port settings have their own row, and key
  reissue is available from the key's action menu.

- Adding connections to a pool now uses one searchable list, account/API sections,
  a selected-connections view and fixed add actions. Selection stays intact
  while filtering.

- Native requests no longer fail adapter compatibility checks because a source
  catalog omits a reasoning level or incorrectly marks a feature unsupported.
  Converted routes retain their feature and reasoning checks.

- Settings no longer repeat the enabled debug-mode notice and operations-folder
  button below the toggle; the folder action remains in Diagnostics.

- Streaming keep-alives wait for complete event boundaries, preventing a pause
  in a fragmented response from inserting a heartbeat into its JSON content.

- Invalid stream events now retain safe parser diagnostics in request details:
  JSON error category, position and frame sizes, without recording response
  content. These diagnostics identify Relay's parser separately from an
  upstream error message.

- Streams with mixed LF/CRLF/CR line endings no longer merge separate events and
  fail with `stream_invalid`. The shared parser preserves event order for
  native streams, protocol adapters and context compaction.

- Converted Codex requests accept client tracing, cache-affinity keys and the
  optional encrypted-reasoning output selector. Plain text and empty reasoning
  controls no longer exclude otherwise compatible routes. Unsupported options
  identify the offending field; encrypted input still requires a native route.

- Codex context compaction accepts completed output items delivered before the
  final stream event. Account routing retains only ChatGPT's infrastructure
  cookie in memory, isolated by account and authorization. WebSocket continuations
  reconnect after credential changes only with portable history, and delayed
  catalog refreshes cannot overwrite a different Codex profile binding.

- Relay accepts gzip and zstd JSON requests from current clients. Account
  compaction falls back to the Responses compaction trigger when the old
  compact endpoint is explicitly unavailable, preserving context and usage.
  Turn-state hints stay with the exact session, model and account credentials,
  including authentication refresh and concurrent responses.

- Model visibility, member model permissions, draining, launch preferences and automation enablement
  now use the same switch styling as application settings.

- Pool model rules can reset model and group order to the catalog default.
  Default order places OpenAI, Anthropic, Google and xAI first, then other
  companies alphabetically, and keeps each catalog family together,
  with newer versions first inside the family, consistently across providers.
  New families are grouped automatically from metadata. Failed reorder operations restore the displayed saved order,
  and another reorder cannot overlap a pending save.

- Catalog refresh retains prices and reference reasoning levels even when legacy
  route lists or endpoint declarations are incomplete. Pool client projections
  follow automatic routing, and converted requests retain their source prices
  across all four protocols. Server background refresh also updates protocol
  metadata without overwriting concurrent edits or restoring removed sources.
  Codex receives shared reference reasoning levels through eligible Responses
  routes, including converted models, with key scopes and adapter limits applied.

- Pool model rules now retain names, groups, prices, reasoning modes and saved
  order for all pooled account and API models, even while a member is disabled
  or unavailable. Shared model IDs appear once and count only pool members.

- Codex model labels use shared reference names, then compact labels from the
  model ID, consistently across pools and direct API
  connections. Shared reference names remain intact when participant metadata
  is malformed, and a valid owning-account card takes precedence over an
  incompatible one. Model IDs and pool order are preserved.

- Removed the technical force-refresh sign-in action from account cards and
  menus. Sign-in recovery now stays in the normal sign-in flow, while quota
  refresh remains the only visible refresh action.

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

- Accounts and API providers now share three pool rotation modes: Automatic,
  In order, and Round robin. Existing Smart profiles become Automatic on update.
  One reorderable list replaces separate API roles, with member weights and shared
  request limits. Settings apply without interrupting active requests; unavailable
  members do not block the rest. Concurrent edits are detected before saving, and
  presets retain the mixed order. The editor uses one scrollbar, short status
  labels and mode-specific controls; centered fields use Request share and
  Concurrent requests, with Unlimited shown for an unset concurrency cap.
  Detailed explanations are in Help. Automatic chooses the least occupied
  eligible members and uses request share only to break ties. It does not rank
  by manual order, balance, or quota percentage. In order preserves the manual
  queue. Modes support keyboard selection.
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
- API source routing is now fully automatic. Relay keeps adapter routing
  internal, prefers each model's declared native endpoint and converts requests
  from Responses, Chat Completions, Messages or Gemini when needed.
  Chat Completions supports function tools and their result history.
- New API sources determine formats automatically from provider declarations
  and endpoint settings. Every catalog model remains routable through the
  source fallback when declarations are absent. Legacy manual-mode fields are
  accepted during import and ignored; old physical endpoint assignments remain
  fallback hints so mixed-protocol sources keep working after an upgrade.
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
- Portable account imports preserve a safe account name and bounded,
  de-duplicated tags without exposing credentials; existing Relay tags remain
  authoritative on reimport.

### Fixed

- Gemini prompt blocks and empty filtered or token-limited candidates now
  reach clients as incomplete responses in JSON and streaming conversions,
  rather than appearing as malformed upstream output or successful completion.
- Account-card actions now use separate, consistent controls without the
  nested header frame; sign-in-required quota states have a clearer
  keyboard and pointer target.
- Pool rotation modes now save on first use and after members join or leave,
  without reverting to Smart because of an outdated stored member list.
  Concurrent edits still refresh and retry safely. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
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
- Model Rules keeps every model from pooled members, including members that are
  disabled or temporarily unavailable. Request availability is checked when
  Relay selects a member, so a missing route does not delete the model row.
- Saving a partial Model Rules reorder now preserves unavailable and
  binding-only pool models instead of treating them as removed.
- Pool and Connections cards use consistent account information and
  reauthentication controls. Unavailable accounts remain visible with their
  status instead of making the whole pool look unavailable.
- The OAuth completion page no longer shows a close button that cannot work.

<!-- relay-notes:ru -->

Zenith Relay 1.1.3 оставляет аккаунт ChatGPT доступным после общего отказа провайдера 403, восстанавливает историю инструментов Excel, показывает примерный срок кэша рядом с токенами использования и применяет те же компактные переключатели по всему приложению.

### Изменения

- Вход в ChatGPT открывается в окне Relay, а не в системном браузере. Прокси
  аккаунта используется и для этого окна, и для обмена токена. Действующая
  сессия завершает вход сама; иначе вход заканчивается в окне. Сохранённые
  заметки входа сами не подставляются. Замок внутри кнопки нового входа
  выбирает сохранённый HTTP-прокси для первого входа и не подставляет
  случайный.

- Инструкции инструментов Excel / Basis Points следуют адаптеру v0.2.8.
  Примеры используют только инструменты текущего запроса и совпадают с их
  типом и схемой, включая два слоя JSON для аргументов функции. Отдельное
  напоминание разработчика повторяет правило транспорта и оставляет ввод
  custom-инструмента как есть. Подсказка одноразового повтора добавляется
  после подготовленного ввода, до триггера сжатия. Структурный `text.format`
  отклоняется, а не отбрасывается. Обычные значения `service_tier` `auto`,
  `default` и `standard` остаются на этом маршруте; Fast по-прежнему нет.
  Загрузка изображений из внешнего плагина не копируется.

- Поток изображения, который закончился неполным, с ошибкой или без реального
  завершения, записывается как ошибка, а не как готовое изображение.

- Неудачное обновление баланса провайдера сохраняет последнюю сумму. Обзор
  выбранного API помечает её **Не обновлено**. Карточка пула оставляет сумму
  без этой пометки; причина остаётся в диагностике.

- Переключатель оптимизации инструментов теперь поясняет, что схемы открываются по запросу на обычном маршруте Responses, а Excel и остальные маршруты по-прежнему отправляют полный список.

- Переключатель для аккаунтов ChatGPT в пуле теперь называется **Использовать Basis Points**. Такие аккаунты отправляются через Excel, а не через Responses. Это может помочь деградировавшему аккаунту с генерацией, но результат не гарантирован.

- Закрытие главного окна освобождает его страницу, а пул продолжает работать в трее. Следующее открытие создаёт окно заново, а не возвращает скрытую страницу.

- Общий отказ провайдера 403 больше не блокирует аккаунт ChatGPT навсегда. Аккаунт остаётся доступным, диалог показывает ошибку доступа от провайдера, а ранее сохранённая ложная блокировка снимается при открытии локального хранилища или при успешном обновлении квоты без нового отказа. Явно отключённое рабочее пространство по-прежнему остаётся заблокированным.

- Модели Anthropic идут в порядке семейств провайдера: Fable, Opus, Sonnet, затем Haiku. Внутри семейства новые выпуски остаются первыми, неизвестные семейства видны после известного ряда.

- Пул и Подключения используют общий компактный заголовок с понятными основными действиями и переносимыми вкладками. Пресеты пула находятся в дополнительном меню. Статус участников, фильтры аккаунтов и управление маршрутизацией собраны в одной панели, а текущая активность показана рядом с управлением пулом.

- Именованные запросы Responses по WebSocket проверяют допустимые символы и длину `stream_id`. События запасного HTTP/SSE и ошибки запроса, созданные Relay, содержат этот идентификатор. Неверный идентификатор возвращает `invalid_stream_id`. Одиночный нативный `[DONE]` у Responses без `response.completed` теперь считается незавершённым потоком, а не успешным ходом WebSocket без завершающего события. Завершающие маркеры SSE другого протокола больше не отмечают запрос Responses, Chat Completions, Messages или Gemini как завершённый. Заявленное завершение Responses с явно неуспешным или неполным статусом больше не записывается как успех. Полное параллельное мультиплексирование WebSocket остаётся незавершённой работой.

- Если у аккаунта одновременно есть оставшееся окно квоты провайдера и кредиты, для допуска в пул сохраняется сообщённое окно, включая защищённый резерв аккаунта Codex. Временно недоступный маршрут без ошибки аккаунта больше не показывается как выдуманный сбой аккаунта.

- Списки запросов и ошибок в использовании принимают номер страницы для прямого перехода. Кнопки назад и вперёд остаются.

- Ожидание восстановления маршрута находится в **API → API** и применяется к текстовым запросам Responses, Chat Completions, Messages и Gemini, в том числе не из ChatGPT. Сохранённые переключатели не сбрасываются. Ожидание остаётся живым и по-прежнему использует бюджеты отправки и очереди этого запроса. Пустые блоки запроса Gemini, преобразованные в Chat Completions, сохраняют пустое поле содержимого ответа ассистента.

- Цены источника и правила участника пула показывают модели прямо под провайдером, без дополнительных заголовков семейств. Порядок списка остаётся каталожным, строки в окнах стали плотнее и единообразнее. Цены источника показывают явную запись кэша на 5 минут и на 1 час, даже если каталог модели получен через другой адрес. Цена без указанного срока остаётся неизвестной. Неизвестная цена чтения кэша больше не подменяется ценой входа.

- Быстрая настройка начинается с общего локального или серверного пула. На компьютере в одном шаге можно добавить и аккаунты ChatGPT, и API-источники, в том числе несколько подключений до выбора клиента. Новый API-источник сразу входит в пул. Импорт текущего профиля ChatGPT остаётся на том же шаге, чтобы можно было добавить ещё один источник. Прямой режим **Выбрать API** по-прежнему доступен в меню режима приложения.

- Светлая и тёмная темы используют тёплые нейтральные поверхности и глубокий зелёный акцент с сайта Zenith. Цвета предупреждения, ошибки и информации остаются различимыми.

- В правилах моделей компактные переключатели размышления, скорости запроса и видимости модели выглядят одинаково. Выбранные уровни размышления отмечены галочкой. В узком окне подписи скорости остаются видны без горизонтальной прокрутки.

- В использовании отдельные карточки сводки, компактные фильтры и более понятные сведения о запросе. В узком окне карточки отчёта подписаны, а переключатели сводки такие же, как в остальном Relay.

- Окна Relay используют один компактный вид и закрываются по щелчку на свободном месте вокруг окна. Меню действий, списки вариантов, выбор режима, контекстные меню и оглавление справки на узком экране используют ту же поверхность и тоже закрываются по щелчку снаружи. Escape и кнопка закрытия работают как раньше. Длинные формы подключений, прокси, автоматизаций и экспорта держат одинаковую ширину полей, область действий, состояние выбора и границы прокрутки.

- Пул отдельно показывает число участников, управление маршрутизацией и текущую активность. Подключения и Пул используют компактную сводку со счётчиками в строке. В узком окне поиск по аккаунтам занимает всю ширину. В сведениях о запросе по-прежнему указан использованный транспорт.

- Ошибки инструментов Excel / Basis Points называют неверное поле конверта и не раскрывают аргументы инструмента. Неоднозначный инструмент без полного имени пространства или аргумент функции, который не является объектом, отклоняется, а не вызывает не тот инструмент. Вывод Responses разворачивается перед преобразованием в Chat Completions, Messages или Gemini, включая их форматы событий SSE. Повреждённый завершённый вывод один раз ограниченно пересобирается, после чего Relay возвращает окончательную ошибку 502. Аккаунт при этом не уходит на паузу, и повторной попытки после этой пересборки нет.

- Маршруты аккаунтов через Excel / Basis Points используют конверт инструментов v0.1.14: имя инструмента клиента передаётся в `references`, а `code` содержит аргументы функции или ввод пользовательского инструмента напрямую. Прежние вызовы клиента, даже если их описания уже нет в запросе, восстанавливаются в этот конверт. Инструмент, исключённый `tool_choice`, отклоняется как ошибка выбора. Инструменты с пространством имён сохраняют полное имя. Запросы также несут устойчивые сведения о ходе и точные заголовки клиента Excel, включая исправленные имена заголовков Office, чтобы внешний адрес не отвечал общим отказом 422 на иначе правильный запрос. Строгое тело Basis Points совпадает с внешним адаптером: описания инструментов клиента остаются в инструкциях разработчика, `tools` и `tool_choice` не пересылаются, исходный признак потока сохраняется. Повреждённая передача инструмента клиента получает ту же одну ограниченную пересборку, что и официальный адаптер, прежде чем Relay запишет окончательную ошибку 502.

- Отключение управляемого профиля ChatGPT восстанавливает только настройки и поля входа, которыми владеет Relay. Внешние правки `config.toml` и более новый ручной вход OAuth сохраняются и не блокируют отключение. Повторное подключение по-прежнему не может тихо заменить этот новый вход.

- Изменённый файл каталога моделей ChatGPT больше не делает резервную копию профиля повреждённой. Отключение возвращает прежние настройки и вход, не удаляя каталог, изменённый снаружи. Более новый вход остаётся защищён.

- Преобразованные ходы с инструментами сохраняют параллельные вызовы функций Responses вместе для провайдеров Chat Completions. Отказы Messages и Chat Completions доходят до клиента с правильной причиной завершения, включая пустой отказ в Codex. Неполный ответ Messages больше не выглядит успешным завершением. Сведения о запросе отличают модель, которую запросил клиент, от идентификатора, отправленного выбранному источнику, если они различаются.

- Сборки macOS подписываются ad-hoc, без сертификата Apple. DMG и архив обновления проверяются до публикации, у файлов релиза есть контрольные суммы. Первый запуск по-прежнему требует одно разрешение в настройках macOS.

- Каталоги Codex сохраняют Ultra для точных моделей, которые поддерживает установленный Codex или подходящая нативная карточка аккаунта, если пул может провести Max и усилие субагента. Обычный Max у API или адаптера больше не создаёт вводящий в заблуждение вариант Ultra. Этот режим оркестрации Codex отделён от скорости запроса.

- Обновление моделей и баланса API-источника делит ограниченную работу между ручным и фоновым запросом. Смена адреса, ключа или каталога отбрасывает запоздалый результат. Повторное открытие страницы использует статистику текущего сеанса. Явное обновление запрашивает данные снова. Если запрос не удался, последняя успешная сумма показывается как устаревшая вместе с причиной, без отключения маршрутизации. Фоновые наблюдения появляются в Пуле и в режиме «Выбрать API» без лишнего обращения к провайдеру. Псевдонимы протокола включают периодичность своего физического API-источника и больше не принимаются за аккаунты.

- Карточки пула и API не показывают в обычном виде подробности обновления моделей, квоты и баланса. Значения, ошибки, с которыми можно что-то сделать, и предупреждение об устаревшем чтении остаются там, где они помогают. Подробное состояние и время проверки остаются в диагностике и не влияют на маршрутизацию.

- Приложение и сервер Relay делят обновление квоты и моделей аккаунта между фоновыми задачами и ручными запросами. Списки квоты и моделей обновляются независимо. Повторные обновления соблюдают паузу провайдера, а закрытие ожидающего запроса не отменяет общую работу. Подготовка авторизации для первичной квоты, моделей и кредитов сброса идёт отдельной задачей по требованию с зарезервированной ёмкостью. Учётные данные не попадают в кэш наблюдений. Запоздалый результат и ошибка не затирают более новый вход, настройку прокси или заменённый аккаунт. Более новая квота из запроса моделей важнее более старого чтения. Свежие заголовки квоты из запроса генерации могут отложить следующую автоматическую проверку, если сведения о подписке актуальны. Ручное обновление и проверка сброса остаются в расписании. Сервер Relay также проверяет квоту вскоре после сообщённого будущего сброса. Запоздалый ответ 401 со старым ключом не может сделать недействительными более новый токен или заменённый вход во время восстановления квоты или моделей. Запоздалая запись OAuth и задач агента не заменяет учётные данные, установленные последующим импортом. Старая сборка среды выполнения не может опубликовать состояние после этого импорта или удаления аккаунта. Повторный импорт и удаление на сервере закрывают старый маршрут аккаунта до смены сохранённых учётных данных. Неудачное удаление из хранилища восстанавливает запись и не открывает заново ожидающую работу старой среды. Управляющие запросы к провайдеру идут через общую ограниченную очередь HTTP с резервом для входа и восстановления. Изменённый аккаунт или источник не может отправить устаревшее чтение из очереди. Более старое обновление приложения или ожидающая запись токена не может заново создать учётные данные после удаления аккаунта и его повторного добавления.

- Обновление 1.1.3 при запуске само переводит существующие профили пула на текущую ротацию, сохраняя настройки участников и включённое состояние шлюза. Отдельное подтверждение, уведомление, ручная остановка, запуск или откат не нужны. Устаревшие пороги ошибок и параметры ранжирования отбрасываются. Старые профили и пресеты по-прежнему открываются. Старый сервер не принимает неподдерживаемые настройки ротации.

- Перенос аккаунтов на свой сервер закрывает ожидающую локальную отправку через импорт и проверенную очистку. Прерванный перенос или аккаунт, уже принадлежащий серверу, не может снова появиться в локальной маршрутизации после перезапуска. Неудачная локальная активация оставляет владение в восстановлении и не включает две копии.

- Если удаление локального аккаунта не удалось и его учётные данные или профиль нельзя восстановить, локальный шлюз останавливается и не обслуживает устаревший маршрут.

- Ожидание ёмкости и восстановления использует общие пределы числа и сохранённых байтов для среды и для каждого ключа, пробуждение по событиям и один накопленный бюджет ожидания на повторы и смену транспорта. Занятая ёмкость распределяется между ключами справедливо. Отмена освобождает учёт очереди и не штрафует провайдера. Когда заняты все физические слоты, новый запрос больше не просматривает всю очередь ожидания заново. Неподдерживаемый маршрут по-прежнему завершается ошибкой, не входя в эту очередь.

- Выбор в автоматическом режиме использует нормализованную локальную загрузку. Физический участник делит ёмкость между псевдонимами протоколов. Повторы используют один бюджет запроса через транспорты и исправления. Неопределённый результат провайдера не повторяется молча. Обязательная пауза провайдера устанавливается до того, как слот станет доступен другому запросу. У API-источников сервера больше нет второй, отдельно отсчитываемой блокировки шторма вне ротации пула. Ожидающая отправка не может использовать старый маршрут аккаунта или источника приложения, пока меняются его участие, адрес, прокси, вход или разрешения. Уже начатая работа может завершиться.

- В сведениях об использовании примерный срок кэша стоит в скобках рядом с чтением кэша, а если чтения нет — рядом с записью. Окно, которое сообщил провайдер, используется как есть. Для GPT-5.6 и новее при отсутствии окна берётся документированный минимум OpenAI в 30 минут. Остальные пропуски помечаются как несообщённый срок. Оценка считается от последней записи или чтения кэша в этом запросе и не является живым сроком у провайдера.

- Порядок каталога держит семейства одной номерной линейки вместе и использует устойчивые идентификаторы семейств. Более поздняя дата выпуска сама по себе больше не поднимает GPT-6 Sol выше GPT-6 Astra.

- Фоновое обновление больше не может заменить сохранённые настройки более старым снимком. Уход с подключения и возврат к нему также сбрасывают ожидающее обновление, поэтому медленный предыдущий запрос не блокирует текущий экран. Открытые страницы удалённого пула подхватывают изменения сервера периодически и при возврате к окну, без локального события об изменении состояния.

- В настройках API есть небольшой переключатель стандартной или автоматической оптимизации инструментов для локального пула и совместимого сервера. Стандартный режим передаёт полный каталог. Автоматический режим применяет отложенный поиск инструментов нативного Responses к каждому подходящему запросу и один раз повторяет запрос без этих полей, если выбранный адрес их не поддерживает. Пороги и списки разрешения или запрета по именам больше не показываются. Переключатель сохраняется сразу. Полное поведение описано в справке.

- Обновление каталога пула один раз определяет маршруты участника для всех его моделей и не повторяет политику обнаружения по мере роста списка. Отключённые от сети модели сохраняют редактируемые сведения и поля цены кэша. (`3e6e174`)

- Схемы инструментов Codex сохраняют область ссылок, ограничения проверки и буквальные данные, ограничивая раскрытие ссылок, чтобы глубокие или ветвящиеся описания не раздували память. (`3e6e174`)

- Преобразованные ответы Chat Completions сохраняют текст размышления рядом с вызовами инструментов в JSON, потоке и последующей истории. Непрозрачное состояние размышления по-прежнему требует своего нативного владельца. (`e211560`)

- Повтор настройки источника после неудачного снимка или обновления участия в пуле использует уже сохранённый источник и не создаёт дубликат. Убраны неиспользуемые элементы и вспомогательный код интерфейса для прежнего порядка ролей API и ручного редактора маршрутов. (`54b0c75`)

- Долгая генерация больше не обрывается из-за таймеров HTTP, SSE или WebSocket самого Relay. Relay ждёт провайдера, поддерживает поток и сохраняет отмену клиентом и повторы для настоящих сбоев до появления ответа. (`3e6e174`)

- Каталоги-справочники удерживают меньше памяти между обновлениями. Данные источника остаются компактными, производные кэшированные данные не читаются при загрузке, а сохранение каталога больше не загружает в память ещё одну полную копию предыдущего файла.

- Запуск и обновление каталога делают меньше работы: совпадения моделей индексируются один раз, неизменяемые справочные данные используются совместно. Обновление больше не сливает один и тот же каталог дважды и не дублирует его дерево JSON в памяти.

- Исправлен аварийный выход Windows при обновлении участников пула. Массовое обновление квоты сохраняет ограниченную параллельность и возвращает результат каждого аккаунта, не переполняя стек команды приложения.

- Запуск Codex применяет ожидающие обновления каталога моделей до открытия клиента. Подключение локального пула проверяет каталог до закрытия Codex, а запуск клиента больше не ждёт промежуточного обновления интерфейса. Ошибка подготовки профиля надёжно снова открывает ранее работавший клиент. Запросы каталогов аккаунтов идут параллельно в общем пределе времени, а сверка истории не перечитывает каждый файл только ради отпечатка. Успешное подключение повторно использует свой каталог при запуске. Ошибка настройки OpenCode больше не мешает обновлению каталога Codex.

- Повторное подключение Codex больше не срывается из-за того, что уже совпадающая история чатов превышает предел исправления истории. В бюджет перезаписи входят только файлы, которым нужна смена провайдера, поэтому обновление модели и скорости завершается без резервного копирования неизменённых разговоров.

- Сведения о моделях берутся из общего справочного каталога для аккаунтов и API-провайдеров. Если метаданных нет, применяются одинаковые значения по умолчанию. Поля возможностей участника больше не скрывают размышление или инструменты, лишний опрос метаданных убран. Цена провайдера по-прежнему важнее, если она передана.

- Модели OpenAI предлагают Обычную, Быструю и Сверхбыструю скорость по правилу Relay, даже если аккаунт или провайдер не вернул поля скорости. Явный выбор скорости сохраняется при ротации и перекрывает значение пула по умолчанию. В каталоге скоростей есть описания, поэтому у Сверхбыстрой в Codex больше нет пустого пояснения.

- API-источники, сохранённые прокси и автоматизации квоты используют более читаемую раскладку и компактные редакторы. Импорт прокси может сразу проверить новый адрес и показать фактический выходной IP, страну и время ответа. Неудачная проверка сохраняет прокси и назначения. Заявленное место показано отдельно от наблюдаемого. Правила квоты выполняются сами, без отдельной кнопки запуска и выбора режима. Прежние ручные правила обновляются, отключённые остаются отключёнными. Имена правил по умолчанию отличают запуск отсчёта квоты от сброса недельного лимита, в том числе у уже существующих правил. Свои имена не меняются. Редактор начинается с типа автоматизации и нужных ей полей. Своё имя правила необязательно. Общие подписи автоматизаций больше не говорят только о квоте. На вкладке автоматизаций нет общей строки поиска и обновления: список обновляется при изменении правила. Частые действия в строке — значки с подсказками. В строке API-источника запуск стоит перед правкой, дополнительные действия — последними. (`0b65f07`, `54b0c75`)

- При создании источника выбор провайдера остаётся над одним столбцом полей ключа, адреса и имени. Быстрая настройка собирает ход, выбор и переход в компактной области и не повторяет логотип окна. Настройка своего API показывает все обязательные поля и позволяет продолжить после их заполнения. Повторный выбор уже активного провайдера сохраняет правки. (`54b0c75`)

- Вкладка API собирает состояние, счётчики пула, адрес и ключ в одной панели подключения с подписанным копированием. Порт настроен в своей строке, а перевыпуск ключа доступен из меню действий ключа.

- Добавление подключений в пул использует один список с поиском, разделы аккаунтов и API, вид выбранных подключений и закреплённые действия добавления. Выбор не сбрасывается при фильтрации.

- Нативный запрос больше не проваливает проверку совместимости адаптера только потому, что каталог источника пропустил уровень размышления или ошибочно пометил возможность как неподдерживаемую. Преобразованные маршруты сохраняют свои проверки возможностей и размышления.

- Настройки больше не повторяют под переключателем уведомление о включённом подробном режиме и кнопку папки операций. Открыть папку по-прежнему можно в диагностике.

- Сигналы поддержания потока ждут границы целого события, чтобы пауза в разорванном ответе не вставила служебный кадр внутрь его JSON.

- Неверные события потока сохраняют безопасную диагностику разбора в сведениях о запросе: категорию ошибки JSON, позицию и размеры кадра, без записи содержимого ответа. Эта диагностика отличает разбор Relay от сообщения об ошибке провайдера.

- Поток со смешанными окончаниями строк LF, CRLF и CR больше не склеивает отдельные события и не завершается ошибкой `stream_invalid`. Общий разбор сохраняет порядок событий для нативных потоков, адаптеров протоколов и сжатия контекста.

- Преобразованные запросы Codex принимают трассировку клиента, ключи привязки кэша и необязательный выбор зашифрованного вывода размышления. Обычный текст и пустое управление размышлением больше не исключают иначе совместимый маршрут. Неподдерживаемый параметр называет поле. Зашифрованный ввод по-прежнему требует нативный маршрут.

- Сжатие контекста Codex принимает завершённые элементы вывода, пришедшие до последнего события потока. Маршрутизация аккаунта держит в памяти только инфраструктурный cookie ChatGPT, отдельно для аккаунта и авторизации. Продолжение WebSocket после смены учётных данных переподключается только при переносимой истории. Запоздалое обновление каталога не может перезаписать привязку другого профиля Codex.

- Relay принимает JSON-запросы gzip и zstd от текущих клиентов. Сжатие аккаунта переходит на механизм сжатия Responses, только если старый адрес сжатия явно недоступен, и сохраняет контекст и использование. Подсказки состояния хода остаются у точных сеанса, модели и учётных данных аккаунта, включая обновление авторизации и одновременные ответы.

- Видимость модели, разрешения моделей участника, вывод из ротации, предпочтение запуска и включение автоматизации используют тот же вид переключателя, что и настройки приложения.

- Правила моделей пула могут сбросить порядок моделей и групп к каталогу по умолчанию. Сначала идут OpenAI, Anthropic, Google и xAI, затем остальные компании по алфавиту. Внутри компании семейства каталога держатся вместе, более новые версии — первыми внутри семейства, одинаково у всех провайдеров. Новые семейства группируются сами по метаданным. Неудачная перестановка возвращает показанный сохранённый порядок, а вторая перестановка не накладывается на ещё не сохранённую.

- Обновление каталога сохраняет цены и справочные уровни размышления, даже если старые списки маршрутов или объявления адресов неполны. Проекция клиента пула следует автоматической маршрутизации, а преобразованные запросы сохраняют цены источника во всех четырёх протоколах. Фоновое обновление сервера также обновляет метаданные протокола, не затирая одновременные правки и не возвращая удалённые источники. Codex получает общие справочные уровни размышления через подходящие маршруты Responses, включая преобразованные модели, с учётом областей ключа и ограничений адаптера.

- Правила моделей пула сохраняют имена, группы, цены, режимы размышления и сохранённый порядок для всех моделей аккаунтов и API в пуле, даже если участник отключён или недоступен. Одинаковые идентификаторы моделей показаны одной строкой и считают только участников пула.

- Подписи моделей Codex используют общее справочное имя, а затем компактную подпись из идентификатора модели, одинаково в пуле и при прямом подключении API. Справочное имя не портится из-за повреждённых метаданных участника. Верная карточка аккаунта-владельца важнее несовместимой. Идентификаторы моделей и порядок пула сохраняются.

- С карточек и меню аккаунтов убрано техническое действие принудительного обновления входа. Восстановление входа остаётся в обычном входе, а обновление квоты — единственное видимое действие обновления.

- Восстановление пула автоматическое: рассматриваются все совместимые участники, временный сбой даёт паузу не меньше пяти секунд, повторные сбои увеличивают паузу. Недоступная модель, отклонённые учётные данные и непрозрачный отказ шлюза больше не мешают перейти к другому подходящему API или аккаунту. Ручные настройки числа повторов и паузы последнего кандидата убраны.

- Английская и русская справка переписаны вокруг текущего порядка подключений, режимов, ротации пула, квот, балансов и восстановления. Добавлено связанное оглавление, исправлены устаревшие пути настроек и описания поведения. У справки есть колонка чтения, закреплённое оглавление с текущим разделом, нумерованные шаги настройки и строки ошибок, которые помещаются в узкое окно. На маленьком экране оглавление становится компактным меню. Справочник ошибок группирует точные коды с причинами и конкретными шагами восстановления, категории раскрываются, доступен поиск по коду или симптому.

- Сброс недельной квоты использует компактную строку действия и отдельный счётчик кредитов на карточках аккаунта и пула.

- Правила участника пула открываются компактным списком моделей с одним переключателем на модель. Раскрываемые группы провайдеров сохраняют отключённые модели, длинные списки можно искать. Цены API выровнены столбцами рядом с моделью, включая запись кэша на 5 минут и 1 час. В узком окне цены подписаны по строкам. Вторичные настройки выровнены на своей вкладке. Порядок моделей аккаунта и API следует метаданным и сохранённому ручному порядку, не перемещая отключённые модели и не ставя выше переопределение цены.

- У аккаунтов и API-провайдеров три режима ротации пула: Автоматически, По порядку и По кругу. Сохранённый Smart при обновлении становится Автоматически. Один переставляемый список заменяет отдельные роли API, с долями участников и общими пределами запросов. Настройки применяются, не прерывая уже идущие запросы. Недоступные участники не блокируют остальных. Одновременные правки обнаруживаются до сохранения, пресеты сохраняют смешанный порядок. В редакторе одна полоса прокрутки, короткие подписи состояния и элементы, нужные выбранному режиму. По центру стоят доля запросов и одновременные запросы, для снятого предела показано «Без ограничений». Подробности — в справке. Автоматический режим выбирает наименее занятых подходящих участников и использует долю запросов только при равной загрузке. Он не ранжирует по ручному порядку, балансу или проценту квоты. По порядку сохраняет ручную очередь. Режимы выбираются с клавиатуры. Недавний сбой теряет штраф планирования в течение минуты, поэтому восстановившийся участник может вернуться сам. Завершение более старого запроса не освобождает чужую пробу восстановления. Занятые пробы допускают ограниченное ожидание. Изменение состава пула на сервере сразу применяет сохранённый порядок и пределы запросов. Подсказка следующего кандидата следует планировщику и не показывается, если выбор зависит от модели или формата. Устаревшие события активности не затирают состояние более новой среды выполнения.

- Управление пулом использует компактную панель, отдельную строку текущего и следующего маршрута и рамку с затенённой полосой состояния и разделителями между счётчиками. Подключения используют ту же панель для поиска, фильтров и действий аккаунта. Обе сводки сохраняют общую сумму кредитов провайдера, если она есть. Полные имена маршрутов и моделей переносятся на узком экране. У действий-значков остаются подсказки.

- Скорость запросов пула — один перетаскиваемый переключатель на три положения: Обычная, Быстрая и Сверхбыстрая, вместо меню и отдельного выключателя. Компактная подпись показывает только выбранный режим, переход плавный и учитывает настройку уменьшения движения. Выбор доступен с клавиатуры и касанием. Перетаскивание сохраняется при отпускании.

### Добавлено

- Пул одновременно обслуживает Responses, Chat Completions, Messages и Gemini, с нативными путями и поддерживаемым преобразованием для JSON и потоковых запросов.

- Маршрутизация API-источника полностью автоматическая. Relay не показывает выбор адаптера, предпочитает объявленный нативный адрес каждой модели и при необходимости преобразует запрос из Responses, Chat Completions, Messages или Gemini. Chat Completions поддерживает функции-инструменты и историю их результатов.

- Новый API-источник определяет форматы сам по объявлениям провайдера и настройкам адреса. Каждая модель каталога остаётся маршрутизируемой через запасной путь источника, если объявлений нет. Старые поля ручного режима принимаются при импорте и игнорируются. Прежние назначения физических адресов остаются подсказкой запасного пути, поэтому источник со смешанными протоколами продолжает работать после обновления.

- OpenCode использует группы SDK по протоколам и сохраняет рабочие идентификаторы моделей и пользовательские параметры при обновлении каталога. Codex использует поток HTTP, когда модели нужно преобразование, а нативные соединения WebSocket по-прежнему поддерживаются.

- Карточки API распознают балансы Sub2API, New API, совместимого биллинга One API, DeepSeek и SiliconFlow. Статистика OpenRouter работает с обычным ключом генерации. Карточки различают кошелёк, остаток ключа, подписку, валюту и собственную оценку использования Relay, не показывают внутренние подписи адаптера и отсутствующее число запросов и помечают неудачное обновление, не отбрасывая последний известный баланс. Ход обновления виден на анимации кнопки, без второй строки состояния под счётчиками.

- Сведения о неудачном запросе показывают исходные код, тип, сообщение и статус HTTP провайдера отдельно от категории Relay. Сообщение можно скопировать. Секретное содержимое скрыто, длинное сообщение ограничено. У старых записей явно показано, что сообщение провайдера не было сохранено.

- В настройках есть **Диагностика**. Relay ведёт отдельные ограниченные по размеру и очищенные журналы ошибок, аварийных завершений и важных этапов операций. Каждую папку можно открыть из приложения. Подробный журнал операций выключен по умолчанию и включается там же, когда нужно разобраться в сбое.

- Импорт переносимых аккаунтов сохраняет безопасное имя и ограниченные
  неповторяющиеся метки, не раскрывая учётные данные. При повторном импорте
  метки Relay остаются главными.

### Исправления

- Блоки запроса Gemini и пустые кандидаты, отфильтрованные или ограниченные по токенам, доходят до клиента как неполный ответ в JSON и в потоковом преобразовании, а не как повреждённый вывод провайдера или успешное завершение.

- Действия на карточке аккаунта разделены и выглядят одинаково, без вложенной рамки заголовка. Состояние квоты, требующее входа, имеет более понятную цель для клавиатуры и указателя.

- Режимы ротации пула сохраняются при первом использовании и когда участники входят или выходят, и не возвращаются к Smart из-за устаревшего сохранённого списка. Одновременные правки по-прежнему обновляются и безопасно повторяют сохранение. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Выбор аккаунта ChatGPT и резерв квоты находятся в одной компактной панели. Переключатели возможностей — плоские строки с коротким описанием. Настройки участника пула больше не добавляют вторую рамку внутри окна. Подзаголовок вкладки OpenCode говорит «Использовать пул в OpenCode». ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Имена моделей GPT в ChatGPT и Codex сохраняют исходные идентификаторы, даже если у выбранного аккаунта нет подходящей карточки каталога. Явные префиксы ключей и существующие псевдонимы поддерживаются. Возможности берутся у совпавшей модели. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Восстановление истории инструментов Responses сохраняет связь вызова и результата, если клиент ссылается на идентификатор элемента или не передаёт идентификатор вызова. Результаты сопоставляются по виду и пространству имён, без удаления истории и без угадывания среди параллельных вызовов. HTTP, поток и WebSocket используют одно и то же ограниченное восстановление. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Ротация пула сразу сохраняет режим, порядок и настройки участников. Перетаскивание работает одинаково, одновременное изменение состава обновляет редактор, а конфликт сохранения повторяется и не возвращает удалённых участников. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Действия правил моделей стоят в одной выровненной группе, элементы формата, размышления, скорости и включения одного размера. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Поля цены провайдера имеют одну скруглённую обводку, отдельный знак валюты и числа, выровненные вправо. Фокус и неверное значение подсвечивают всё поле одинаково в обеих темах. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Подписи скорости пула и сводки подключений помещаются при более широком системном шрифте и не обрезаются и не переносятся без нужды на обычном экране. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Несколько маршрутов одного участника делят вес ротации и ёмкость запросов. Неподдерживаемый адрес не отключает остальные форматы участника. Сбои квоты и авторизации остаются общими. Повторы продолжения сохраняют полную историю, результаты инструментов и непрозрачное владение.

- Преобразованный запрос отклоняет неподдерживаемые параметры, а не отбрасывает их молча. Схемы сохраняют ограничения, инструкции одного хода не переносятся в следующие, уровни размышления предлагаются только там, где их можно сопоставить. Потоковые инструменты сохраняют идентификаторы и порядок. Использование отражает фактический формат провайдера и не выдумывает отсутствующие счётчики.

- Привязка кэша больше не выбирает участника вне лучшей доступной группы и не обходит учёт ротации.

- Задержка повтора, которую сообщил провайдер, соблюдается для перегруженных и недоступных моделей и других ошибок службы, в том числе если настроена более короткая задержка восстановления.

- Обработка TLS обновлена до rustls 0.23.45, чтобы закрыть RUSTSEC-2026-0285.

- Пул больше не сообщает, что недоступны все участники, если телеметрия маршрутизации отсутствует или устарела, но готовые участники есть. Если готовых нет, предупреждение отдельно показывает ожидание квоты, недоступных и отключённых участников, с понятной причиной ошибки. Пул и Подключения группируют аккаунты по доступности и сохраняют порядок маршрутизации внутри группы.

- Ошибка провайдера внутри ответа HTTP 200, в JSON или в потоке, остаётся ошибкой в истории использования. Ограничение частоты и лимит расходов различаются. Явная ошибка проверки запроса не отправляет аккаунт на паузу.

- Общий отказ запроса провайдером или отключённая модель больше не блокируют переход к другому совместимому участнику. Ошибка службы на маршруте аккаунта сохраняет принадлежность аккаунту и не помечает аккаунт нездоровым.

- Продолжение инструментов проверяет всю историю, включая вызовы после первых шестнадцати. Завершённые старые вызовы больше не выбирают владельца нового результата инструмента, а выводы разных владельцев нельзя смешать неявно. Примеры в схемах инструментов и вложенные данные результата больше не влияют на маршрутизацию.

- Обновление моделей аккаунта больше не стирает независимую блокировку, требование входа или ошибку проверки и не понижает окончательную ошибку каталога после временного сбоя сети. Ошибка наблюдения за квотой больше не скрывает причину недоступности аккаунта в Подключениях и Пуле.

- Активность пула сопоставляет последний аккаунт или API-источник по идентичности, поэтому запросы с одинаковым временем не подсвечивают не того участника.

- Восстановление сохраняет исходную привязку ответа для других веток чата и повторов и использует параметры текущего запроса, не возвращая старые инструкции или настройки транспорта. Ссылки провайдера на разговор и элементы или результаты инструментов без их вызовов больше не считаются полным локальным повтором.

- Чат с переданным сжатым контекстом может продолжиться без устаревшей ссылки на ответ по HTTP или WebSocket. Сжатое окно и сохранённые результаты инструментов остаются. Нечитаемое сжатие больше не отбрасывается молча.

- Восстановление чата требует сохранённую цепочку разговора, прежде чем убрать прежнюю ссылку на ответ, если не передано полное сжатое окно. Так прежний контекст не теряется молча.

- Ручное обновление учётных данных и наблюдаемый вход клиента возвращают аккаунту возможность маршрутизации. Карточки удалённых аккаунтов показывают паузу среды выполнения и недоступные маршруты.

- Восстановленные аккаунты пула остаются доступны, не удаляя другого участника. Запоздалое обновление авторизации больше не отменяет более новое отключение аккаунта. Карточки аккаунтов показывают временную недоступность среды во время паузы.

- Импорт аккаунта или API-источника без добавления в пул больше не перезапускает работающий локальный шлюз. Посторонний перезапуск приёмника не прерывает Relay при импорте только в список.

- После прерванного запуска или импорта диагностика при следующем старте записывает последний очищенный этап операции, даже если подробный журнал был выключен.

- Чат, у исходного аккаунта которого кончилась квота, может продолжиться через следующий совместимый здоровый аккаунт или API-источник. Это работает и для обычных запросов Responses, и для WebSocket.

- Чат может восстановиться после истёкшей ссылки на ответ по локально сохранённой истории Relay, в том числе на уже открытом WebSocket. Полная история инструментов может перейти вместе с пулом. Неполное состояние инструмента больше не удаляется молча при восстановлении.

- Неизвестная ссылка на ответ отклоняется до обращения к несвязанному аккаунту. Клиент может отправить полную историю без старой ссылки. Иначе Relay сообщает, что контекст продолжения недоступен.

- Аккаунт, которому нужен вход, который на паузе или у которого ошибка, отключает только этого кандидата, в том числе если статус изменился при уже работающем локальном шлюзе. Остальной пул и его доступные модели продолжают ротацию.

- Импорт аккаунта и очистка OAuth больше не оставляют устаревшее временное состояние, которое может закрыть Relay или заблокировать следующий импорт. JSON-аккаунт можно добавить в Relay, не добавляя его в пул.

- Обновлённые кредиты аккаунта и пределы квоты применяются к работающему пулу, в том числе когда Relay открыт только в трее. Как доступный баланс показываются только настоящие кредиты провайдера.

- Фоновые проверки пробуждения ChatGPT используют живую среду шлюза, включая обновление токена, паузу, использование и диагностику, и остаются закреплены за аккаунтом, который выбрал планировщик.

- Модель можно вернуть на Обычную скорость, пока её маршрут временно недоступен. Быстрая и Сверхбыстрая заново считаются для каждого кандидата повтора, поэтому неподдерживаемая скорость не переносится на следующий аккаунт или API-источник.

- Правила моделей сохраняют каждую модель участников пула, включая отключённых и временно недоступных. Доступность запроса проверяется при выборе участника, поэтому отсутствие маршрута не удаляет строку модели.

- Сохранение неполной перестановки правил моделей сохраняет недоступные модели и модели, которые есть только как привязка, и не считает их удалёнными.

- Карточки Пула и Подключений одинаково показывают сведения об аккаунте и управление повторным входом. Недоступные аккаунты остаются видны со своим статусом, и из-за них весь пул не выглядит недоступным.

- На странице завершения OAuth больше нет кнопки закрытия, которая не может сработать.
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

[Unreleased]: https://github.com/F0RLE/zenith-relay/compare/v1.1.5...HEAD
[1.1.5]: https://github.com/F0RLE/zenith-relay/compare/v1.1.4...v1.1.5
[1.1.4]: https://github.com/F0RLE/zenith-relay/compare/v1.1.3...v1.1.4
[1.1.3]: https://github.com/F0RLE/zenith-relay/compare/v1.1.2...v1.1.3
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
