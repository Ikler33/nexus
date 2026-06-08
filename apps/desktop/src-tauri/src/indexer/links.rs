//! Резолв ссылок индексатора (§4.2): прямой (target → `file_id`) и обратный (до-резолв висячих).
//!
//! Ссылка `[[target]]` сопоставляется файлу по пути (точному / +`.md` / basename) с приоритетом над
//! алиасом (реальный файл важнее одноимённого алиаса). Висячие ссылки (цель ещё не проиндексирована)
//! до-резолвятся: точечно при индексации новой цели ([`path_forms`]) и пакетно после скана
//! ([`resolve_all_dangling`]).

use rusqlite::{OptionalExtension, Transaction};

/// Резолвит цель ссылки в `file_id` (точный путь, путь+`.md`, basename ± `.md`; затем алиас).
/// Путь имеет приоритет над алиасом (реальный файл `X` важнее алиаса `X`).
pub(super) fn resolve_target(tx: &Transaction, target_raw: &str) -> rusqlite::Result<Option<i64>> {
    let by_path = tx
        .query_row(
            "SELECT id FROM files WHERE is_deleted=0 AND ( \
               path = ?1 OR path = ?1 || '.md' \
               OR path LIKE '%/' || ?1 OR path LIKE '%/' || ?1 || '.md' \
             ) ORDER BY length(path) LIMIT 1",
            [target_raw],
            |r| r.get(0),
        )
        .optional()?;
    if by_path.is_some() {
        return Ok(by_path);
    }
    // Фолбэк: точное совпадение с алиасом (V4.1), файл не удалён.
    tx.query_row(
        "SELECT a.file_id FROM aliases a JOIN files f ON f.id = a.file_id \
         WHERE f.is_deleted=0 AND a.alias = ?1 LIMIT 1",
        [target_raw],
        |r| r.get(0),
    )
    .optional()
}

/// До-резолвит ВСЕ висячие ссылки (после начального скана — закрывает порядок индексации).
pub(super) fn resolve_all_dangling(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE links SET target_id = COALESCE( \
           ( SELECT f.id FROM files f WHERE f.is_deleted=0 AND ( \
               f.path = links.target_raw OR f.path = links.target_raw || '.md' \
               OR f.path LIKE '%/' || links.target_raw OR f.path LIKE '%/' || links.target_raw || '.md' \
             ) ORDER BY length(f.path) LIMIT 1 ), \
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
