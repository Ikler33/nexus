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

## Тесты
- Лоадер: совместимый грузится; `TooNew`/`TooOld`/`BadVersion`/`Parse`; `scan` различает состояния.
- Права (13): glob (`**`/`*`/exact/регистр), scope с deny-override (любой порядок), vault read/write,
  path-escape (`..`,`/abs`,`\`,пустой сегмент), ai+local_only, `ai:complete:false`, net-allowlist,
  unknown-method fail-closed, пустые права = deny-all.

## Дальше (Ф2-2+)
- Рантайм-брокер: сессии по `MessagePort` (identity по порту), capability-токены + ревокация, audit-log,
  `dispatch` к vault/ai через `resolve_vault_path`. Исполнение: доверенный JS в Worker + редакторные
  расширения в main-контексте; опц. WASM (epoch/fuel + StoreLimits). `registerCommand(source:'plugin')`,
  плагинные i18n-namespace. Подпись `id@version#sha256`, marketplace. Код плагинов НЕ в git (ADR-002).
