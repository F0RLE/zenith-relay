# Ротация пула: выбор, попытки и восстановление

**Статус: целевой проект, реализован частично; полный контракт не принят.**

Общее ядро admission/lease, бюджеты попыток и bounded admission подключены к
основным drivers. Переход формата настроек выполняется автоматически при
обычном обновлении до 1.1.3, без preview, подтверждения и уведомления о миграции.
Host refresh и полная приёмка ещё не завершены; открытые условия перечислены в [ROADMAP.md](ROADMAP.md).
Описание реализованного поведения находится в [PLANNING.md](PLANNING.md).
Наличие кода и локальных тестов не означает полной готовности этого проекта.

Эта редакция заменяет предыдущий проект целиком. Автоматическое преобразование
сохранённого формата при обновлении не доказывает завершения остальных условий
приёмки движка.
Подключённый код тоже подлежит пересмотру: наличие модуля и зелёных unit-тестов
не доказывает правильность алгоритма или его подключение к реальным запросам.

Текущее реализованное поведение определяют исходники и focused tests;
[PLANNING.md](PLANNING.md) описывает текущие контракты,
[ROADMAP.md](ROADMAP.md) — принятую незавершённую работу.
Этот документ описывает **предлагаемое** поведение. Числовые профили ниже —
гипотезы для стенда, а не свойства провайдеров и не утверждённые release defaults.

Область: отдельный продукт Relay, личные аккаунты, пользовательские API-источники,
desktop и user-managed server. Это не production Gateway и не клиентский биллинг
Zenith. Примеры синтетические, реальные credentials не нужны для стенда.

## 1. Главный результат, ради которого делается замена

Пул не должен сам превращать единичный сбой одного источника в остановку всех.
Для обычного пользователя это означает:

- A не принимает запрос, B доступен и разрешён — пробуем B без общей паузы.
- У A кончилась квота — ждёт A, а не весь пул.
- Все заняты — ограниченно ждём свободного места, не объявляя аккаунты сломанными.
- Модель долго думает — не создаём вторую генерацию только из-за отсутствия текста.
- Пользователь нажал «Стоп» — этот запрос не повторяем; сам аккаунт не выключаем
  навсегда только потому, что у провайдера нет API статуса удалённой работы.
- Проблемный источник получает ограниченную возможность вернуться, но не забирает
  весь трафик у работающих источников.
- Ошибка обновления баланса или списка моделей не выключает рабочую генерацию.
- Relay не разрешает новые платные пути молча и не обещает контроль списаний,
  который upstream-протокол реально не даёт.

Успех ротации — не обещание «любой запрос всегда завершится». Требуется сохранять
доступную ёмкость, не терять контекст, не дублировать неизвестно принятую работу
и объяснять причину отказа. Проблемы провайдера при этом не исчезают.

## 2. Какие решения прежнего проекта заменяем

| Прежняя конструкция | Почему её недостаточно или она опасна | Новое решение |
| --- | --- | --- |
| После uncertain cancel держать удалённый слот до ручного reconcile | Для sync API без job-status одна отмена могла блокировать источник навсегда | Локальный lease и доказанная удалённая job-capacity — разные ресурсы; для sync API новый независимый запрос не блокируется одной неопределённостью (§9) |
| Degraded primary всегда раньше здорового резерва | При разрешённом резерве приоритет мог сохранять неисправность вместо доступности | Обычный трафик идёт на готовый разрешённый источник; возврат primary регулируется общей квотой recovery (§5, §10) |
| Один overflow-флаг одновременно про порядок, занятость и деньги | Невозможно отличить «ждать аккаунт» от «не разрешаю этот расход» | Разделить выбор, waiting policy и разрешение использования источника/overage (§5) |
| Due recovery получает приоритет без общего ограничения | Десятки больных источников способны забирать запросы по очереди | Ограничения recovery на источник **и на пул**, справедливая очередь tickets (§10) |
| Любое событие меняет общий epoch | Защита от позднего success может выкидывать независимые одновременные failures | Отдельные identity, incident и lease generations; явный порядок наблюдений (§10) |
| `replayable` как достаточный признак безопасного повтора | Сериализуемое тело не доказывает, что первая генерация не началась | Разделить repeatable input, execution evidence, portability и idempotency (§7) |
| Один числовой test profile уже достаточен для подключения | Тестовые лимиты могут незаметно стать пользовательской политикой | Версия конфигурации, adapter capability matrix, автоматическое преобразование с сохранением настроек и испытания (§12, §17–19) |
| Один большой новый refresh/executor вместо всех сервисов | Можно создать второго владельца OAuth и переписать исправный код | Общий контроллер попыток; authority, parsers, stores и reference loaders сохраняют владение (§3, §12) |
| Все runtime blocks — критическое persistent состояние | Временные сбои и повреждённый cache способны выключить весь runtime | Отделить безопасность/credentials/config от восстанавливаемых наблюдений (§14) |
| Сначала большое абстрактное ядро, затем подключение | Unit-тесты ядра не замечают расходящиеся реальные HTTP/WS пути | Сначала контракты и стенд, затем вертикальный путь от admission до terminal (§18) |

Не отменяем правильные основы: один владелец retry, точные scopes, идемпотентный
release, сохранение контекста, отсутствие failover после commit, single-flight
авторизации, проверку stale observations, ограничения памяти и отсутствие secrets
в диагностике. **Пересмотр архитектуры не означает удалить весь рабочий Relay.**

## 3. Границы системы и владельцы решений

Не проектируем один монолит, который одновременно хранит токены, разбирает SSE,
выбирает аккаунт и обновляет баланс.

| Компонент | Отвечает за | Не делает |
| --- | --- | --- |
| Router | Допуск, причины исключения, стратегия выбора и очередь готовности | Provider HTTP, OAuth, разбор тел |
| Resource registry | Физическая ёмкость, permits, lease, атомарный reserve/release | Оценку «стабильности» по таймеру |
| Attempt controller | Один request context, dispatch permission, retry/commit/cancel lifecycle | Самостоятельную provider-specific классификацию |
| Observation reducer | Auth/quota/rate/health observations и fences | Генерации и скрытые проверки |
| Source refresh service | Due jobs, budgets, coalescing, revision-safe применение | Второй OAuth manager или собственную ротацию |
| Существующий token authority | Token refresh, lock+reread, безопасную запись и login CAS | Сброс quota/health после получения токена |
| Adapter/transport driver | Протокол, wire errors, parsing и проверяемые evidence | Самостоятельный выбор другого источника/бесконтрольные retries |
| Host desktop/server | Vault, persistence, lifecycle, management API | Отличающийся алгоритм выбора |
| UI | Настройки, снимок состояния, запрос refresh через Rust | Таймеры provider HTTP и управление lease |

HTTP, SSE, WS и images остаются отдельными drivers с одним контрактом контроллера.
«Один executor» означает **один владелец политики попыток**, а не одну огромную
async-функцию. Reference metadata/prices уже имеют loaders: они могут делить общий
лимит фонового HTTP, но не обязаны переписываться ради ротации.

### 3.1. Граница исходного визуального бага

Группировка `thinking` и tool calls относится к протоколу и отображению клиента.
Ротация проверяет upstream dispatch и commit, а не внешний вид «Думает».
Один dispatch с несколькими событиями не доказывает повтор генерации.
Нужны redacted request/attempt IDs, event kinds и момент commit, без содержимого
prompt/response. Исправление группировки — отдельная работа, если трасса её подтвердит.

## 4. Идентичности, ресурсы и независимые состояния

### 4.1. Не путать карточку, маршрут и физический лимит

- `MemberId` — запись аккаунта/API-источника в пуле.
- `CapacityKey` — известный общий физический ресурс; aliases одного member делят
  лимит. Дубли одной identity связываются только по проверенному adapter/authority
  contract, не по display name. Если общность неизвестна, Relay не обещает её учёт.
- `RouteKey` — member, конкретные model mapping, protocol, operation и существенный
  execution profile. Нельзя строить декартово произведение «все models × все routes».
