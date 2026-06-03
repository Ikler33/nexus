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

## Брокер, host-сторона (Ф2-2a, `broker.rs`) — §7.4
`PluginBroker { sessions: HashMap<PortId, PluginSession>, audit: AuditLog }`:
- **identity по порту** (`register(port, session)`): права берутся из сессии ПОРТА, а не из payload —
  закрывает confused-deputy/capability-laundering (плагин A не может назваться B).
- `authorize(port, req)`: порт→сессия (нет → `UnknownSession`, fail-closed) → `Permissions::check` →
  запись в **audit** (и успех, и отказ). `handle(port, req, &mut dyn HostDispatch)` = authorize → dispatch.
- **`AuditLog`** — только добавление (неотключаемый, §7.9); `revoke(port)` мгновенно лишает прав (ревокация).
- Реальный I/O (vault/ai) — за трейтом `HostDispatch` (Ф2-2b: через `vault::resolve_vault_path` + db/ai).
> Capability-токены, MessagePort/iframe-транспорт, генерация секретов — Ф2-2b (нужна фронт-сторона).

## Тесты
- Лоадер: совместимый грузится; `TooNew`/`TooOld`/`BadVersion`/`Parse`; `scan` различает состояния.
- Права (13): glob (`**`/`*`/exact/регистр), scope с deny-override (любой порядок), vault read/write,
  path-escape (`..`,`/abs`,`\`,пустой сегмент), ai+local_only, `ai:complete:false`, net-allowlist,
  unknown-method fail-closed, пустые права = deny-all.
- Брокер (6): неизвестный порт → deny+audit, scope allow+audit, out-of-scope deny+audit,
  **identity-по-порту** (confused-deputy: узкий плагин не дотянется до прав широкого), ревокация, handle→dispatch.

## Дальше (Ф2-2b+)
- **Транспорт + токены (фронт):** доверенный JS в Worker + редакторные расширения в main-контексте;
  UI-вью в sandbox-iframe; один `MessagePort` на плагин (identity по порту); capability-токены (генерация
  секретов + проверка на каждый вызов + ревокация); реальный `HostDispatch` (vault/ai через
  `resolve_vault_path`). `registerCommand(source:'plugin')`, плагинные i18n-namespace (Ф2-3).
- Подпись `id@version#sha256`, marketplace; опц. WASM (epoch/fuel + StoreLimits). Код плагинов НЕ в git.
