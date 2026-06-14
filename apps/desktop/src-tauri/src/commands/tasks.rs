//! TASK-дашборд (TASK-1): сводка markdown-задач (`- [ ]`/`- [x]`) со всех заметок vault.
//! Скан на лету — без индекс-таблицы (выбор плана: хирургия индексатора + force-реиндекс старых
//! vault слишком рискованны ради личного vault; индекс-016 оставлен в BACKLOG как апгрейд при росте
//! до десятков тысяч заметок). Перечисляем проиндексированные заметки из `files`, читаем их тела
//! параллельно и парсим таск-строки. Парсер — РУЧНОЕ зеркало фронтового `TASK_LINE_RE`
//! (apps/desktop/src/lib/editor/format.ts), без regex-крейта. Тоггл задачи делается на фронте
//! (буфер-aware, lib/tasks/toggle.ts) — отдельная бэк-команда не нужна.

use std::path::Path;

use futures::stream::{self, StreamExt};
use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;
use crate::vault::{self, NoteRef};

/// Сколько заметок читаем «в полёте» при скане (как SCAN_CONCURRENCY индексатора).
const SCAN_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub path: String,
    /// 1-based номер строки задачи в файле.
    pub line: u32,
    pub checked: bool,
    /// Текст задачи после `]` (trim).
    pub text: String,
    /// Заголовок заметки из индекса (None → фронт покажет basename).
    pub title: Option<String>,
}

/// Разбор одной строки как markdown-таска — ТОЧНОЕ зеркало фронтового TASK_LINE_RE
/// `^(\s*(?:[-*+]|\d+[.)])\s+\[)([ xX])(\].*)$`. Возвращает `(checked, текст-после-`]`-trim)` или
/// `None`. Таск в цитате (`> - [ ]`) НЕ распознаётся — префикс `>` ломает якорь `^` (контракт EDIT-5).
pub(crate) fn parse_task_line(line: &str) -> Option<(bool, String)> {
    let trimmed = line.trim_start(); // \s* отступ
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    // Маркер списка: -/*/+ ИЛИ <цифры>(.|))
    let after_marker = if matches!(bytes[0], b'-' | b'*' | b'+') {
        &trimmed[1..]
    } else {
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == 0 || i >= bytes.len() || !matches!(bytes[i], b'.' | b')') {
            return None;
        }
        &trimmed[i + 1..]
    };
    // \s+ — хотя бы один пробел после маркера
    let after_ws = after_marker.trim_start();
    if after_ws.len() == after_marker.len() {
        return None;
    }
    // \[[ xX]\]
    let ab = after_ws.as_bytes();
    if ab.len() < 3 || ab[0] != b'[' || ab[2] != b']' || !matches!(ab[1], b' ' | b'x' | b'X') {
        return None;
    }
    let checked = ab[1] != b' ';
    let text = after_ws[3..].trim().to_string();
    Some((checked, text))
}

/// Собирает все таск-строки одного файла в `TaskItem`'ы (1-based номера строк).
fn tasks_in_file(path: &str, title: &Option<String>, content: &str) -> Vec<TaskItem> {
    content
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            parse_task_line(line).map(|(checked, text)| TaskItem {
                path: path.to_string(),
                line: (i + 1) as u32,
                checked,
                text,
                title: title.clone(),
            })
        })
        .collect()
}

/// Сводка всех задач vault (скан на лету). Список заметок — из индекса `files` (даёт пути и
/// заголовки), тела читаем параллельно. Несохранённые правки фронт накладывает из грязных буферов
/// (lib/tasks/collect.ts) — здесь только дисковое состояние.
#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> AppResult<Vec<TaskItem>> {
    let (root, reader) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.reader().clone())
    };
    let notes: Vec<NoteRef> = reader
        .query(|c| {
            let mut stmt = c.prepare("SELECT path, title FROM files WHERE is_deleted=0")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(NoteRef {
                        path: r.get(0)?,
                        title: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;

    let nested: Vec<Vec<TaskItem>> = stream::iter(notes)
        .map(|note| {
            let root = root.clone();
            async move {
                let Ok(abs) = vault::resolve_vault_path(&root, Path::new(&note.path)) else {
                    return Vec::new();
                };
                let Ok(content) = tokio::fs::read_to_string(&abs).await else {
                    return Vec::new(); // файл исчез между листингом и чтением — пропускаем
                };
                tasks_in_file(&note.path, &note.title, &content)
            }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .collect()
        .await;

    Ok(nested.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::parse_task_line;

    #[test]
    fn parses_task_variants() {
        assert_eq!(
            parse_task_line("- [ ] buy milk"),
            Some((false, "buy milk".into()))
        );
        assert_eq!(parse_task_line("- [x] done"), Some((true, "done".into())));
        assert_eq!(parse_task_line("* [X] star"), Some((true, "star".into())));
        assert_eq!(
            parse_task_line("1. [ ] first"),
            Some((false, "first".into()))
        );
        assert_eq!(parse_task_line("42) [ ] num"), Some((false, "num".into())));
        assert_eq!(
            parse_task_line("    - [x] indented"),
            Some((true, "indented".into()))
        );
    }

    #[test]
    fn rejects_non_tasks() {
        assert_eq!(parse_task_line("- bullet"), None);
        assert_eq!(parse_task_line("plain text"), None);
        assert_eq!(parse_task_line(""), None);
        assert_eq!(parse_task_line("> - [ ] quoted"), None); // цитата — не таск (зеркало EDIT-5)
        assert_eq!(parse_task_line("-[ ] nospace"), None); // нет пробела после маркера
        assert_eq!(parse_task_line("- [] empty"), None); // пустой бокс
        assert_eq!(parse_task_line("- [y] bad"), None); // символ не из [ xX]
    }
}