- `QuotaBucketKey`/`RateScopeKey` — лимит с собственным scope и единицами.
- `TransportScope` — endpoint/proxy и транспортная конфигурация. Общий proxy может
  блокировать несколько routes, но одинаковое имя провайдера ничего не доказывает.
- `PrincipalScope` — локальный request key и разрешённые ему members/operations.
- `OwnerBinding` — identity и scope opaque response/file/cache/continuation.
- `RequestId` — локальная логическая операция. В постоянном WS новый turn имеет
  новый request context; WS → HTTP fallback того же turn его сохраняет.
- `AttemptId`, `LeaseId`, `RuntimeEpoch` — попытка, reservation и поколение runtime.

Каждый изменяемый ресурс имеет свои revisions. Изменение веса или display name
не инвалидирует квоту. Обычный token refresh не создаёт новую account identity.
Замена endpoint не обязательно создаёт новую квоту: adapter решает, что относится
к identity/project, а что к transport. Нельзя сбросить block изменением подписи.

### 4.2. Не один `healthy` на всё

Состояния configuration, authentication, allowance, rate, circuit, local capacity,
inventory, balance и ownership хранятся отдельно. Например:

```text
account A: auth usable, quota unknown, model X backoff, model Y ready, in_flight 1/2
API B: inference ready, balance stale, models last-known-good
```

Успешный quota read не доказывает inference health. Inference success не обновляет
баланс и не снимает ограничение другой модели. Ошибка stats 403 не доказывает,
что inference credential отозван. Каждый результат содержит origin, scope,
тип наблюдения, evidence, started revision и допустимое действие восстановления.

### 4.3. Инварианты независимо от выбранного режима

1. Rotation не меняет молча модель, reasoning, speed/tier, tools, формат, контекст
   и вложения. Protocol adaptation сохраняет поддержанную семантику либо отклоняет
   запрос; она не даёт разрешения урезать возможности ради успешного fallback.
2. Ни вес, ни «последний аккаунт», ни recovery не обходят access/spend/owner policy.
3. У одного запроса один context, монотонные budget counters и необратимые
   cancel/commit latches. Ни alias, ни refresh, ни смена протокола их не сбрасывают.
4. Release касается только своего lease. Busy и local cancellation не являются
   доказательством неисправности провайдера.
5. Повторяемый input, удалённое принятие и downstream commit — независимые факты.
6. Результат применяется только к своему scope и поколению; новый login нельзя
   затереть поздним refresh. Счётчики concurrency и health имеют разные fences.
7. Unknown usage/quota/balance не превращается в ноль. Ошибка monitoring не является
   автоматически inference block.
8. Тишина генерации без явного deadline не является failure. Фоновые jobs bounded,
   а recovery не создаёт скрытую оплачиваемую генерацию.
9. Память, admission, очередь, retry и management traffic имеют конечные лимиты.
   Диагностика не содержит secrets или содержимое пользовательских запросов.

## 5. Выбор источника, резерв и пользовательские режимы

### 5.1. Три существующих понятных режима, не десяток новых

Предлагается сохранить три продуктовые стратегии, а не объявлять retry, quota,
recovery, paid overflow и affinity отдельными «режимами ротации»:

| Режим | Поведение после общих проверок |
| --- | --- |
| **Автоматически** | Готовые источники; сначала меньшая нормированная локальная нагрузка, затем weighted выбор среди равных. Без скрытого score из денег, процентов разных квот и TTFT |
| **По порядку** | Первый готовый разрешённый member в сохранённом порядке; занятые/заблокированные не останавливают просмотр списка |
| **По кругу** | Smooth weighted round robin между готовыми физическими members; доля запросов, не токенов, денег или времени |

Автоматический режим — предлагаемый default для **нового** пула. Нормированная
нагрузка `in_flight / effective_local_capacity` сравнивается без float; знаменатель
конечен благодаря runtime cap. Поле `max_concurrency=0` означает отсутствие
дополнительного member-ограничения, не бесконечные ресурсы.
В автоматическом режиме веса разрешают ничью по нагрузке, не обещают точные доли.
В режиме «По кругу» веса определяют долю admission при неизменном eligible subset.
В режиме «По порядку» вес не влияет на результат и не должен притворяться активным.

Smart автоматически преобразуется в Automatic при обновлении до 1.1.3.
Порядок, веса, лимиты и разрешения сохраняются; отдельного migration UX нет (§17).

### 5.2. Membership, резерв и расход — разные настройки

По умолчанию все уже явно включённые разрешённые members доступны стратегии.
Relay не объявляет аккаунт «бесплатным», а API «платным резервом» по типу записи.
Явное включение API для inference разрешает обычные вызовы этого источника;
переключение A → B внутри этого набора не требует новой галочки на каждую ошибку.

В расширенной политике могут быть **основные** и **резервные** members:

- `reserve_allowed` — разрешено ли вообще использовать выбранный резерв;
- `busy_preference_wait` — сколько предпочесть ждать основные, **только если они
  заняты**, но исправны; предлагаемый default 0;
- `provider_overage_allowed` — отдельное разрешение дополнительного provider
  credits/overage-пути. Оно не выводится из веса, режима или наличия резерва.

Не вводим произвольный язык вложенных priority groups в первую версию.
Сохранённая очередность работает внутри набора; вход в резерв имеет явную причину.
Если сохранённый формат уже содержит дополнительные tiers, до включения нужен
явный migration mapping, а не потеря tier или копирование скрытой старой политики.

### 5.3. Порядок выбора

1. Отфильтровать несовместимость, запрещённый расход, auth/access/quota/rate blocks.
2. Применить hard owner; перенос возможен лишь с доказанным portable envelope.
3. Среди основных выбрать `Ready` по выбранной стратегии.
4. Если основные только busy, ждать исключительно явный `busy_preference_wait`,
   не дольше общего admission budget. При default 0 сразу проверить разрешённый резерв.
5. Если основных готовых нет, использовать `Ready` разрешённого резерва.
   Quota/auth/backoff не заставляют сначала ждать busy-таймер.
6. Контролируемая recovery-попытка может заменить обычный выбор только по общему
   recovery budget (§10). Degraded primary не получает безусловного преимущества.
7. Если готового источника нет, но есть совместимый due trial — взять его permit.
8. Если в оставшееся время возможно actionable событие — встать в bounded queue;
   иначе вернуть точную причину, а не спать заведомо бесполезные 30 секунд.

После безопасного failure выбираем прежде не опробованный независимый источник.
Смена alias/protocol не делает тот же account независимым. Same-member retry
используется для явно разрешённого auth/compatibility repair или при отсутствии
альтернативы после его `not_before`, всегда в общем бюджете.

### 5.4. Affinity и fairness

Hard owner действует во всех режимах, не может быть сломан ради распределения.
Soft affinity — ограниченное предпочтение только среди равно допустимых winners
автоматического режима, с bounded TTL и состоянием; она не удерживает busy/bad A
вместо готового B. «По кругу» soft affinity игнорирует.

SWRR обновляется только при успешном reservation, не на preview/refresh UI.
Absent subset не накапливает долг, aliases не увеличивают вес, изменение веса
делает явный rebase. Состояние ограничено configured routes/principal classes,
а не произвольными строками модели из невалидированного клиентского запроса.

## 6. Request lifecycle и ёмкость

```text
validate + bounded immutable envelope + request context
  -> eligibility/owner/spend
  -> prepare auth, если требуется, без inference lease
  -> atomic recheck + reserve всех нужных локальных ресурсов
  -> pre-dispatch fence + разрешение одной wire attempt
  -> transport send и наблюдение evidence
  -> downstream commit (если что-либо публикуется)
  -> terminal/cancel/disconnect
  -> release + scoped observation
  -> безопасный retry с тем же context либо завершение
```

Под scheduler lock нет HTTP, OAuth, await и sleep. Между prepare и send проверяются
identity/token/config revisions. Если credential успел измениться, до отправки
освобождаем reservation и повторяем prepare в том же бюджете ожидания.

