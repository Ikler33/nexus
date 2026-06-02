# Индексатор: parser + watcher + indexer (`src-tauri/src/{parser,watcher,indexer}`)

> Подсистема Ф0-4 (§4.2, Б9). Критерии: **AC-Б9-1/2/3**. Chunks/embeddings (RAG) — Фаза 1.

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
- `scan_vault()`: рекурсивный обход `.md` + финальный до-резолв висячих ссылок.
- `spawn(db, root)`: начальный скан + цикл `watcher → index` (вызывается из `open_vault`).

## Резолв ссылок
Прямой: для каждой исходящей ссылки ищем файл по точному пути / `+.md` / basename `±.md`
(кратчайший путь при неоднозначности — упрощённо). Обратный: при появлении файла висячие
ссылки на него (`target_id IS NULL`, совпадение по нормализованным формам) до-резолвятся.

## Тесты
- parser: frontmatter/title/links/tags, исключение кода, номера строк.
- watcher: `is_ignored` (AC-Б9-2), `normalize` (AC-Б9-3).
- indexer: atomic-save сохраняет file_id + беклинки (AC-Б9-1), обратный резолв, замена тегов.

## Дальше
- Reconcile после краха (`indexed_at < updated_at` / `hash` ≠), персистентная очередь индексации (§5.1).
- chunks/FTS5/usearch (RAG) — Ф1; rename как перемещение записи с сохранением `file_id` — refinement.
- Резолв через aliases (frontmatter `aliases:`) — когда появится разбор frontmatter в JSON.
