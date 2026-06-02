# DB-слой — `apps/desktop/src-tauri/src/db`

> Подсистема Ф0-2. **ADR-003** (rusqlite + write-actor), **ADR-004** (SQLite — источник истины
> графа/беклинков). Критерии: `AC-Б7-1`, `AC-Б7-2`, `AC-PR-3`.

## Назначение
Единая точка доступа к `nexus.db` (внутри `<vault>/.nexus/`): метаданные файлов, ссылки/беклинки,
теги, алиасы, настройки. Записи сериализованы (единственный писатель), чтения масштабируются (WAL).

## Компоненты
- **`Database::open(path)`** — создаёт `.nexus/`, открывает write-коннект, включает WAL+pragmas,
  применяет миграции, поднимает write-actor и read-пул. К запросам готова **только после** миграций.
  Аксессоры: `writer()`, `reader()`, `schema_version()`.
- **`WriteActor`** (`write_actor.rs`) — единственный поток-писатель (`nexus-db-writer`):
  - `call(f)` — операция на `&mut Connection` без авто-транзакции;
  - `transaction(f)` — `f` в одной синхронной транзакции (commit при `Ok`, rollback при `Err`/панике).
  Клонируется (общий `mpsc`-канал) → раздаётся индексатору и Tauri-командам.
- **`ReadPool`** (`read_pool.rs`) — N read-коннектов (по умолчанию 4): семафор + `spawn_blocking`;
  `query(f)` выполняет `f` на `&Connection` (read-only, `query_only=ON`).
- **`migrations.rs`** — упорядоченный `MIGRATIONS` (SQL через `include_str!`), `apply()`.

## Инварианты
- **Единственный писатель.** Все мутации — через `WriteActor` → исключён `SQLITE_BUSY` между
  писателями (AC-Б7-1) и гонка двух write-транзакций.
- **Атомарность на файл.** `transaction()` — всё-или-ничего; частичного состояния не бывает (AC-Б7-2).
- **Версия схемы — `PRAGMA user_version`** (не `settings('schema.version')`): транзакционна,
  без chicken-egg с таблицей `settings`, без гонок. Миграция и поднятие версии — в одном коммите
  → идемпотентно и резюмируемо после краха (AC-PR-3).
- **WAL персистентен** в файле БД; read-коннекты наследуют его и читают параллельно с записью.
- **Pragmas:** writer — `WAL, foreign_keys=ON, busy_timeout=5000, synchronous=NORMAL`;
  reader — те же + `query_only=ON` (defense-in-depth).
- **Инвариант пула:** число свободных permit'ов семафора == числу коннектов в пуле в момент,
  когда permit можно получить; коннект возвращается в пул до освобождения permit'а.

## Схема v1 (`migrations/001_initial.sql`)
`files, links, tags, file_tags, aliases, settings` + индексы (`idx_links_source`, `idx_links_target`,
`idx_file_tags_file`, `idx_files_updated`). Источник — ARCHITECTURE §5.

**v2** (`002_chunks_fts.sql`, Ф1-1): `chunks` (+`idx_chunks_file`) + `fts_chunks` (FTS5
external-content поверх `chunks.content`) + триггеры синхронизации `chunks_ai/ad/au`. FTS5
доступен в bundled SQLite. `usearch` (векторный ANN) — sibling-файл, не в SQLite (Ф1-4);
`chat_* / link_suggestions` — отдельными миграциями позже (FTS5/usearch нельзя `ALTER` →
пересоздание + переиндексация из контент-таблиц).

## Как тестируется
`mod.rs` `#[cfg(test)]`, на temp-файле (реальный WAL, не in-memory):
- `migrations_apply_and_are_idempotent` → **AC-PR-3**
- `transaction_is_atomic_on_error` → **AC-Б7-2**
- `concurrent_writes_no_busy` (64 параллельных INSERT через actor) → **AC-Б7-1**
- `concurrent_reads_during_writes` (чтения параллельно с записью, WAL)

## Дальше
- Открытие/миграции пока inline в async-fn (одноразово на старте). Тяжёлые фоновые миграции
  (Ф1: переиндексация usearch при смене эмбеддера, §6.5) — увести в `spawn_blocking` с прогрессом.
- Wiring в Tauri-state и команды vault — Ф0-3 (после выбора vault).