Reserve атомарно получает member capacity, operation lane, runtime cap и необходимые
rate/recovery permits. Частичный захват откатывается. Alias не даёт дополнительный
слот. Quota estimate не выдаётся за точную reservation неизвестных provider tokens.
Известный локальный RPM permit и неизвестная subscription quota — не одно и то же.

Lease содержит immutable owner и generation. Release идемпотентен, выполняется
на каждом exit/drop/error path. Поздний callback не освобождает новый lease.
Removal сохраняет tombstone до завершения активной работы; limit shrink не убивает
streams. Новые attempts после disable/key revocation запрещены.

Применение scheduling outcome и release сериализованы до уведомления waiters.
Между «слот освободился» и «на источник установлен уже известный block» нельзя
допускать новый request на старом snapshot. Parsing и persistence I/O остаются
за пределами этой короткой критической секции.

`begin_dispatch` — точка сериализации последнего gate, budget и permission.
Если cancel/revoke выиграл эту гонку, новый send не начинается. Если permission
уже выдан и отправка могла начаться, отмена обрабатывает in-flight attempt,
не переписывая его как `NotSent`. Не обещаем физически отозвать уже ушедшие bytes.

### 6.1. Очередь

Один request ждёт без lease и учитывается один раз. Ограничиваются количество,
retained bytes, общий runtime и per-principal лимит. Service выбирает старейшего
совместимого waiter с fairness между principals; несовместимый head не блокирует
все остальные модели. Cancel сразу снимает waiter.

Wake-up по release, auth/quota/config event, due timer или deadline; нет polling
всего пула каждые несколько миллисекунд. После wake-up повторная проверка обязательна.
Для обязательных блоков берётся максимум применимых `not_before`, для альтернатив —
минимум достижимых. Бесконечное login-required не получает выдуманный retry-at.
Частота попыток reserve после гонки тоже bounded и не расходует provider dispatch.

## 7. Повторы: четыре независимых вопроса вместо `replayable: bool`

До повторной отправки контроллер должен ответить на четыре вопроса:

1. **InputRepeatable:** можно точно воспроизвести тело и вложения?
2. **ExecutionEvidence:** могла ли предыдущая попытка уже запустить работу?
3. **TargetPortability:** разрешён ли перенос owner/context на этот target?
4. **IdempotencyContract:** если работа принята, есть ли документированная дедупликация
   именно этой операции, identity, endpoint, ключа и периода хранения?

```text
ExecutionEvidence:
  NotSent                  доказанно не отправили application request
  RejectedBeforeExecution  provider/adapter доказал отказ до запуска работы
  Accepted                 есть подтверждение принятия
  Unknown                  отправка/результат могли произойти
  Terminal                 известен финальный результат, success/refusal/error отдельно
```

`InputRepeatable=true` **не разрешает** повтор `Accepted/Unknown`.
В MVP обычные генерации с такими evidence не повторяются. Adapter-specific
идемпотентный resume/deduplication — только после отдельной проверки, не общий
флаг безопасности. Новый idempotency key не защищает от дубля; тот же ключ на другом
провайдере тоже не даёт такой гарантии.

Envelope ограничен по размеру и неизменяем. Нельзя перечитать уже изменившийся
локальный файл или повторить частично потреблённый upload как исходное вложение.
Multipart, compressed bodies и decoders сохраняют свои byte/expansion limits.
Известный terminal error после начала работы тоже не равен pre-execution rejection.
Успешная compaction и следующая generation — разные шаги одной операции, не retry
успешной compaction; оба расходуют work budget, если запускают оплачиваемую работу.

### 7.1. Матрица исходов

| Ситуация | Текущий запрос | Состояние источника |
| --- | --- | --- |
| Доказанный DNS/connect failure до application send | Можно выбрать B до commit, если тело повторяемо | Transport-scoped backoff, не auth/quota |
| Потеря связи после возможной отправки | No transparent replay без проверенного deduplication contract | Unknown outcome; transient signal только доказанного origin |
| Проверенный 401 до запуска работы | Shared auth refresh; максимум один повтор с обновлённым credential | Auth scope; не общий circuit failure |
| Проверенный 429/rate/quota rejection до работы | B или bounded wait по scope | Rate/bucket block с реальным hint |
| 502/503/504 без доказанной pre-execution rejection | Сам статус не делает POST безопасным | Scope/health отдельно от replay safety |
| Invalid input | Завершить, не обходить все accounts | Нет provider failure vote |
| Корректный refusal/tool result | Вернуть результат | Не network failure |
| Explicit unsupported model/operation | Другая совместимая разрешённая route лишь при доказанном отказе до работы | Только capability/route scope |
| Client cancel/local disk/parser error | Не создавать новую генерацию | Не штрафовать upstream без доказательств |
| Ошибка после downstream commit | Завершить stream ошибкой/incomplete | Никакого склеивания ответа другого source |

Origin и parsing contract обязательны: 403 от WAF, stats 403 и inference access
403 не взаимозаменяемы. Не маркируем любой 400 как безопасный protocol repair.
Transport должен доказать pre-send boundary, а не определять его по наличию текста
«connection» в ошибке. Неизвестная классификация не разрешает рискованный replay.
Generic HTTP-proxy retry для non-idempotent POST отключён. Возможный повтор
прикладной операции решает controller с проверенным adapter contract; это не
универсальное разрешение транспортному proxy повторять POST по коду статуса.

### 7.2. Один бюджет, но не путать transport и генерацию

`RequestContext` создаётся на входе один раз, содержит cancellation/commit latches,
owner/envelope, client deadline и все счётчики:

- `wire_attempts`: попытки соединения/HTTP exchange, включая неудачный connect и WS
  handshake; увеличивается до сетевого действия, ограничивает network churn;
- `work_sends`: отправки операций, способных запустить оплачиваемую работу;
  generation, image и billable compaction учитываются здесь;
- `same_member_retries`, `auth_rechecks`, `repair_count`;
- общий accumulated queue wait и retry-start window;
- runtime/fault-domain retry permits и byte budgets.

В HTTP generation exchange расходуются wire и work budgets до попытки отправки
тела; неудачный connect не возвращает work budget автоматически. В WS handshake
расходуется wire budget, а `response.create` — work budget. Поэтому handshake 401
не приравнивается к принятой генерации, но не становится бесконечным бесплатным
циклом. На persistent WS каждый новый turn получает context, не каждый frame.
OAuth exchange и metadata reads имеют management budget; не притворяются generation,
но ожидание auth не выходит за request admission/deadline.

Скрытые retries HTTP client, redirect replay, SDK, auth helper, WS reconnect,
protocol repair и compaction проходят через тот же permission boundary или
отключаются. Host/adapter не может создать новый context для текущего запроса.
Лимит выбирается **до** отправки, не проверяется задним числом после двух callbacks.

### 7.3. Ожидание и отсутствие глобальной паузы

Backoff A не добавляет sleep перед готовым независимым B. Provider Retry-After
ограничивает свой resource/domain, а не любые другие accounts. Общий retry limiter
может остановить дополнительные попытки при шторме, но не отключает первичные
запросы к здоровому независимому источнику.

Retry-start window открывается после первого replay-safe failure, не от первого
байта входящего запроса, и не сбрасывается на alias/fallback. Оно запрещает старт
новой попытки после истечения, но не обрывает уже начавшуюся здоровую генерацию.
Общий client deadline, если задан, остаётся сильнее. Новые запросы клиента — новые
contexts; Relay не обещает exactly-once между ними без отдельного idempotency API.

## 8. Commit, streaming и сохранение контекста

Commit latch устанавливается **перед** первой возможной публикацией response:
HTTP headers, SSE comment/heartbeat, metadata, reasoning, tool event, text или
WS response/error frame текущего turn. Даже если write упал, клиент мог получить
часть данных — latch не откатывается.

WS upgrade и Ping/Pong не являются commit turn. Внутренний bounded pre-output
buffer тоже не commit, пока он не опубликован. Отсутствие downstream commit
не доказывает отсутствие upstream принятия: нужны обе независимые проверки.

