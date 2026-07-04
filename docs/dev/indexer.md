# Индексатор: parser + watcher + indexer (`src-tauri/src/{parser,watcher,indexer}`)

> Подсистема Ф0-4 (§4.2, Б9) + RAG-индексация Ф1-5 (§6.1). Критерии: **AC-Б9-1/2/3**, RAG —
> **AC-Б4-1/2 · AC-Б5-2 · AC-Б8-1/2 · AC-PERF-5**.

## parser
`parse(content) -> ParsedDocument { title, frontmatter(raw), links[], tags[], aliases[], fields[], word_count }`.
- **fields**: плоские скаляры верхнего уровня frontmatter (`progress/due/goal/evergreen/draft`…) как
  `(key, value)` — мини-парсер (без YAML-либы); инлайн-списки/вложенный YAML/блок-списки исключены.
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
  схлопывается в одно событие (AC-Б9-3). **Пара переименования** (`RawChange::Renamed`, склеенная из
  двух путей события `Modify(Name)`) → `VaultEvent::Renamed{from,to}`, если итоговое состояние не
  перекрыто другими событиями пачки (иначе деградирует к Deleted/Upsert — безопасно).
- `VaultWatcher`: `notify-debouncer-full` (400 мс) + `FileIdMap`; склеивает From/To одного move по
  file-id в одно событие с двумя путями; шлёт `VaultEvent` (`Upsert`/`Deleted`/`Renamed`) в канал.

## indexer
- `index_file(rel)`: шорткат по mtime+size (не читаем неизменённое); `parse` в `spawn_blocking`;
  в ОДНОЙ write-транзакции — UPSERT `files` по `path` (СОХРАНЯЕТ `file_id`, AC-Б9-1), полная замена
  `links`/`tags`, прямой + обратный резолв целей ссылок.
- `remove_file(rel)`: soft-delete + обнуление входящих ссылок + чистка исходящих/тегов.
- `rename_file(from, to)` (V2.2, AC-Б9-1): **перенос `files.path` с сохранением `file_id`** вместо
  delete+create → беклинки (входящие ссылки на этот id) и чанки целы. В одной транзакции: найти
  `from`-строку; при коллизии замостить строку на `to` (убрать её links/tags/chunks/aliases); `UPDATE
  files SET path`; до-резолв висячих `[[New]]` на новые формы имени. Затем `index_file(to)` —
  обновит контент под тем же id, если rename совпал с правкой (чистый rename → ранний выход).
  Источника нет в БД → индексируем `to` как новый; rename в не-`.md` → `remove_file(from)`.
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

**Переэмбеддизация при смене модели (§6.5, AC-Б5-2; R-3d).** `embedding.model`/`embedding.dim`
хранятся в `settings`. КАНОННЫЙ `nexus_core::vector::reconcile_embedding_model` (его зовут desktop
`open_vault`/`build_rag` и agentd `build_rag_min` ДО открытия индексов) сверяет их с активным
эмбеддером: при расхождении на НЕпервом запуске — ПОЛНАЯ чистка производных (решение владельца
§8.5): `chunks` (+FTS триггерами), ВСЕ 4 usearch-файла (`VECTOR_INDEX_FILES`, вкл. `chat_vectors`),
`chat_episodes.embed_model=NULL`; в любом случае несовпадения поднимает `force` → начальный скан
игнорирует mtime-шорткат и переиндексирует всё. Чистка дополнительно взводит durable-маркер —
`files.size_bytes=-1` (реальный размер неотрицателен): mtime+size-шорткат сломан для всех файлов,
и chunks пересоздаются ЛЮБЫМ следующим сканом, даже если `force`-возврат потребил процесс без
индексатора заметок (agentd реконсилил первым → desktop открылся позже как no-op). Та же модель/dim —
СТРОГИЙ no-op (пользовательские индексы не пересобираются на ровном месте). `dim` берётся из
конфига или пробным эмбеддингом (`OpenAiEmbedder::probe_dim`) — НЕ хардкод.

