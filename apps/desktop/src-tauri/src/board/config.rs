//! Персист канбан-доски (BOARD-3): конфиг доски в `.nexus/boards/<id>.json` — колонки (переименование
//! без правки файлов), ручной порядок карточек (фундамент DnD-реордера BOARD-5), scope (разбивка по
//! проектам/папкам), statusKey. Запись атомарна (`atomic_write_io`) — обрыв не корраптит конфиг.
//!
//! Источник истины колонки — `id` = raw-значение `status` в файле; `label` — отображение (пусто → фронт
//! локализует через `board.col.*`). order — `{colId: [path]}`; самозалечивается (GC удалённых при чтении,
//! патч пути при rename). Битый JSON → дефолт + флаг (фронт-тост); отсутствие файла = первый запуск (НЕ
//! ошибка). serde_json (serde_yaml архивирован — но это НЕ vault-данные пользователя, а служебный конфиг).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::TaskCard;

/// id доски по умолчанию (одна доска в MVP; мульти-доска — позже).
pub const BOARD_ID_DEFAULT: &str = "personal";

fn default_status_key() -> String {
    super::DEFAULT_STATUS_KEY.to_string()
}
fn default_card_fields() -> Vec<String> {
    vec!["due".into(), "priority".into(), "tags".into()]
}
fn default_sort() -> String {
    "manual".into()
}

/// Колонка доски: `id` = raw-значение `status` (источник истины); `label` пусто → локализация на фронте.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BoardColumn {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub wip: Option<u32>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub done_like: bool,
}