JSON, streaming, native WS, HTTP bridge и images должны иметь одинаковый смысл
latches. Нельзя убрать `previous_response_id`, encrypted reasoning, tool result,
file/cache reference или вложение, чтобы сделать запрос якобы переносимым.
Portable replay требует полного validated envelope; владельцы opaque state
восстанавливаются из проверенного binding, не по совпавшей строке response ID.

Обычный token refresh может сохранить логическую identity/owner, но новый turn
перепроверяет credential-bound WS/turn-state fingerprint. Новый login не наследует
opaque state старой identity. Активный stream не обрывается только из-за refresh.

### 8.1. Длинные запросы и медленный клиент

Нет скрытого «5 секунд без текста — fail». Ограничены connect/TLS/handshake,
management HTTP, входное тело и очередь. Ожидание generation headers/первого
полезного события нельзя случайно ограничить connect timeout: provider может
начать работу до первого response byte. Total/idle generation deadline — только
явная поддержанная request/runtime policy; expiry после send не разрешает retry.

У downstream есть bounded buffering и отдельная политика slow-reader/cancel.
Медленный клиент не ухудшает health провайдера. Если для поддержания SSE приходится
отправить heartbeat, это закрывает failover window; нельзя одновременно обещать
ранние heartbeat и неограниченный pre-commit fallback. Внешний proxy/client может
иметь свои timeouts — Relay не обещает держать их соединение бесконечно.

## 9. Cancel: не повторять работу и не выключать аккаунт навсегда

### 9.1. Обычные синхронные HTTP/SSE/WS операции

1. Cancel закрывает локальную попытку и запрещает все дальнейшие sends её context.
2. Освобождаются локальные lease, buffers и waiter; release идемпотентен.
3. После possible send сохраняется bounded redacted outcome `remote_unknown`.
4. Это **не** утверждение, что remote job остановилась, и не нулевой usage.
5. Следующий независимый запрос может использовать источник при обычных auth,
   quota, rate, health и local-capacity проверках. Один неизвестный cancel сам по
   себе не создаёт вечный hard block и не требует ручного «воскрешения» аккаунта.
6. Автоматически повторить отменённый запрос под новым RequestId запрещено.

`max_concurrency` здесь означает локальные управляемые attempts. Строгую удалённую
concurrency после disconnect Relay не гарантирует. Она контролируется provider
limits; полученный rate/concurrency rejection обрабатывается по своему scope.
Нельзя назвать истечение произвольного orphan TTL доказательством окончания job.
TTL ограничивает только хранение диагностической записи.

### 9.2. Операции с настоящим remote job contract

Если adapter поддерживает job ID, remote status/cancel и известный remote capacity
contract, существует **отдельный** remote permit. Его освобождает terminal status,
подтверждённый cancel или доказанная provider lifetime, не локальный disconnect.
Reconcile bounded, его ошибки не блокируют другие operations/sources.

Это возможность конкретного adapter, а не обязательное условие работы generic API.
Remote permits переживают restart только при таком контракте. В отсутствие status
API не изобретаем постоянные «удалённые слоты». Usage, delivery и provider terminal
хранятся отдельно; delivery failure после terminal success не запускает генерацию.

## 10. Ошибки, circuit и ограниченное восстановление

### 10.1. Scope важнее имени провайдера

- Model rejection — конкретная route/model.
- Credential rejection — identity/auth scope.
- RPM/TPM/quota — соответствующий bucket.
- Proxy/connect — доказанный transport domain; без «лечения» сменой alias.
- Inference transient — минимальный обоснованный operation/route scope.

Не копируем всё в один счётчик аккаунта. Success одной модели не лечит другую,
успех `/models` не закрывает inference circuit. Shared project limits связываются
только по contract; rotation не должна обходить условия provider limits.

### 10.2. State machine

```text
closed -> countable transient -> degraded + короткий scoped backoff
несколько независимых failures одного incident -> open
open/degraded + due + recovery permit -> trial/half_open
trial + terminal success -> closed, новый incident
trial + transient failure -> open, следующий backoff
trial + cancel/local failure -> permit released, нейтральный paced повтор позже
```

Consecutive threshold — только базовый detector малого пула, не доказанный лучший
вариант для любого traffic pattern. Failure ratio/latency score не входят в MVP.
Временной диапазон streak ограничен: разнесённые надолго ошибки не копятся вечно.
Terminal success важен; первый token не подтверждает полный успешный ответ.

### 10.3. Concurrent outcomes: не один epoch на каждое событие

Контракт reducer:

- `IdentityEpoch`/transport revision отсекают результаты действительно другого
  login/endpoint. Policy-only изменения не уничтожают полезные observations.
- `IncidentId` объединяет failures одной health-ситуации и **не увеличивается
  на каждом failure**. Три одновременно выданных leases могут дать три голоса.
- Один `RequestId` даёт максимум один failure vote на health scope/incident.
- При применении failure запоминается admission fence: success уже выполнявшейся
  тогда попытки не доказывает восстановление после этой новой ошибки.
- Актуальный trial permit или допустимый success попытки, начатой после fence,
  может закрыть incident. Только после этого прежний incident становится historical.
- Поздние outcomes завершённого incident идут в metrics, не открывают/закрывают
  новый автоматически. Quota/auth observations рассматриваются отдельным reducer.
- `LeaseGeneration` защищает release, `ProbeGeneration` — владение trial permit;
  ни один из них не заменяет учёт независимых health failures.

Порядок применения сериализован; попытка регистрируется до send и settle принимает
только принадлежащий ей context. Невозможные последовательности вроде «terminal
success без begin_dispatch» не используются как happy-path fixtures.

### 10.4. Recovery budget на весь пул

Просто выбрать только healthy навсегда — starvation. Просто проверять каждый due
source раньше healthy — outage от recovery. Нужны обе границы:

1. Один trial одновременно на конкретный resource scope.
2. Bounded общее число trials в runtime и отдельный rate limit fault domain.
3. При наличии ready-альтернатив — exploration budget на runtime: initial burst 1,
   затем один credit за K успешных обычных пользовательских запросов, cap 1.
   Credit расходуется при реальном trial dispatch, не при просмотре кандидатов.
4. Для каждого source действует свой `not_before`; общий permit не сокращает его.
5. Due tickets обслуживаются oldest-compatible-first; hot модель не вытесняет
   другую при наличии подходящих запросов. Нет подходящего спроса — нет probe.
6. Если нет ни одной готовой альтернативы, разрешён один due demand-driven trial
   без exploration credit, но со всеми concurrency/backoff/rate gates.
7. Recovery никогда не обходит owner, spend, auth или обязательную quota.

K — числовая гипотеза, не пользовательский «четвёртый режим». При healthy-трафике
проверок не больше `1 + floor(healthy_completions / K)` на runtime, кроме явно
размеченного режима без готовых альтернатив. Не создаём отдельный burst на каждую
модель или новый principal. При малом спросе восстановление может занять больше
времени; точный момент возврата без запроса не обещается.

Этот механизм сознательно заменяет прежний запрет на счётчик доли recovery:
временной pacing **одного** source недостаточен для большого числа sources.
Никаких скрытых оплачиваемых генераций, hedging и background inference probes.

## 11. Quota, rate limit, credits и деньги

Quota — набор buckets, не универсальный процент аккаунта. Каждый bucket имеет
identity/project/model/operation scope, единицы, observation source/version,
observed/as-of/reset time при наличии, freshness и evidence. Нет hardcoded пяти
часов/недели или особого routing policy для Free в общем ядре.

- Unknown/stale observation не равна нулю и не штрафуется в Smart.
- Подтверждённый отрицательный block не пропадает только от истечения cache TTL.
- Если операция требует два окна, должны разрешать оба; reset одного не лечит второе.
- RPM/TPM, subscription allowance, API wallet и local usage не взаимозаменяемы.
- Нельзя точно вычитать незавершённую генерацию из provider dashboard balance.
- Account quota и API quota описывает adapter; исключение — только затронутый bucket.

