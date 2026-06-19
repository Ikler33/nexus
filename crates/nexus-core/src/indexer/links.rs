//! Резолв ссылок индексатора (§4.2): прямой (target → `file_id`) и обратный (до-резолв висячих).
//!
//! Ссылка `[[target]]` сопоставляется файлу по пути (точному / +`.md` / basename) с приоритетом над
//! алиасом (реальный файл важнее одноимённого алиаса). Висячие ссылки (цель ещё не проиндексирована)
//! до-резолвятся: точечно при индексации новой цели ([`path_forms`]) и пакетно после скана
//! ([`resolve_all_dangling`]).

use rusqlite::{Connection, OptionalExtension, Transaction};

/// SQL-выражение «basename пути» (последний сегмент после последнего `/`). ДОЛЖНО дословно совпадать
/// с индексом `idx_files_basename` (миграция 013), иначе планировщик его не возьмёт → полный скан.
/// Идиома без `reverse()`: `rtrim` по набору всех неслэш-символов снимает хвост до последнего `/`,
/// `replace` срезает получившийся префикс-каталог. См. cold-bench #19 (резолв был O(N²)).
const BASENAME_EXPR: &str = "replace(path, rtrim(path, replace(path, '/', '')), '')";

/// Резолвит цель ссылки в `file_id` (точный путь, путь+`.md`, basename ± `.md`; затем алиас).
/// Путь имеет приоритет над алиасом (реальный файл `X` важнее алиаса `X`).
///
/// Принимает `&Connection` (а не `Transaction`): индексатор зовёт внутри транзакции (deref),
/// команда `resolve_note` — на read-пуле. ОДНА семантика резолва на запись и клик (кросс-план #22).
///
/// Перф (#19): резолв разбит на ИНДЕКСИРУЕМЫЕ шаги (раньше один OR с ведущим-wildcard LIKE → полный
/// скан `files` на каждую ссылку → O(N²) на скан vault):
/// 1) точный путь / путь+`.md` — UNIQUE-индекс `files.path`;
/// 2) basename-шорткат `[[Note]]` — индекс по выражению `idx_files_basename`;
/// 3) мульти-сегментный шорткат `[[dir/Note]]` (target с `/`) — суффикс не индексируется, скан;
///    редкий путь (только если в цели есть `/`), поэтому не делает скан O(N²) на обычном vault;
/// 4) алиас.
pub fn resolve_target(tx: &Connection, target_raw: &str) -> rusqlite::Result<Option<i64>> {
    // 1) Точный путь / путь+`.md` (индекс).
    let by_path = tx
        .query_row(
            "SELECT id FROM files WHERE is_deleted=0 AND (path = ?1 OR path = ?1 || '.md') \
             ORDER BY length(path) LIMIT 1",
            [target_raw],
            |r| r.get(0),
        )
        .optional()?;
    if by_path.is_some() {
        return Ok(by_path);
    }
    // 2) Шорткат по basename (индекс по выражению; кратчайший путь выигрывает).
    let by_base = tx
        .query_row(
            &format!(
                "SELECT id FROM files WHERE is_deleted=0 AND ({0} = ?1 OR {0} = ?1 || '.md') \
                 ORDER BY length(path) LIMIT 1",
                BASENAME_EXPR
            ),
            [target_raw],
            |r| r.get(0),
        )
        .optional()?;
    if by_base.is_some() {
        return Ok(by_base);
    }
    // 3) Мульти-сегментный шорткат `[[dir/Note]]` — только если в цели есть `/` (иначе пропускаем
    //    скан). Суффикс не индексируется; цена терпима, т.к. срабатывает редко.
    if target_raw.contains('/') {
        let by_suffix = tx
            .query_row(
                "SELECT id FROM files WHERE is_deleted=0 AND \
                   (path LIKE '%/' || ?1 OR path LIKE '%/' || ?1 || '.md') \
                 ORDER BY length(path) LIMIT 1",
                [target_raw],
                |r| r.get(0),
            )
            .optional()?;
        if by_suffix.is_some() {
            return Ok(by_suffix);
        }
    }
    // 4) Фолбэк: точное совпадение с алиасом (V4.1), файл не удалён.
    tx.query_row(
        "SELECT a.file_id FROM aliases a JOIN files f ON f.id = a.file_id \
         WHERE f.is_deleted=0 AND a.alias = ?1 LIMIT 1",
        [target_raw],
        |r| r.get(0),
    )
    .optional()
}

/// До-резолвит ВСЕ висячие ссылки (после начального скана — закрывает порядок индексации).
/// COALESCE-ветки идут от индексируемых к сканирующим: для строки берётся первая non-null, поэтому
/// дорогой LIKE-суффикс (ведущий wildcard) считается ТОЛЬКО для ссылок, не закрытых индексами (#19).
pub(super) fn resolve_all_dangling(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute(
        // Ветки от индексируемых (точный путь → basename-выражение idx_files_basename) к
        // сканирующим (LIKE-суффикс для редкого `[[dir/Note]]`); COALESCE берёт первую non-null,
        // дорогой LIKE считается ТОЛЬКО для не-закрытых индексами строк (#19). basename-выражение
        // должно совпадать с индексом `idx_files_basename` (BASENAME_EXPR, тут на f.path).
        "UPDATE links SET target_id = COALESCE( \
           ( SELECT f.id FROM files f WHERE f.is_deleted=0 AND \
               (f.path = links.target_raw OR f.path = links.target_raw || '.md') \
             ORDER BY length(f.path) LIMIT 1 ), \
           ( SELECT f.id FROM files f WHERE f.is_deleted=0 AND \
               (replace(f.path, rtrim(f.path, replace(f.path, '/', '')), '') = links.target_raw \
                OR replace(f.path, rtrim(f.path, replace(f.path, '/', '')), '') = links.target_raw || '.md') \
             ORDER BY length(f.path) LIMIT 1 ), \
           ( SELECT f.id FROM files f WHERE f.is_deleted=0 AND \
               (f.path LIKE '%/' || links.target_raw OR f.path LIKE '%/' || links.target_raw || '.md') \
             ORDER BY length(f.path) LIMIT 1 ), \
           ( SELECT a.file_id FROM aliases a JOIN files f ON f.id = a.file_id \
             WHERE f.is_deleted=0 AND a.alias = links.target_raw LIMIT 1 ) \
         ) WHERE target_id IS NULL",
        [],
    )?;
    Ok(())
}

/// Нормализованные формы относительного пути для обратного резолва ссылок.
pub(super) fn path_forms(rel: &str) -> Vec<String> {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let mut forms = vec![
        rel.to_string(),
        rel.strip_suffix(".md").unwrap_or(rel).to_string(),
        base.to_string(),
        base.strip_suffix(".md").unwrap_or(base).to_string(),
    ];
    forms.sort();
    forms.dedup();
    forms
}
