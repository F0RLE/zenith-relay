<div align="center">
  <img src="src-tauri/icons/128x128.png" width="112" alt="Zenith Relay">
  <h1>Zenith Relay</h1>
  <p>One private, OpenAI-compatible endpoint for your accounts and compatible API sources.</p>
  <p><a href="#english">English</a> | <a href="#русский">Русский</a></p>
</div>

<a id="english"></a>

## English

Zenith Relay is a local-first desktop application for managing your own
ChatGPT accounts and compatible API sources. It presents the available,
allowed models through one OpenAI-compatible endpoint, records safe usage
metadata, and can attach that endpoint to a supported client profile with a
reversible backup.

### Choose where it runs

| Mode | What it does | Use it when |
| --- | --- | --- |
| **Choose API** | Connects a compatible hosted API directly. No local account pool is started. | You already have an API key. |
| **Computer** | Runs a private account and API-source pool on this computer. The app must remain open. | You want your own accounts and sources behind one local endpoint. |
| **On your server** | Manages the same personal pool on a server you operate. The server continues after the desktop app closes. | You need a persistent endpoint for your own devices. |

### What is in the app

- **Connections** stores compatible API sources, current ChatGPT sign-ins or
  imported sessions, optional proxies, and quota automations.
- **Pool** controls membership, drain state, order, source fallback, and
  model rules. A candidate must be enabled, allowed, healthy, and able to
  serve the requested model before it can receive a request.
- **API & ChatGPT** exposes the endpoint, creates scoped client keys, and
  attaches it to a supported profile without overwriting an unrelated setup.
- **Usage** shows requests, model, selected pool member, token breakdown,
  time to first output, speed, result, and API-equivalent estimate. Prompt and
  response bodies are not stored in ordinary usage history.
- **Recovery** keeps encrypted profile snapshots and restores a selected
  snapshot or removes Relay-managed configuration safely.

Quota windows come from the provider response. Relay does not assume that
every account has a fixed five-hour or weekly limit. Monitoring and routing are
separate: an enabled account may be checked even when it is not currently
routable, while routing requires all of its pool and credential conditions.

### Screenshots

<p align="center">
  <img src="docs/screenshots/overview.png" width="49%" alt="Overview">
  <img src="docs/screenshots/connections.png" width="49%" alt="Connections">
</p>
<p align="center">
  <img src="docs/screenshots/pool.png" width="49%" alt="Pool">
  <img src="docs/screenshots/usage.png" width="49%" alt="Usage">
</p>

### Privacy and safety

Desktop secrets use the operating-system credential store. A self-hosted
server keeps secrets in its encrypted vault. A server management token and a
pool request key are different credentials and are never interchangeable.
Relay redacts secrets from normal UI state, diagnostics, and usage records.

The self-hosted server is a personal deployment, not a public Zenith account
inventory or customer billing service. Keep the server behind HTTPS, retain the
vault key separately from its data directory, and test a restore before relying
on a backup.

### Guides

| Mode | English | Russian |
| --- | --- | --- |
| Computer | [Guide](docs/help/en/this-computer.md) | [Инструкция](docs/help/ru/this-computer.md) |
| Choose API | [Guide](docs/help/en/zenith-api.md) | [Инструкция](docs/help/ru/zenith-api.md) |
| On your server | [Guide](docs/help/en/my-server.md) | [Инструкция](docs/help/ru/my-server.md) |

### Current scope

The released account connector is ChatGPT OAuth and compatible session import.
Compatible API sources are supported independently. Additional account systems
and client-profile integrations are planned only after their authentication,
terms, quota semantics, and safe recovery path are understood; see
[ROADMAP.md](ROADMAP.md).

Relay returns only models that the active pool and the selected client key
allow. The next model-catalog work will make Relay-provided models visibly
separate from a client's native models instead of maintaining a hard-coded
client list.

### Development

~~~powershell
cd src
bun install
bun run verify
bun run test:e2e
bun run screenshots
~~~

