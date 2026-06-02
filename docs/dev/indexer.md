# Индексатор: parser + watcher + indexer (`src-tauri/src/{parser,watcher,indexer}`)

> Подсистема Ф0-4 (§4.2, Б9) + RAG-индексация Ф1-5 (§6.1). Критерии: **AC-Б9-1/2/3**, RAG —
> **AC-Б4-1/2 · AC-Б5-2 · AC-Б8-1/2 · AC-PERF-5**.

## parser
`parse(content) -> ParsedDocument { title, frontmatter(raw), links[], tags[], word_count }`.
- **frontmatter**: YAML-блок между ведущими `---` (вырезан из тела, хвостовые `\n` срезаны).
- **title**: frontmatter `title:`, иначе первый H1.
- **links**: `[[wiki]]`, `![[embed]]` (ручной скан сырого тела) + внутренние markdown-ссылки
  (из pulldown). Матчи внутри код-спанов/код-блоков ИСКЛЮЧЕНЫ (диапазоны кода берутся из pulldown).
  Цель wiki нормализуется: срезаются `#heading` и `|alias`.
- **tags**: `#tag` (обязательна буква → `# H` и `#123` отсекаются), lowercase, уникальные.

## watcher
- `is_ignored(path)`: `.nexus`/`.git`, `*.db`/`*.db-wal`/`*.db-shm`, dotfiles, `.conflict` (AC-Б9-2) —
  иначе записи БД внутри vault зациклили бы реиндексацию.
- `normalize(changes)`: по пути, последнее состояние побеждает → remove+create = `Upsert`, шторм
  схлопывается в одно событие (AC-Б9-3).
- `VaultWatcher`: `notify-debouncer-full` (400 мс) + `FileIdMap`; шлёт `VaultEvent` в канал.

## indexer
- `index_file(rel)`: шорткат по mtime+size (не читаем неизменённое); `parse` в `spawn_blocking`;
  в ОДНОЙ write-транзакции — UPSERT `files` по `path` (СОХРАНЯЕТ `file_id`, AC-Б9-1), полная замена
  `links`/`tags`, прямой + обратный резолв целей ссылок.
- `remove_file(rel)`: soft-delete + обнуление входящих ссылок + чистка исходящих/тегов.
- `scan_vault()`: рекурсивный обход `.md` + финальный до-резолв висячих ссылок; при RAG —
  чекпойнт usearch + прогресс N/M каждые `SCAN_CHECKPOINT` файлов (AC-PERF-5), финальный `save`.
- `spawn(indexer)`: начальный скан + цикл `watcher → index` (вызывается из `open_vault`, который
  и решает — `Indexer::new` без RAG или `with_rag`).

## RAG-индексация (Ф1-5)
Включается, только если в `open_vault` собран embedding-провайдер (есть `.nexus/local.json`
с `ai.embedding` и сервер доступен); иначе RAG-шаги пропускаются — **vault работает без AI**.

На каждый `.md` дополнительно к графу/тегам:
1. **Чанкинг** (`chunker`, пока `WordTokenizer`-placeholder) — в том же `spawn_blocking`, что и parse.
2. **Эмбеддинг** чанков батчами по `EMBED_BATCH` под семафором — **до** транзакции (async, вне rusqlite).
3. В **той же** write-транзакции, что file/links/tags: полная замена `chunks` (старые id → для usearch;
   `DELETE`+`INSERT … RETURNING id`; FTS5 синхронизируется триггерами `chunks_ai/ad/au` — AC-Б8-1/2).
4. **После** транзакции — usearch: `remove` старых chunk-векторов + `upsert(chunk_id, vec)` новых
   (1:1, порядок сохранён `RETURNING` → нет осиротевших векторов, **AC-Б4-1/2**).

`remove_file` дополнительно удаляет `chunks` (+FTS) и снимает их векторы из usearch (**AC-Б8-2**).

**Транзакционность.** SQLite-часть (file/links/tags/chunks) атомарна; usearch — sibling-файл вне БД,
обновляется сразу после коммита. Полная атомарность с БД невозможна → крах между коммитом и
`save()` оставляет рассинхрон, который подчистит reconcile (§5.1, ниже).

**Переэмбеддизация при смене модели (§6.5, AC-Б5-2).** `embedding.model`/`embedding.dim` хранятся в
`settings`. `reconcile_embedding_model` (в `open_vault`) сверяет их с активным эмбеддером: при
расхождении на НЕпервом запуске чистит `chunks` и файл векторов; в любом случае несовпадения
поднимает `force` → начальный скан игнорирует mtime-шорткат и переиндексирует всё. `dim` берётся из
конфига или пробным эмбеддингом (`OpenAiEmbedder::probe_dim`) — НЕ хардкод.

## Резолв ссылок
Прямой: для каждой исходящей ссылки ищем файл по точному пути / `+.md` / basename `±.md`
(кратчайший путь при неоднозначности — упрощённо). Обратный: при появлении файла висячие
ссылки на него (`target_id IS NULL`, совпадение по нормализованным формам) до-резолвятся.

## Тесты
- parser: frontmatter/title/links/tags, исключение кода, номера строк.
- watcher: `is_ignored` (AC-Б9-2), `normalize` (AC-Б9-3).
- indexer (граф): atomic-save сохраняет file_id + беклинки (AC-Б9-1), обратный резолв, замена тегов.
- indexer (RAG, `MockEmbedder`): запись chunks+FTS+векторов (Б4-1); реиндексация без осиротевших
  векторов (Б4-2); `remove` чистит chunks+FTS+векторы (Б8-2); `force` переиндексирует неизменённый
  файл (§6.5). `reconcile_embedding_model`: первое включение → force+settings; смена модели → wipe+force.
- **Живой** (`#[ignore]`, nomic :8081): индексируем cat.md+physics.md → семантический запрос про кошку
  находит чанк именно из cat.md. ✓ проверено вживую.

## Дальше
- **Reconcile после краха** (§5.1): дочинить usearch для файлов, чьи chunks есть в БД, но векторов нет
  (крах между коммитом и `save`); персистентная очередь индексации; дебаунс самого `save`.
- Реальный токенайзер чанкера из эмбеддера (сейчас `WordTokenizer`); параллельный начальный скан.
- Tauri-событие прогресса индексации в UI (Ф1-8) — сейчас прогресс только в логах.
- Резолв через aliases (frontmatter `aliases:`) — когда появится разбор frontmatter в JSON.
