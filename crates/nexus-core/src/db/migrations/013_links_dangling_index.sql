-- Перф (#19 cold-bench): полный скан vault был O(N²) на ДВУХ местах резолва ссылок. Замер
-- (плоский синтетический vault, мок-эмбеддинг): 1k→10k файлов индексация 1.3с→83.8с,
-- throughput 770→119 файлов/с — суперлинейно.

-- (1) Обратный резолв в `index_file` на КАЖДЫЙ файл:
--     `UPDATE links SET target_id=? WHERE target_id IS NULL AND target_raw=?` — без индекса по
--     `target_raw` это полный скан таблицы `links` (растёт ~3×N). Частичный индекс ровно под этот
--     UPDATE (только НЕрезолвленные ссылки): в связном vault почти пуст.
CREATE INDEX IF NOT EXISTS idx_links_dangling
    ON links(target_raw)
    WHERE target_id IS NULL;

-- (2) Прямой резолв `resolve_target` (3× на файл) для basename-шортката `[[Note]]` → искал
--     `path LIKE '%/' || ?` — ВЕДУЩИЙ wildcard не индексируется, полный скан `files` на каждую
--     ссылку. Индекс по ВЫРАЖЕНИЮ «последний сегмент пути» (после последнего `/`): шорткат-резолв
--     становится O(log N) без отдельной колонки/бэкфилла. Выражение ДОЛЖНО совпадать дословно с
--     тем, что в запросе (`indexer::links`), иначе планировщик индекс не возьмёт.
--     Идиома basename без reverse(): rtrim по набору всех неслэш-символов снимает хвост до
--     последнего `/`, replace срезает получившийся префикс-каталог.
CREATE INDEX IF NOT EXISTS idx_files_basename
    ON files( replace(path, rtrim(path, replace(path, '/', '')), '') );