Допустимый credits-путь может альтернативно заменить subscription allowance только
по проверенному adapter contract. Он не отменяет auth/access/RPM/concurrency.
Если upstream сам расходует деньги и протокол не позволяет это запретить, checkbox
Relay не является гарантией: UI явно предупреждает, а не называет путь бесплатным.
Не добавляем универсальный allowance DSL или автоматический выбор «дешевле».

### 11.1. Возврат после quota block

В момент `reset_at` заканчивается обязательное ожидание, но не появляется выдуманная
fresh quota. Делаем coalesced quota read. Если read недоступен, возможен один реальный
совместимый trial по adapter policy после `not_before`, без hidden generation.
Для неизвестного reset используются paced reads/trials, не бесконечный hard block
и не повтор в каждом запросе. Авторитетный access restriction trial не обходит.

Read получает observation fence до отправки. Старый positive, пришедший после
нового 429, не снимает его. Даже read, начатый позже, может получить cached payload:
для снятия block нужны provider as-of/cycle/version или проверенный freshness
contract. Без них не заявляем строгую causal freshness; используем paced trial,
а не немедленный массовый возврат. Top-up/rolling windows не обязаны монотонно
уменьшать remaining. Late observations другой identity не применяются.

Quota trial — явное состояние `RecoveryEligible` после применимого reset/backoff
и проверки adapter contract. Это не обход ещё действующего обязательного запрета.
Другие обязательные окна и ограничения продолжают действовать.

## 12. Обновление аккаунтов, моделей и API-баланса

### 12.1. Сначала capability profile, потом таймеры

До подключения adapter описывает:

| Контракт | Что требуется |
| --- | --- |
| Auth | Authority, expiry, refresh semantics, stale login fence, uncertain-exchange policy |
| Inference | Protocol/operation, pre-send evidence, typed rejection, commit/terminal signals |
| Owner/replay | Что переносимо, какие references требуют исходную identity |
| Quota/rate | Buckets, единицы, scopes, hints, reset/freshness и разрешённый recovery |
| Models | Endpoint/manual inventory, completeness/pagination/304 semantics |
| Balance/stats | Endpoint и отдельные credentials/scopes при наличии; read-only смысл |
| Remote work | Sync local capacity либо проверенный job/cancel/status contract |

Capability может быть `unsupported` или `unknown`. Это не повод выдумывать endpoint
и не причина выключить inference. Live поддержка некоторых stats endpoints требует
согласованного provider профиля; generic API не обязан иметь баланс в принципе.

### 12.2. Разные типы источников

| Тип | Auth | Models | Quota/balance |
| --- | --- | --- | --- |
| Личный OAuth-аккаунт | Существующий authority | Поддержанный account inventory path | Provider-specific quota/subscription; не универсальный API wallet |
| Обычный API key | Без OAuth exchange; проверенный auth failure требует смены ключа | Configured `/models`/adapter inventory либо manual | Только реально поддержанный read; unsupported не poll-ится |
| Provider с management/stats API | Management и inference scope не смешиваются | Через проверенный adapter | Wallet, key allowance и subscription отображаются отдельно |
| Source за Sub2API | Только явно настроенный base URL и согласованные paths | Catalog этого source | Только доступные этому user-owned source statistics; не внутренние accounts реселлера |

UI не угадывает приватный кабинет и не получает provider secrets напрямую.
Sub2API не получает скрытый второй endpoint/CDN по совпадению названия.

### 12.3. Начальная частота для стенда

`active` — был реальный dispatch за последние 10 минут или есть in-flight/waiter
для источника. Открытая карточка показывает cache; сама по себе не превращает весь
пул в active и не запускает poll. Explicit manual refresh — отдельное событие.

| Данные | Active | Idle, включённый мониторинг | События вне расписания |
| --- | --- | --- | --- |
| Access token | По expiry и необходимости, пример margin 60 с | Не обновлять без предстоящего использования; refresh может требоваться monitoring job | 401 проверенной generation, login, resume, auth prerequisite |
| Quota/subscription | Не чаще 5 мин при отсутствии свежих passive данных | 15 мин | Подтверждённый 429, reset due, top-up, manual |
| Account/provider models | 8 ч | 24 ч | Первое подключение, relevant config/login change, manual |
| API balance/stats | 5 мин | 30 мин | Manual/top-up, проверенный payment signal |
| Reference metadata | Сохранить собственную loader policy, ориентир 1 ч | По существующему loader/cache contract | Catalog/config change |
| Reference prices | Сохранить собственную loader policy, ориентир 24 ч | По существующему loader/cache contract | Явное обновление |

Это отдельные freshness/due policies, а не обязательный GET каждые N минут.
Свежая пассивная quota из inference может заменить quota poll **того же scope**,
но не balance/models. Error backoff может увеличить интервал, provider hint всегда
соблюдается. После каждой generation не выполняется полный refresh аккаунта.

Disabled monitoring/source не опрашивается фоново. Аккаунт вне inference pool можно
мониторить при отдельном включённом monitoring, не включая его в ротацию.
Для неподдерживаемого read нет periodic job; для temporary failure — backoff, для
подтверждённого unsupported — остановка kind до config/manual recheck.

### 12.4. Один владелец job, но не один последовательный worker

Ключ `(resource scope, kind, relevant identity/config revisions)` задаёт single-flight.
Если один endpoint возвращает несколько видов данных, один read может публиковать
несколько observations. Несколько UI/manual/startup callers присоединяются к job.
Один отменившийся waiter не отменяет shared работу для остальных.

Management concurrency/rate ограничены runtime и origin. Auth prerequisite имеет
защищённую долю worker capacity, чтобы медленные stats не задержали авторизацию;
quota reset — следующая приоритетность, models/stats — ниже. Нижние классы получают
fair share, не вечное starvation. Auth authority остаётся единственным владельцем
exchange; source service лишь запрашивает у него ensure/join.

Dirty event имеет sequence. Job фиксирует captured sequence до send; событие,
пришедшее во время job, вызывает максимум один follow-up после completion.
Повторные UI requests не отодвигают due бесконечно и не обходят minimum interval.
Provider Retry-After сильнее dirty/manual. Jitter не сокращает обязательный hint.
Successful job без нового пригодного результата получает no-progress backoff:
иначе expiry-driven refresh способен крутиться бесконечно на одном token.
Для read-only polling неизменившееся, но валидно перепроверенное значение — успех,
а не no-progress: постоянный баланс сам по себе не ошибка.

### 12.5. Auth refresh и сохранение login

- Startup/resume сначала перечитывает текущий credential; usable token не меняется
  просто потому, что приложение открыли. Неизвестный expiry не означает expired.
- Margin ограничивается сроком жизни token; короткий token не порождает немедленный
  бесконечный refresh. Никакого общего exchange каждые пять минут.
- Lock + reread + generation CAS до применения; более новый login побеждает.
- Новый credential должен безопасно сохраниться до объявления его durable/usable.
  Persistence retry не делает второй OAuth exchange ради повторной записи.
- Потерянный ответ на rotating refresh token — uncertain operation. Нет универсального
  safe replay: authority использует provider-specific grace/reconcile contract либо
  просит повторный вход. Ещё валидный access token не отзывается без доказательства.
- Изоляция auth scope: отказ management token не удаляет inference key автоматически.
- Не создавать новый durable intent manager параллельно существующему authority.
  Если его contract недостаточен, менять в отдельном проверяемом auth-срезе.

### 12.6. Model inventory

Bounded HTTP/pagination с проверкой origin, redirect policy, cursors, duplicates,
size и completeness. 304 применим лишь к совпадающим identity/config/validator.
Полный валидный результат заменяет inventory атомарно. Partial/malformed/timeout
сохраняет last-known-good с stale status; неизвестная форма не считается пустым
списком. Подтверждённый полный пустой inventory может быть валидным.

Provider inventory доказывает доступные IDs; семантика capabilities/limits/reasoning
остаётся у Relay validated reference catalog. Manual filters не стирают inventory.
Поддержка protocol/operation проверяется конкретной route, не только именем модели.
Изменение inventory не прерывает уже начатые streams. Модель не пропадает из
пользовательского catalog только потому, что все её sources временно busy/backoff.
Источник без models API может работать с проверенным manual inventory.

