# Plugin loader + модель прав — `src-tauri/src/plugin`

> Ф0-13 (§7.2, С-13): манифест + совместимость API. **Ф2-1 (ADR-002, §7.4/§7.9): scoped-права +
> `check_scoped_permission`** — security-ядро брокера. Рантайм-брокер (порты/токены/audit/iframe),
> исполнение (JS/WASM), подпись, marketplace — далее в Фазе 2.

## Версии API
- `ApiVersion { major, minor }`, `ApiVersion::parse("1.2")`; ядро — `CORE_API_VERSION` (1.0, Приложение B).
- **С-13**: `min_api_version` — это МИНИМУМ версии ядра. `"^1.0"` отвергается (`BadVersion`), а НЕ
  трактуется как «любой 1.x» (иначе плагин под фичи 1.2 падал бы на ядре 1.0).

## API
- `parse_manifest(json)` → `PluginManifest { id, name, version, min_api_version, max_api_version?, entry? }`.
- `check_compatibility(m, core)`: ядро ∈ `[min, max]`, иначе `TooNew` / `TooOld`.
- `load_manifest(json, core)` = parse + compat.
- `scan_plugins(dir)` → `Vec<PluginInfo { dir, id, name, version, compatible, error }>` по
  `.nexus/plugins/*/manifest.json` (НЕ исполняет код). Команда `list_plugins`.

## Модель прав (Ф2-1, `permission.rs`) — security-ядро брокера
Манифест несёт `permissions` (§7.2): `vault:read`/`vault:write` (path-glob со scoped-правами и `!`-deny),
`ai:embed` (bool), `ai:complete` (`true`/`{local_only}`), `net` (host-allowlist), `ui` (точки расширения).
Отсутствие ключа = право не выдано (**fail-closed**, deny-all по умолчанию).