## Резолв ссылок
Прямой: для каждой исходящей ссылки ищем файл по точному пути / `+.md` / basename `±.md`
(кратчайший путь при неоднозначности — упрощённо). Обратный: при появлении файла висячие
ссылки на него (`target_id IS NULL`, совпадение по нормализованным формам) до-резолвятся.

**Алиасы (V4.1).** Frontmatter `aliases:` (инлайн `[A,B]` / блочный `- A` / скаляр `alias: A`) парсятся
мини line-парсером (без YAML-либы — `serde_yaml` архивирован) и пишутся в таблицу `aliases`
(полная замена на файл; `OR REPLACE` на глобальном `UNIQUE(alias)`). `resolve_target` и
`resolve_all_dangling` после path-матча падают на алиас (`COALESCE(path, alias)`), а обратный резолв
обновляет висячие ссылки и по алиасам файла — так `[[Алиас]]` находит цель forward и backward. **Путь
приоритетнее алиаса.**

**Typed-frontmatter (плоские поля).** Плоские скаляры frontmatter (`parsed.fields`) пишутся в таблицу
`frontmatter_fields` (миграция 003; `UNIQUE(file_id,key)` + индекс по `key`) — полная замена на файл,
как теги/алиасы. Разблокирует кросс-файловые запросы (цели/stale-radar/Dataview). Значения — строки
(типизацию делает консьюмер); сложный/вложенный YAML — fallback на сырой `frontmatter`. Выбор владельца:
расширенный мини-парсер (без YAML-либы). Query-API/команда — с первым консьюмером (BACKLOG).

## Тесты
- parser: frontmatter/title/links/tags, исключение кода, номера строк; **плоские поля frontmatter
  (только скаляры; дубль→последний; списки/вложенность исключены)**.
- watcher: `is_ignored` (AC-Б9-2), `normalize` (AC-Б9-3) + склейка переименования в `Renamed` и
  безопасная деградация перекрытого move (V2.2).
- indexer (граф): atomic-save сохраняет file_id + беклинки (AC-Б9-1), обратный резолв, замена тегов;
  **rename сохраняет file_id+беклинки** (`[[Old]]` по id, `[[New]]` до-резолвилась) и **чанки+векторы**
  (V2.2); **плоские поля frontmatter пишутся в `frontmatter_fields` и заменяются при реиндексе**.
- indexer (RAG, `MockEmbedder`): запись chunks+FTS+векторов (Б4-1); реиндексация без осиротевших
  векторов (Б4-2); `remove` чистит chunks+FTS+векторы (Б8-2); `force` переиндексирует неизменённый
  файл (§6.5). `reconcile_embedding_model`: первое включение → force+settings; смена модели → wipe+force.
- **Живой** (`#[ignore]`, nomic :8081): индексируем cat.md+physics.md → семантический запрос про кошку
  находит чанк именно из cat.md. ✓ проверено вживую.

## Crash-reconcile usearch (§5.1) — реализовано
`reconcile_vectors` (вызывается в конце `scan_vault`): чанки, что есть в БД, но чьих векторов нет в
usearch (`contains == false` — крах между commit и `save`), переэмбеддит батчами и доливает. На
force-скане no-op. Best-effort: эмбеддер недоступен → лог + выход (повтор при следующем открытии).
Тест `reconcile_restores_lost_vectors` (имитируем потерю вектора → reconcile возвращает).

## Дальше
- **Переписывание текста ссылок при rename** (`[[Old]]`→`[[New]]` у ссылающихся файлов, как «update
  links on rename» в Obsidian). V2.2 сохраняет `file_id` и беклинки по id, но текст `[[Old]]` у
  источников не правится → при их следующей переиндексации `[[Old]]` повиснет. Требует записи в файлы
  пользователя (осторожно) + опция в настройках. BACKLOG.
- Персистентная очередь индексации + дебаунс самого `save` усearch (сейчас по чекпойнтам/событию).
- Реальный токенайзер чанкера из эмбеддера (сейчас `WordTokenizer`); параллельный начальный скан.
- Tauri-событие прогресса индексации в UI (Ф1-8) — сейчас прогресс только в логах.
- Резолв через aliases (frontmatter `aliases:`) — когда появится разбор frontmatter в JSON.