### 12.7. Balance и stats

Last successful value, `as_of`, freshness, unit/currency и refresh error видны отдельно.
Unknown не превращается в 0; stale value не выдаётся за live. Management 401/403/404,
429 или timeout не выключает inference без доказанной общей причины.

Wallet, key limit и subscription allowance не складываются в «общий остаток».
Неизвестный курс не выдумывается; локальный usage не выдаётся за точный dashboard.
Top-up ставит bounded recheck, не бесконечный loop до желаемого баланса. Отсутствующий
balance endpoint отображается «провайдер не сообщает», не «обновление сломалось».

### 12.8. Sleep/resume и время

Во время работы используются монотонные deadlines. Provider dates и persisted
wall-clock values проверяются отдельно. После sleep/resume просроченные jobs
coalesce, распределяются jitter/origin limiter и не запускаются все одновременно.
Jump wall clock не создаёт huge sleep или отрицательный busy loop. Прошедшее время
не доказывает health/окончание remote job. Resume не является новым login.

## 13. Фактические ограничения и пример test profile

Инварианты обязательны независимо от чисел; defaults подтверждаются измерениями.
Предлагаемый fixture profile **не должен импортироваться как скрытая policy**:

| Параметр | Стендовое значение | Граница |
| --- | --- | --- |
| Work sends на request | 3, включая первую | Общий budget; auth generation replay и compaction не бесплатны |
| Wire attempts | 6 | Включая connect/WS handshake; более слабый work cap остаётся |
| Same-member retries | 1 | Нужна отдельная причина и safety, не право повторить unknown; последовательные шаги операции считаются отдельно |
| Auth generation recheck | 1 | Shared exchange у authority, не N exchanges на N waiters |
| Retry-start window | 10 с от первого safe failure | Не generation timeout |
| Accumulated queue wait | 30 с | С учётом client deadline и реально actionable событий |
| Busy preference wait | 0 | Не удерживать request перед доступным разрешённым резервом |
| Transient source backoff | 250–750 мс | Не общий sleep перед B |
| Failure detector | 3 разных RequestId одного scope за 60 с без подтверждённого восстановления | Concurrent failures должны учитываться |
| Open backoff | 2 с, x2 до 60 с | Provider not-before не сокращается; jitter после обязательной границы |
| Recovery | 1 одновременно/runtime, K=20 healthy completions на credit | Один initial credit; при отсутствии ready альтернатив отдельный due trial |
| Runtime retry limiter | 10/s, burst 10 | Дополнительные попытки, не первичные healthy admissions |
| Fault-domain retries | 1/s, burst 1 | Только доказанный общий domain |
| Refresh | Таблица §12.3, bounded management workers | Отдельно от inference leases |

Для capacity, waiter count, retained bytes, parser buffers, observations и job queue
обязателен конечный runtime cap. Не задаём их «по ощущениям» в production: fixtures
проверяют как малые значения, так и целевой load. Один живой lease нельзя вытеснить
из памяти как обычную cache запись; при достижении cap срабатывает backpressure.

Три попытки не гарантируют обход всех десяти sources. Непроверенные sources не
помечаются failed; следующий request видит сохранённые scopes и готовые alternatives.
Увеличение числа aliases не увеличивает budget. Изменение пользовательского лимита
проходит validation/versioned policy, не скрытый clamp в helper.

## 14. Persistence, restart и hot changes

### 14.1. Какие данные действительно критичны

| Данные | Хранение/восстановление |
| --- | --- |
| Credential/login revision, access policy, разрешение расходов | Существующий secure store/vault и versioned config; при недостоверности затронутый scope закрыт |
| Owner binding для opaque continuation | Сохранять только разрешённые redacted metadata; потеря отклоняет эту continuation, не все новые запросы |
| Подтверждённый provider quota/rate block | Versioned scope + evidence + not-before; restore проверяет применимость и планирует bounded reconciliation |
| Circuit streak, local load, ranking history | Восстанавливаемое runtime-состояние; нельзя оживлять старые leases или превращать cache corruption в бессрочную неисправность |
| Inventory, stats, reference metadata/prices | Last-known-good либо unknown с предупреждением/refresh, не ложный ноль |
| Uncertain sync attempt | Bounded diagnostics, не вечный remote capacity lock |
| Реальная remote job | Adapter-specific durable reconcile, если такой contract поддержан |

Restart не способ обхода known restrictions, но и не восстановление вчерашнего
cooldown навсегда. Expired observation приводит к проверке/контролируемому trial,
а не к заявлению «провайдер здоров». Старые process leases не живые.

При storage error запрещены ложные «сохранено». Config/credential change не публикуется
как durable до записи. Новый block сразу действует в памяти; сохранение и ошибка
записи наблюдаемы. Для повреждённого блока, scope которого известен, reconciliation
касается этого scope. Runtime-wide fail-closed нужен при повреждении vault/access
policy или невозможности установить границы безопасности, не из-за stats cache.
Не обещаем zero-dispatch-after-crash, если запрет не был durable; требующая этого
adapter policy должна иметь отдельный transactional contract.

SQLite migrations append-only. Не сохраняем prompts, raw provider bodies, tokens
в runtime tables, tool arguments или незашифрованную историю. Большой универсальный
WAL для всех network outcomes не является условием приёмки ротации.

### 14.2. Горячие изменения

Saved, applied и durable — разные статусы. Order/weight не сбрасывают auth/quota,
leases, owners, retry или failure incident. Lower capacity не убивает in-flight.
Revoke/disable запрещает новые sends, включая retry уже ожидающего context;
прервать активный stream — отдельное явно определённое security-действие.

Endpoint/identity replacement инвалидирует только относящиеся к нему observations.
Проверяются одновременно local и server пути конфигурации; новый snapshot не
перезапускает listener ради смены веса. Нельзя resurrect удалённый member stale CAS.

## 15. Что показывать пользователю и как искать реальную причину

У источника основная причина и подробности, а не один красный «сломан»:

- Готов; занят 2/2; временная ошибка, следующая попытка после …;
- ждёт квоту/лимит; reset не сообщён; требуется вход;
- баланс устарел/не поддерживается; обновление моделей не удалось, сохранён cache;
- запрос отменён, удалённый результат неизвестен — **не статус вечной блокировки**.

Для request показывается причина выбора B, ожидания или отказа. Retry exhaustion
не маркирует неиспользованный C неисправным. `Retry-After` для ответа клиенту
соответствует достижимому разрешённому scope, не минимальному таймеру запрещённого
source. Permanent input/access error не оформляется как локальный временный 429.

Redacted diagnostics: opaque request/attempt IDs, counters wire/work, lease/revision,
operation, классификация, exclusion reason, queue/wire/terminal timings, evidence
и commit boundary. Метрики используют bounded labels, не произвольный request ID.
Никаких prompts, tool args, account identities, auth headers, cookies или URL query
с секретами. Event grouping измеряется отдельно от dispatch.

Сравниваются terminal success, pool-added delay, retry amplification, source
recovery latency, false exclusions, refresh rate, lease leaks, stale applications,
unknown vs zero usage. Само число успешных unit-тестов не показатель стабильности.

## 16. Что взять у других реализаций — и чего не копировать

Проверяем конкретный код, не репутацию сервиса. Стабильность частной установки и
поведение её скрытых upstream accounts по GitHub не устанавливаются.