`Permissions::check(ApiRequest{method,path?,host?}) -> Result<(), Denied>` (§7.4 `check_scoped_permission`):
- метод → требуемое право (`vault.readFile`→`vault:read`, `vault.writeFile`→`vault:write`, `ai.*`, `net.fetch`, `ui.*`);
- **path-scoped**: `path_in_scope` — совпал с allow-glob И ни с одним `!`-deny (**deny перекрывает allow**);
- **анти-traversal** (защита в глубину поверх `vault::resolve_vault_path`): `..`/`.`/абсолютный/`\`/пустой сегмент → `PathEscape`;
- `net` — только по allowlist; `ai:complete` несёт `local_only`; **неизвестный метод → `UnknownMethod`** (fail-closed).
- `glob_match`: сегментный glob — `**` (0..N сегментов), `*` (внутри сегмента, не пересекает `/`).
> Identity плагина и capability-токен проверяются РАНТАЙМОМ по порту (§7.9), не из payload (Ф2-2).

## Брокер, host-сторона (Ф2-2a/2-2b, `broker.rs`) — §7.4/§7.9
`PluginBroker { sessions: HashMap<CapToken, PluginSession>, audit: AuditLog }`:
- **identity по capability-токену** (`open_session(session) -> CapToken`): токен = 32 случайных байта
  в hex (`getrandom`, неугадываем, §7.9) — IPC-эквивалент порт-идентичности. Права берутся из сессии
  ТОКЕНА, а не из payload → закрывает confused-deputy/laundering (плагин A не предъявит токен B).
- `authorize(&token, req)`: токен→сессия (нет → `UnknownSession`, fail-closed) → `Permissions::check`
  → запись в **audit** (и успех, и отказ). `handle(&token, req, &mut dyn HostDispatch)` = authorize → dispatch.
- **`AuditLog`** — только добавление (неотключаемый); `revoke(&token)` мгновенно инвалидирует сессию.
- Реальный I/O (vault/ai) — за трейтом `HostDispatch` (через `vault::resolve_vault_path` + db/ai).
> На фронте каждому плагину — один `MessagePort` (§7.5); хост-релей привязывает к порту правильный
> токен и передаёт его в Tauri-вызов.

## Команды брокера (live, Ф2-2b·2)
- `plugin_open_session(dir)` → читает `.nexus/plugins/<dir>/manifest.json`, проверяет совместимость,
  заводит сессию с правами манифеста в `AppState.plugins` (брокер) → возвращает **capability-токен**.
- `plugin_invoke(token, method, path?, content?)` → брокер `authorize(token, req)` (scoped + audit) →
  `dispatch_vault` (вынесен отдельной тестируемой функцией). Методы: `vault.readFile`/`vault.listFiles`
  (право `vault:read`), `vault.writeFile` (`vault:write`, через `resolve_vault_path_for_write`).
  Результат — JSON: строка-контент / массив записей каталога / `{ok,bytes}`. Лок брокера держится только
  на синхронную авторизацию; async-I/O и резолв пути (та же граница, defense-in-depth) — после освобождения.
> Брокер в `AppState` (`std::Mutex<PluginBroker>`). End-to-end через эти команды проверяется фронтом (ниже).
- `plugin_close_session(token)` → `broker.revoke` (мгновенный отзыв токена при размонтировании плагина —
  иначе сессии копятся; идемпотентно).

## Фронт-транспорт (Ф2-2b·4, `lib/plugin-host.ts` + `components/plugins/PluginsPanel.tsx`) — §7.5
Плагин живёт в `<iframe sandbox="allow-scripts">` (opaque origin — нет доступа к родителю/storage/cookies)
и общается с хостом ТОЛЬКО через свой `MessagePort`.
- `attachPlugin(dir, hostPort)`: открывает сессию (токен **host-side**, плагину НЕ передаётся), привязывает
  токен к ПОРТУ, обслуживает запросы `{id,method,path?,content?}` через `tauriApi.plugins.invoke` и шлёт
  `{id,ok,result|error}`. **Токен — из привязки порта, не из payload** → даже подсунутый плагином `token`
  игнорируется (confused-deputy/laundering закрыт и на фронте, как identity-по-токену в Rust).
- `mountPlugin(dir, iframe)`: рукопожатие `nexus:ready` (iframe готов, повторяется) → `nexus:init` (порт
  через `transfer`) — без гонки «порт раньше подписки». `dispose()` закрывает порт + `closeSession` (отзыв).
- Контракт `tauriApi.plugins`: `list`/`openSession`/`invoke`/`closeSession` (вне Tauri — мок-брокер
  `lib/mock/plugins.ts`: токен→scope, glob с deny-override — зеркало Rust → превью показывает РЕАЛЬНУЮ
  границу прав, включая отказы).
- UI: `PluginsPanel` (демо-плагин «Hello Reader» в песочнице + лог брокерских вызовов ✓/✋), команда/кнопка
  `view.plugins`, i18n RU/EN. **Проверено в превью:** листинг + чтение через брокер, аудит фиксирует вызовы.
- **`registerCommand(source:'plugin')` (Ф2-3, двунаправленно):** плагин шлёт `ui.registerCommand {id,title}`
  → брокер авторизует (право **`ui:command`** в манифесте) → релей регистрирует команду (`plugin:<dir>:<id>`,
  `source:'plugin'`) в реестре (§4.6). Запуск из палитры → хост шлёт плагину событие `command` по порту
  (host→plugin) → плагин исполняет свой обработчик. `dispose()` снимает команды плагина. `plugin_invoke`
  для `ui.*` — только авторизация (host-I/O нет). **Проверено в превью:** команда плагина в палитре → запуск
  → плагин читает Inbox.md через брокер (аудит фиксирует `vault.readFile`).
- **Плагинные i18n (Ф2-3, AC-I18N-7):** `ui.addTranslations {локаль→{ключ→строка}}` → релей кладёт строки
  в i18next namespace `plugin` **вложенно** (`{<dir>:{<key>:value}}` → ключ `plugin:<dir>:<key>`; i18next
  режет ключ по `:` на ns + вложенный путь). `registerCommand` принимает `titleKey` → заголовок команды
  локализован и меняется при смене языка (палитра резолвит `titleKey` через `t()`). `ui.*` требует
  объявленной хотя бы одной ui-точки (fail-closed). **Проверено в превью:** EN↔RU меняет заголовок команды.

## Тесты
- Лоадер: совместимый грузится; `TooNew`/`TooOld`/`BadVersion`/`Parse`; `scan` различает состояния.
- Права (14): glob (`**`/`*`/exact/регистр), scope с deny-override (любой порядок), vault read/write,
  path-escape (`..`,`/abs`,`\`,пустой сегмент), ai+local_only, `ai:complete:false`, net-allowlist,
  unknown-method fail-closed, пустые права = deny-all, `ui.registerCommand` требует `ui:command`.
- Брокер (7): токены уникальны/неугадываемы (64 hex), неизвестный/отозванный токен → deny+audit,
  scope allow+audit, out-of-scope deny+audit, **identity-по-токену** (confused-deputy: узкий плагин не
  дотянется до прав широкого), ревокация, handle→dispatch.
- Dispatch (4, `commands/plugin.rs`): read/list/write в пределах vault; path-escape (read+write)
  отклонён; неизвестный метод / нет аргумента → ошибка; **E2E** «scope (broker) → dispatch I/O» + аудит.
- Фронт-транспорт (13, vitest): мок-брокер (scope/glob/revoke/unknown); `attachPlugin` — listFiles/read/
  write-в-scope/write-отказ, **confused-deputy** (payload-токен игнорируется), мусор → без ответа, dispose;
  **`ui.registerCommand`** (команда в реестре → запуск шлёт событие плагину → dispose снимает);
  **`ui.addTranslations`** (строки резолвятся в namespace `plugin`, `titleKey` формируется).

## Дальше (Ф2-3 + доводка транспорта)
- **Реальная загрузка кода плагина** из `.nexus/plugins/<id>/<entry>` (сейчас демо встроено в хост) +
  **iframe-CSP упакованного приложения** (`frame-src`/`child-src`, origin ассетов плагина). Доверенный JS
  в Worker + редакторные расширения в main-контексте (сейчас UI-JS прямо в iframe). См. BACKLOG.
- Расширить dispatch: ~~`vault.writeFile`/`listFiles`~~ (сделано) → `ai.embed/complete/searchSemantic`,
  `net.fetch` (allowlist). ~~`registerCommand(source:'plugin')`~~ (сделано), ~~плагинные
  i18n-namespace `plugin:<id>:<key>`~~ (сделано, AC-I18N-7). Осталось из Ф2-3: AI/сеть для плагинов.
- Подпись `id@version#sha256`, marketplace; опц. WASM (epoch/fuel + StoreLimits). Код плагинов НЕ в git.
