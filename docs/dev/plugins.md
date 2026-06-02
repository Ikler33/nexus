# Plugin loader (минимум, Ф0-13) — `src-tauri/src/plugin`

> §7.2, **С-13**. Только чтение манифеста + совместимость версии API. Broker, исполнение
> (JS/WASM), path-scoped права, подпись, marketplace — Фаза 2 (§7, ADR-001/002).

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

## Тесты
Совместимый грузится; нужно новее ядра → `TooNew` (С-13); `max < ядро` → `TooOld`; в диапазоне → ok;
`^1.0` → `BadVersion`; битый json → `Parse`; `scan` различает compatible/incompatible/broken;
нет каталога → пусто.

## Дальше (Ф2)
- Capability-broker (граница прав), MessagePort-identity, path-scoped permissions, audit-log.
- Подпись `id@version#sha256`, реестр/marketplace, hot enable/disable, миграция настроек.
- Исполнение: доверенный JS в Worker + редакторные расширения в main-контексте; опц. WASM
  (epoch/fuel + StoreLimits). Код плагинов НЕ в git (ADR-002).