| Первичный источник | Полезный механизм | Ограничение переноса |
| --- | --- | --- |
| Sub2API `failover_loop.go` | Разные причины same-account retry, переключений и request-scoped ошибок | Вложенные retry layers и его лимиты не становятся безопасным единым бюджетом Relay автоматически |
| Sub2API `concurrency_service.go`, `openai_account_scheduler.go` | Account/user slots, явный release, ownership/sticky и load-aware выбор | Redis/service-level leases не доказывают окончание remote generation; не переносим внутреннюю коммерческую логику |
| CLIProxyAPI `selector.go` | Разделение fill-first, RR и weighted selection; работа с изменяющимся subset | Стратегия выбора не заменяет retry safety и quota contract |
| CLIProxyAPI `conductor_refresh.go` | Single owner lifecycle и no-progress backoff | Refresh cadence/token semantics должны принадлежать конкретному adapter |
| Envoy circuit breaking/outlier detection | Ограничение retry amplification, отдельных ресурсов и различение origin failures | Нельзя копировать panic bypass для auth/quota; admission circuit breaking и health outlier detection — разные механизмы |
| RFC 9110 §9.2.2 | Различение idempotent semantics, прикладного знания клиента и запрета generic proxy retry non-idempotent requests | HTTP status/отсутствие текста не разрешают повтор POST; прикладной controller требует отдельного contract |

Из сравнения не следуют ни Redis для личного desktop, ни нужда переписать secure store,
ни универсальные цифры cooldown, ни exactly-once. LiteLLM может быть дополнительным
материалом для будущих detector policies, но его error thresholds не являются
основанием для чисел этой редакции.

## 17. Что пересмотреть в уже начатом коде до следующего внедрения

Это карта границ и рисков, не отчёт о готовности ротации. Доказательством исправления
каждого пункта должен стать regression test на реальном controller path.

| Участок | Требуемый пересмотр |
| --- | --- |
| `scheduler/rotation.rs`: `RequestBudget::retry_decision` | Заменить неоднозначное `Accepted + replayable` четырьмя независимыми доказательствами §7; одной сериализуемости недостаточно |
| Там же: `RotationCandidate`/`RotationRequest` | Точные model-route-operation связи, физические capacity/bucket scopes; не один scalar quota на весь member |
| Там же: `select`, `health_class` | Явный recovery arbitration: healthy peer не должен навсегда исключать due trial; primary не должен подавлять разрешённый healthy reserve |
| Там же: `update_circuit` | Incident/failure accounting без потери одновременных failures при epoch increment; late success не снимает более новый block |
| Там же: `RequestBudget::for_incoming_request` | Убрать неявное превращение configured limit >3 в 3; выбранный budget принадлежит утверждённой policy и миграции |
| `runtime.rs`, `gateway/execution*`, `images.rs`, `websocket*` | Карта всех dispatch paths; общий счётчик не означает, что старый PoolScheduler/AutomaticRecovery уже заменены |
| `runtime/authorization.rs` | Auth replay должен иметь typed pre-execution rejection и общий context; authority не получает независимый inference retry loop |
| `scheduler/refresh.rs` и host workers | Capability/due/freshness/limits контракт, затем единственный владелец jobs; standalone coordinator сам по себе не заменяет host lifecycle |

Переписать API этих заготовок допустимо. Полезные budget/lease guards и корректные
regression tests сохраняются по смыслу, а не по прежнему имени файла.
Не удалять чужие параллельные изменения и не откатывать whole files ради чистого diff.

### 17.1. Обязательная миграция настроек

Обновление до 1.1.3 автоматически переводит сохранённые настройки до создания
runtime/listener. Подтверждение, отдельное уведомление и ручной stop/start не нужны.

- Smart → Automatic; InOrder/RoundRobin сохраняют режим и порядок;
- веса, concurrency, membership, enabled, retry limits и persistent waiting сохраняются;
- source roles задают начальный порядок, не скрытый барьер между типами;
- source delays сохраняют обязательные provider pauses, не заменяют circuit pacing;
- старые threshold/keep-last/score поля читаются для совместимости, не исполняются;
- поддержанные старые presets преобразуются после remap IDs без расширения прав;
- unsupported server schema/capability — отказ записи, не silent downgrade;
- повторное открытие уже преобразованного профиля ничего не меняет;
- некорректная/неизвестная версия не становится разрешением сбросить настройки.

Никаких двух working schedulers одновременно. Остаётся небольшой reader старого
формата; rollback к старому engine и downgrade БД не являются функциями приложения.

## 18. Новый порядок реализации: вертикальные этапы вместо большого скачка

Этапы B–E сначала исполняются в изолированном synthetic runtime. Частичная готовность
одного driver не означает готовность всей ротации для обычного пользователя. В пути нового controller
старый retry/recovery owner отключён: нельзя снова получить новый budget поверх
старых независимых loops. До этапа F production-style switch не выполняется.

### A. Контракты, причина проблемы, минимальный воспроизводимый стенд

Карта **реальных** HTTP/API/account/SSE/WS/images/compaction/continuation sends,
безопасная трасса исходного инцидента при отдельном разрешении,
fake adapters, clock, storage и seeded jitter. Зафиксировать вышеуказанные спорные
сценарии как падающие regression tests. Проверить изменения из §2 и сохранность старых настроек.

Выход: ясно, где есть повтор, где только UI events, и какие guarantees даёт adapter.
Не объявлять quota/account live contracts доказанными по локальным mocks.

### B. Одна операция end-to-end

Один stateless HTTP путь: validate → select → prepare → reserve → send → terminal.
Приоритеты — один context, корректный cancel, no replay unknown, быстрый безопасный
переход A → B, без fixed global pause. Никаких новых production defaults из test module.

Выход: тесты на конкретные sends и downstream commit, не только вызовы чистого ядра.

### C. Перенос остальных drivers и ownership

SSE, JSON, native WS/HTTP bridge, images, compaction, tool/continuation пути подключаются
по одному к тому же controller contract. Сначала audit nested retries, потом их
замена. Длинная генерация, refresh credentials и новые turns тестируются отдельно.

Выход: каждая operation в матрице §19; поддержанные несовпадающие контракты явны.

### D. Реальный Router/Resource registry/health reducer

Заменить старый selector/recovery state на одном lifecycle owner. Подключить общий
recovery budget, fair queue, known shared scopes и update/restart semantics.
Не оставить новый counter поверх старой независимой recovery-машины.

Выход: concurrency/fault tests на actual runtime, различия desktop/server отсутствуют.

### E. Source refresh service и user-facing состояние

Присоединить существующий authority; перевести quota, models, stats workers с
контролируемым отключением прежнего владельца. Reference loaders не переписывать
без отдельной причины. Добавить fresh/stale/unsupported и реальные reasons в UI.

Выход: несколько screens/manual jobs не множат requests, dashboard outage не валит
inference, каждая job bounded и revision-safe. Отдельные host integration tests.

### F. Автоматический переход и удаление legacy

Переход — часть загрузки настроек новой версии, до runtime и listener. Он не
прерывает живые streams, не меняет gateway enabled и не запускает второй engine.
Shadow допустим только как read-only decision, без reserve/dispatch/provider reads.

Удалить legacy/unified branches, старые polling/retry owners, независимый
AutomaticRecovery, alias-dependent limits и obsolete UI после replacement tests.
Не удалять reader сохранённых данных и не обещать downgrade старого executable.

## 19. Приёмка: сначала сценарии, потом «готово»

### 19.1. Обязательные точечные сценарии