The desktop bundle is built with <code>bun run app:build</code> from
<code>src</code>. The user-managed server is the
<code>relay-server</code> crate; start its release binary only with its
HTTPS origin, management token, vault key, and data directory configured.
The current architecture and operating rules are in
[PLANNING.md](PLANNING.md); contribution and verification rules are in
[CONTRIBUTING.md](CONTRIBUTING.md).

<a id="русский"></a>

## Русский

Zenith Relay - локальное desktop-приложение для управления собственными
учётными записями ChatGPT и совместимыми API. Оно отдаёт разрешённые и реально
доступные модели через один OpenAI-совместимый адрес, сохраняет безопасную
статистику использования и может подключить этот адрес к поддерживаемому
профилю клиента с обратимой резервной копией.

### Где работает Relay

| Режим | Что делает | Когда выбирать |
| --- | --- | --- |
| **Выбор API** | Подключает готовый совместимый API напрямую. Локальный пул не запускается. | Уже есть API-ключ. |
| **Компьютер** | Запускает приватный пул аккаунтов и API-источников на этом компьютере. Приложение должно быть открыто. | Нужен единый локальный адрес для своих аккаунтов и источников. |
| **На своём сервере** | Управляет тем же личным пулом на вашем сервере. Сервер работает после закрытия desktop-приложения. | Нужен постоянный адрес для своих устройств. |

### Что умеет приложение

- **Подключения**: совместимые API, текущие входы ChatGPT, импортированные
  сессии, необязательные прокси и автоматизации квот.
- **Пул**: состав участников, дренирование, порядок, fallback API-источников
  и правила моделей. Запрос получит только включённый, разрешённый, здоровый
  участник, который реально умеет обслужить выбранную модель.
- **API и ChatGPT**: адрес, ключи для клиентов и обратимое подключение
  поддерживаемого профиля.
- **Использование**: запросы, модель, выбранный участник, токены, время до
  первого вывода, скорость, результат и API-эквивалент. Тексты промптов и
  ответов в обычной истории не хранятся.
- **Восстановление**: зашифрованные снимки профиля и безопасное возвращение
  выбранного снимка или удаление настроек, которыми управляет Relay.

Окна квоты берутся из ответа провайдера. Relay не считает, что у каждого
аккаунта обязательно есть одинаковые лимиты на пять часов или неделю.
Наблюдение за квотой и маршрутизация разделены: включённый аккаунт можно
проверять вне пула, но маршрутизация требует всех условий пула и доступных
учётных данных.

### Конфиденциальность и безопасность

На компьютере секреты находятся в системном хранилище учётных данных. На
своём сервере они лежат в зашифрованном vault. Управляющий токен сервера и
ключ запросов пула - разные учётные данные и не взаимозаменяемы. Relay не
выводит секреты в обычном состоянии интерфейса, диагностике и истории
использования.

Серверный режим предназначен для личного развёртывания, а не для публичного
хранилища аккаунтов Zenith или клиентского биллинга. Используйте HTTPS,
храните ключ vault отдельно от каталога данных и заранее проверяйте
восстановление резервной копии.

### Справка и развитие

Инструкции для каждого режима доступны в таблице выше и внутри раздела
«Помощь» приложения. Сейчас поддержан вход ChatGPT через OAuth и совместимый
импорт сессии, а также независимые совместимые API-источники.

Будущая поддержка других подписок, например Kiro или Antigravity, и
подключение других программ будет добавляться только при наличии безопасного
официального пути входа, понятной семантики квот и обратимого восстановления.
План разделённого каталога моделей, где модели Relay отдельно видны в Codex и
содержат только доступные модели пула, описан в
[PLANNING.md](PLANNING.md) и [ROADMAP.md](ROADMAP.md).

### Лицензия

Copyright (C) 2026 FORLE. Лицензия:
[GNU Affero General Public License v3.0 only](LICENSE)
(<code>AGPL-3.0-only</code>).