/// Scope доски — «разбивка по проектам» на уровне вью (folder-префикс / project-поле / superset тегов).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardScope {
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Конфиг доски (персист `.nexus/boards/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BoardConfig {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_status_key")]
    pub status_key: String,
    pub columns: Vec<BoardColumn>,
    #[serde(default)]
    pub scope: BoardScope,
    /// colId → упорядоченные пути карточек (ручной порядок). Самозалечивается.
    #[serde(default)]
    pub order: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default = "default_card_fields")]
    pub card_fields: Vec<String>,
}

/// Дефолтная доска: колонки todo/doing/done (label пусто → фронт локализует), done терминальная.
pub fn default_board() -> BoardConfig {
    default_board_with_id(BOARD_ID_DEFAULT)
}

fn default_board_with_id(id: &str) -> BoardConfig {
    let col = |id: &str, done_like: bool| BoardColumn {
        id: id.to_string(),
        label: String::new(),
        wip: None,
        color: None,
        done_like,
    };
    BoardConfig {
        id: id.to_string(),
        title: String::new(),
        status_key: default_status_key(),
        columns: vec![col("todo", false), col("doing", false), col("done", true)],
        scope: BoardScope::default(),
        order: BTreeMap::new(),
        sort: default_sort(),
        card_fields: default_card_fields(),
    }
}

/// Краткая сводка доски для списка/переключателя.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary {
    pub id: String,
    pub title: String,
}

/// Результат чтения конфига: дефолт-фолбэк, плюс `corrupt` — JSON был, но битый (→ фронт-тост, не молча).
pub struct LoadedBoard {
    pub config: BoardConfig,
    pub corrupt: bool,
}

fn boards_dir(root: &Path) -> PathBuf {
    root.join(".nexus").join("boards")
}

/// Безопасное имя файла доски: id — идентификатор (анти-traversal в имени файла).
fn board_file(root: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
    {
        return None;
    }
    Some(boards_dir(root).join(format!("{id}.json")))
}

/// Читает конфиг доски. Нет файла → дефолт (первый запуск, `corrupt=false`); битый JSON → дефолт +
/// `corrupt=true` (фронт покажет тост и не затрёт пользовательский файл вслепую — save только по действию).
pub fn load(root: &Path, id: &str) -> LoadedBoard {
    let Some(path) = board_file(root, id) else {
        return LoadedBoard {
            config: default_board_with_id(BOARD_ID_DEFAULT),
            corrupt: false,
        };
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<BoardConfig>(&raw) {
            Ok(mut config) => {
                // F4 (ревью): имя файла — источник истины id (иначе `{"id":"work"}` в personal.json увёл
                // бы save/self-heal в work.json, осиротив файл). F2: дедуп колонок по id (дубль из ручного
                // JSON ломал бы группировку last-wins и давал дубль React-key).
                config.id = id.to_string();
                dedup_columns(&mut config.columns);
                LoadedBoard {
                    config,
                    corrupt: false,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, id, "board JSON битый — фолбэк на дефолт");
                LoadedBoard {
                    config: default_board_with_id(id),
                    corrupt: true,
                }
            }
        },
        Err(_) => LoadedBoard {
            config: default_board_with_id(id),
            corrupt: false,
        },
    }
}

/// Пишет конфиг доски атомарно (каталог создаётся). id валидируется (анти-traversal).
pub fn save(root: &Path, cfg: &BoardConfig) -> std::io::Result<()> {
    let path = board_file(root, &cfg.id).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "недопустимый id доски")
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(cfg).expect("BoardConfig сериализуем всегда");
    crate::vault::atomic_write_io(&path, json.as_bytes())
}

/// Список досок (`ls .nexus/boards/*.json`), битые пропускаются. Пусто → синтетический дефолт (всегда
/// есть хотя бы одна доска для UI).
pub fn list_boards(root: &Path) -> Vec<BoardSummary> {
    let mut out: Vec<BoardSummary> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(boards_dir(root)) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(&p) {
                if let Ok(cfg) = serde_json::from_str::<BoardConfig>(&raw) {
                    out.push(BoardSummary {
                        id: cfg.id,
                        title: cfg.title,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    if out.is_empty() {
        out.push(BoardSummary {
            id: BOARD_ID_DEFAULT.to_string(),
            title: String::new(),
        });
    }
    out
}

/// Карточка попадает в доску по scope: folder-префикс И project-совпадение И superset тегов (все условия,
/// заданные непусто). Пустой scope → всё. Папка матчится как префикс пути с разделителем (без ложных
/// срабатываний `Work` vs `Workspace`).
pub fn matches_scope(card: &TaskCard, scope: &BoardScope) -> bool {
    if let Some(folder) = scope.folder.as_deref().filter(|f| !f.is_empty()) {
        let prefix = if folder.ends_with('/') {
            folder.to_string()
        } else {
            format!("{folder}/")
        };
        if !card.path.starts_with(&prefix) {
            return false;
        }
    }
    if let Some(project) = scope.project.as_deref().filter(|p| !p.is_empty()) {
        if card.project.as_deref() != Some(project) {
            return false;
        }
    }
    scope.tags.iter().all(|t| card.tags.contains(t))
}

/// GC порядка: убирает из `order` пути, которых нет среди `existing` (удалённые/вне-scope карточки), и
/// пустые/«осиротевшие» колонки. Возвращает true, если что-то изменилось (→ опц. persist self-heal).
pub fn gc_order(cfg: &mut BoardConfig, existing: &HashSet<&str>) -> bool {
    let mut changed = false;
    cfg.order.retain(|_, paths| {
        let before = paths.len();
        paths.retain(|p| existing.contains(p.as_str()));
        if paths.len() != before {
            changed = true;
        }
        !paths.is_empty()
    });
    changed
}

/// Дедуп колонок по id (case-insensitive, ПЕРВЫЙ выигрывает) — дубль из ручного JSON ломал бы
/// группировку (Map last-wins → первая колонка пустая) и давал дубль React-key (adversarial-ревью F2).
fn dedup_columns(columns: &mut Vec<BoardColumn>) {
    let mut seen: HashSet<String> = HashSet::new();
    columns.retain(|c| seen.insert(c.id.to_lowercase()));
}

/// Точечно убирает удалённые пути из order во ВСЕХ досках (delete-хук из `delete_path`/корзина). Это
/// БЕЗОПАСНЫЙ self-heal порядка: чистим по РЕАЛЬНОМУ удалению, а не по «отсутствию в выборке» (F1 —
/// холодный индекс не должен стирать порядок живых задач). Best-effort, число изменённых файлов.
pub fn remove_from_orders(root: &Path, removed: &[String]) -> usize {
    if removed.is_empty() {
        return 0;
    }
    let drop: HashSet<&str> = removed.iter().map(String::as_str).collect();
    let Ok(entries) = std::fs::read_dir(boards_dir(root)) else {
        return 0;
    };
    let mut patched = 0usize;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(mut cfg) = serde_json::from_str::<BoardConfig>(&raw) else {
            continue;
        };
        let mut changed = false;
        cfg.order.retain(|_, paths| {
            let before = paths.len();
            paths.retain(|path| !drop.contains(path.as_str()));
            if paths.len() != before {
                changed = true;
            }
            !paths.is_empty()
        });
        if changed && save(root, &cfg).is_ok() {
            patched += 1;
        }
    }
    patched
}

/// Патчит путь во ВСЕХ досках при rename (from→to по парам .md). Best-effort, возвращает число изменённых
/// файлов. Сохраняет ПОЗИЦИЮ карточки в колонке (не сбрасывает в конец) — ключевой инвариант §14.6.
pub fn rename_in_orders(root: &Path, pairs: &[(String, String)]) -> usize {
    if pairs.is_empty() {
        return 0;
    }
    let mut patched = 0usize;
    let Ok(entries) = std::fs::read_dir(boards_dir(root)) else {
        return 0;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(mut cfg) = serde_json::from_str::<BoardConfig>(&raw) else {
            continue;
        };
        let mut changed = false;
        for paths in cfg.order.values_mut() {
            for path in paths.iter_mut() {
                if let Some((_, to)) = pairs.iter().find(|(from, _)| from == path) {
                    *path = to.clone();
                    changed = true;
                }
            }
        }
        if changed && save(root, &cfg).is_ok() {
            patched += 1;
        }
    }
    patched
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn card(path: &str, project: Option<&str>, tags: &[&str]) -> TaskCard {
        TaskCard {
            path: path.into(),
            title: None,
            status: "todo".into(),
            project: project.map(str::to_string),
            priority: None,
            due: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn load_missing_is_fresh_default_not_corrupt() {
        let dir = TempDir::new().unwrap();
        let r = load(dir.path(), BOARD_ID_DEFAULT);
        assert!(!r.corrupt);
        assert_eq!(r.config.columns.len(), 3);
        assert!(r
            .config
            .columns
            .iter()
            .any(|c| c.id == "done" && c.done_like));
    }

    #[test]
    fn load_corrupt_json_falls_back_with_flag() {
        let dir = TempDir::new().unwrap();
        let path = board_file(dir.path(), "personal").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ это не json").unwrap();
        let r = load(dir.path(), "personal");
        assert!(r.corrupt, "битый JSON → corrupt=true");
        assert_eq!(r.config.id, "personal");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let mut cfg = default_board();
        cfg.title = "Личные".into();
        cfg.order
            .insert("todo".into(), vec!["a.md".into(), "b.md".into()]);
        save(dir.path(), &cfg).unwrap();
        let r = load(dir.path(), BOARD_ID_DEFAULT);
        assert!(!r.corrupt);
        assert_eq!(r.config, cfg);
    }

    #[test]
    fn id_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        assert!(board_file(dir.path(), "../evil").is_none());
        assert!(board_file(dir.path(), "a/b").is_none());
        assert!(board_file(dir.path(), "").is_none());
        assert!(board_file(dir.path(), "ok-1_2").is_some());
    }

    #[test]
    fn gc_order_drops_missing_and_empty_columns() {
        let mut cfg = default_board();
        cfg.order
            .insert("todo".into(), vec!["a.md".into(), "gone.md".into()]);
        cfg.order.insert("doing".into(), vec!["gone2.md".into()]);
        let existing: HashSet<&str> = ["a.md"].into_iter().collect();
        assert!(gc_order(&mut cfg, &existing));
        assert_eq!(cfg.order.get("todo").unwrap(), &vec!["a.md".to_string()]);
        assert!(!cfg.order.contains_key("doing"), "пустая колонка убрана");
        // Повторный GC ничего не меняет (идемпотентно).
        assert!(!gc_order(&mut cfg, &existing));
    }

    #[test]
    fn matches_scope_folder_project_tags() {
        let s_folder = BoardScope {
            folder: Some("Work".into()),
            ..Default::default()
        };
        assert!(matches_scope(&card("Work/a.md", None, &[]), &s_folder));
        assert!(!matches_scope(
            &card("Workspace/a.md", None, &[]),
            &s_folder
        )); // не префикс-обман
        assert!(!matches_scope(&card("Home/a.md", None, &[]), &s_folder));

        let s_proj = BoardScope {
            project: Some("Nexus".into()),
            ..Default::default()
        };
        assert!(matches_scope(&card("a.md", Some("Nexus"), &[]), &s_proj));
        assert!(!matches_scope(&card("a.md", Some("Дом"), &[]), &s_proj));

        let s_tags = BoardScope {
            tags: vec!["task".into(), "urgent".into()],
            ..Default::default()
        };
        assert!(matches_scope(
            &card("a.md", None, &["task", "urgent", "x"]),
            &s_tags
        )); // superset
        assert!(!matches_scope(&card("a.md", None, &["task"]), &s_tags)); // не хватает urgent

        assert!(matches_scope(
            &card("any.md", None, &[]),
            &BoardScope::default()
        )); // пустой → всё
    }

    #[test]
    fn rename_in_orders_preserves_position() {
        let dir = TempDir::new().unwrap();
        let mut cfg = default_board();
        cfg.order.insert(
            "todo".into(),
            vec!["a.md".into(), "old.md".into(), "c.md".into()],
        );
        save(dir.path(), &cfg).unwrap();

        let n = rename_in_orders(dir.path(), &[("old.md".into(), "new.md".into())]);
        assert_eq!(n, 1);
        let r = load(dir.path(), BOARD_ID_DEFAULT);
        assert_eq!(
            r.config.order.get("todo").unwrap(),
            &vec!["a.md".to_string(), "new.md".to_string(), "c.md".to_string()],
            "позиция сохранена (середина), не сброшена в конец"
        );
    }

    #[test]
    fn list_boards_empty_yields_synthetic_default() {
        let dir = TempDir::new().unwrap();
        let l = list_boards(dir.path());
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id, BOARD_ID_DEFAULT);
    }

    #[test]
    fn load_forces_id_from_filename_and_dedups_columns() {
        let dir = TempDir::new().unwrap();
        let path = board_file(dir.path(), "personal").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // id в файле «врёт» (work), и дублирующая колонка todo/Todo.
        std::fs::write(
            &path,
            r#"{"id":"work","columns":[{"id":"todo"},{"id":"Todo"},{"id":"done"}]}"#,
        )
        .unwrap();
        let r = load(dir.path(), "personal");
        assert!(!r.corrupt);
        assert_eq!(r.config.id, "personal", "F4: id из имени файла, не из JSON");
        let ids: Vec<&str> = r.config.columns.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["todo", "done"],
            "F2: дубль Todo убран (первый выигрывает)"
        );
    }

    #[test]
    fn remove_from_orders_drops_deleted_paths_precisely() {
        let dir = TempDir::new().unwrap();
        let mut cfg = default_board();
        cfg.order.insert(
            "todo".into(),
            vec!["a.md".into(), "del.md".into(), "c.md".into()],
        );
        cfg.order.insert("doing".into(), vec!["only-del.md".into()]);
        save(dir.path(), &cfg).unwrap();

        let n = remove_from_orders(dir.path(), &["del.md".into(), "only-del.md".into()]);
        assert_eq!(n, 1);
        let r = load(dir.path(), BOARD_ID_DEFAULT);
        assert_eq!(
            r.config.order.get("todo").unwrap(),
            &vec!["a.md".to_string(), "c.md".to_string()],
            "удалённый путь убран, остальные на местах"
        );
        assert!(
            !r.config.order.contains_key("doing"),
            "опустевшая колонка убрана"
        );
        // Пустой список — no-op.
        assert_eq!(remove_from_orders(dir.path(), &[]), 0);
    }
}