| ID | Сценарий | Требуемый результат |
| --- | --- | --- |
| R01 | A not-sent failure, B ready | B без artificial sleep A; общий context/budget |
| R02 | Accepted/unknown, repeatable body, downstream ещё пуст | Повтор запрещён без отдельно доказанного deduplication contract |
| R03 | Проверенный 401, auth replay, fallback | Один bounded budget, не N независимых loops |
| R04 | WS handshake отказал до payload → HTTP | Тот же context; wire/work учтены правильно |
| R05 | WS payload мог уйти, socket оборвался | Нет прозрачной второй generation |
| R06 | Любой headers/heartbeat/metadata/tool/reasoning/text commit | Failover запрещён во всех drivers |
| R07 | Три запроса стартовали до первого failure и все упали | Три независимых голоса, не один |
| R08 | Новая ошибка → старый concurrent success | Старый success не закрывает новый incident/block |
| R09 | A due recovery, B healthy, постоянный compatible спрос | A получает bounded шанс, а не 0 за любое число requests |
| R10 | Много bad sources + healthy B | Recovery не поглощает поток; проверяется runtime share bound |
| R11 | Нет ready источников, один due | Demand-driven trial возможен без background generation |
| R12 | Cancel после send у sync source с capacity=1 | Нет retry этого request; следующий независимый request не заблокирован навсегда |
| R13 | Remote job adapter: cancel без подтверждения | Его remote permit сохраняется до contract reconcile, local lease освобождён |
| R14 | Sources busy, reserve разрешён, wait=0 | Доступный reserve без принудительных 30 с |
| R15 | Reserve/overage запрещён | Никакой обход ограничений из-за веса/ошибки primary |
| R16 | Hard owner недоступен, replay неполный | Ясный отказ/ограниченное ожидание; ничего не вырезается из истории |
| R17 | 100 requests требуют auth refresh | Single exchange/join; newer login не перезаписывается |
| R18 | Exchange ответ потерян/запись credential не удалась | Нет blind rotating-token replay и ложного durable success |
| R19 | 429 → stale quota positive, включая cached read после 429 | Block не снимается недостаточным evidence |
| R20 | Reset неизвестен или dashboard недоступен | Paced contract recovery, не вечный ban/не poll storm |
| R21 | Partial models, 304 другой identity, malformed/empty response | LKG/validation; пустой валидный результат отличим от ошибки |
| R22 | Stats 403/timeout/unsupported | Не zero balance и не inference outage |
| R23 | UI refresh + dirty во время job | Coalesce + максимум один follow-up, Retry-After соблюдается |
| R24 | Job success, но credential всё ещё непригоден | No-progress guard, нет tight loop |
| R25 | Sleep/resume, clock jump, restart | Нет лавины jobs/leases и бессрочных stale cooldown |
| R26 | 20 минут нет текста без заданного deadline | Нет скрытого cancel/retry/health penalty |
| R27 | Duplicate alias, API/account operation lanes | Общая physical capacity и budget, не двойные permits |
| R28 | Удаление/disable/weight change во время requests | Новые sends fenced; старый release не трогает новый lease |
| R29 | Queue saturation/slow client | Bounded memory/fairness; provider health не портится |
| R30 | Configured лимит, preset/старый server, migration rollback | Нет silent clamp, permission widening или потери настроек |
| R31 | Один trace на desktop/server | Одинаковые selection/retry/recovery decisions |
| R32 | Один dispatch и несколько UI tool events | Не объявлять это rotation bug без иной evidence |
| R33 | Fallback требует другой model/tier/tools или урезанного контекста | Не менять запрос молча; несовместимая route исключена |

### 19.2. Transport/operation matrix

Для API и account adapters отдельно проверяются JSON, SSE, native WS и WS→HTTP,
images/multipart, compaction, tools и continuations: success; rejection; connect
failure; unknown after send; cancellation; commit; token change; slow downstream.
Unsupported клетка матрицы обозначается явно, не заполняется заглушкой «прошло».

Fake clock и concurrency barriers проверяют перестановки reserve/disable/send,
release/remove/readd, failure/success/refresh и dirty/completion. Assertions считают
реальные sends на synthetic upstream, а не только `AttemptId` из helper.

### 19.3. Нагрузочная и разрешённая live-приёмка

Сравнить baseline и новый вариант на одном workload: короткие/длинные requests,
малый pool и burst, один unstable provider, все providers unstable, общий proxy,
постоянные cancel, недоступный dashboard, много accounts и startup/resume.

Безусловные gates: нет post-commit replay, скрытого выхода за budget, lease leak,
permission bypass, stale credential overwrite, бесконечного polling и starvation
при достаточном совместимом спросе. При healthy workload нет неоправданной
регрессии pool-added latency и refresh traffic. Конкретные performance thresholds
фиксируются до release на целевой конфигурации, не придумываются после результата.

Live tests только на разрешённых пользователем источниках и с явным допустимым
расходом. Они проверяют реальные adapter contracts, не заменяются mocks.
Contributor commands остаются в [CONTRIBUTING.md](../../CONTRIBUTING.md).

## 20. Какие решения ещё требуют утверждения

Редакция предлагает, но не объявляет уже согласованными:

1. Новый смысл автоматического режима без смешанного quota/money/latency score.
2. `busy_preference_wait=0` и явное разделение membership, reserve и overage.
3. Local concurrency semantics для sync API после unknown cancel.
4. Общий recovery share budget и его fixture параметры.
5. Migration старых настроек, request limits и долгого ожидания.
6. Adapter capability matrix, обязательный runtime cap profile и live gates.

Недоказанный adapter contract блокирует соответствующую возможность, не всю разработку.
Нельзя обещать универсальную стабильность, remote exactly-once, полный контроль
provider spending или исправление исходного визуального бага до их проверки.

Итог для пользователя: **работающий источник продолжает работать; сбой изолирован;
переключение быстрое только когда безопасно; восстановление автоматическое, но
ограниченное; обновление данных не выключает генерацию; отмена не убивает пул.**

## 21. Проверяемые источники

### Локальные границы

- [Действующая архитектура](PLANNING.md) и [приёмка](ROADMAP.md).
- [Pool policy](../../crates/relay-core/src/scheduler/policy.rs),
  [scheduler adapter](../../crates/relay-core/src/scheduler/selection.rs),
  [recovery](../../crates/relay-core/src/scheduler/selection/recovery.rs).
- [Ядро ротации](../../crates/relay-core/src/scheduler/rotation.rs),
  [refresh coordinator](../../crates/relay-core/src/scheduler/refresh.rs).
- [Runtime](../../crates/relay-core/src/runtime.rs),
  [authorization](../../crates/relay-core/src/runtime/authorization.rs),
  [execution](../../crates/relay-core/src/gateway/execution.rs),
  [images](../../crates/relay-core/src/gateway/images.rs),
  [WebSocket](../../crates/relay-core/src/gateway/websocket.rs).
- [Token authority](../../crates/relay-core/src/accounts/token_authority.rs),
  [desktop jobs](../../src-tauri/src/local_pool/refresh.rs),
  [server jobs](../../relay-server/src/jobs/refresh.rs).
- [Account tests](../../crates/relay-core/tests/accounts.rs),
  [scheduler tests](../../crates/relay-core/tests/scheduler.rs),
  [длинные запросы](../../crates/relay-core/tests/support/long_requests.rs),
  [client compatibility](../../crates/relay-core/tests/support/client_compatibility.rs).

### Внешние первичные источники

Сравнительный код закреплён ревизиями, проверенными 23 сентября 2026 года.
Он подтверждает отдельные механизмы §16, не свойства частного provider deployment.

- Sub2API, commit `a3eb7ef302961cba716dc78b39b93b60c467db0e`:
  `https://github.com/Wei-Shaw/sub2api/blob/a3eb7ef302961cba716dc78b39b93b60c467db0e/backend/internal/handler/failover_loop.go`
  `https://github.com/Wei-Shaw/sub2api/blob/a3eb7ef302961cba716dc78b39b93b60c467db0e/backend/internal/service/concurrency_service.go`
  `https://github.com/Wei-Shaw/sub2api/blob/a3eb7ef302961cba716dc78b39b93b60c467db0e/backend/internal/service/openai_account_scheduler.go`
- CLIProxyAPI, commit `673131f57484517c3a1eae7e36c4cfa7b9bb4efc`:
  `https://github.com/router-for-me/CLIProxyAPI/blob/673131f57484517c3a1eae7e36c4cfa7b9bb4efc/sdk/cliproxy/auth/selector.go`
  `https://github.com/router-for-me/CLIProxyAPI/blob/673131f57484517c3a1eae7e36c4cfa7b9bb4efc/sdk/cliproxy/auth/conductor_refresh.go`
- Envoy, circuit breaking и outlier detection:
  `https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/circuit_breaking`
  `https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier`
- RFC 9110, §9.2.2 (idempotent methods):
  `https://www.rfc-editor.org/rfc/rfc9110.html#section-9.2.2`
  Копия HTTP Working Group: `https://httpwg.org/specs/rfc9110.html#idempotent.methods`

Частоты polling, thresholds, выбор режимов и миграция здесь являются решениями
проекта Relay. Внешние ссылки не делают их автоматически правильными.
