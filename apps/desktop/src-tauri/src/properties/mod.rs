//! Реестр типов свойств (PROP-2, спека §7 kanban-board.md): `.nexus/property-types.json` — тип свойства
//! ГЛОБАЛЕН по ИМЕНИ (как Obsidian Properties). Явно заданные типы хранятся в реестре; для остального —
//! эвристика по значению. Потребитель — Properties-панель (PROP-3): тип → выбор виджета.
//!
//! serde_json (служебный конфиг в `.nexus`, НЕ vault-данные пользователя; serde_yaml архивирован, но это
//! не frontmatter). Запись атомарна (`atomic_write_io`). Битый JSON → пустой реестр (одна эвристика).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Тип свойства (виджет Properties-панели). JSON — lowercase (`"text"`, `"datetime"`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    Text,
    List,
    Number,
    Checkbox,
    Date,
    Datetime,
    Tags,
}

/// Реестр: имя свойства → явный тип. Порядок стабилен (BTreeMap) — детерминированный JSON.
pub type Registry = BTreeMap<String, PropertyType>;

/// Имена, у которых тип форсится в `Tags` (Obsidian-семантика — список тегов, не текст).
const FORCED_TAGS: [&str; 3] = ["tags", "aliases", "cssclasses"];

/// `true`, если значение — YAML-bool (любой регистр). → Checkbox.
fn is_bool(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off"
    )
}

/// `YYYY-MM-DD` (валидные диапазоны мес/дня — грубо, без календаря).
fn is_iso_date(v: &str) -> bool {
    let b = v.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    digits(0..4) && digits(5..7) && digits(8..10)
}

/// `YYYY-MM-DDThh:mm` (опц. `:ss`, опц. зона) — ISO datetime по началу строки.
fn is_iso_datetime(v: &str) -> bool {
    if v.len() < 16 {
        return false;
    }
    let (date, rest) = v.split_at(10);
    is_iso_date(date)
        && (rest.starts_with('T') || rest.starts_with(' '))
        && rest.as_bytes().get(3) == Some(&b':')
        && rest[1..3].bytes().all(|c| c.is_ascii_digit())
        && rest[4..6].bytes().all(|c| c.is_ascii_digit())
}

/// Число (int/float). Пустое — нет.
fn is_number(v: &str) -> bool {
    !v.is_empty() && v.parse::<f64>().is_ok()
}

/// Инлайн-список YAML `[a, b]`. CSV-подобное (`Привет, мир`) НЕ список — это текст с запятой
/// (иначе ложные срабатывания). Скаляры frontmatter и так не `[...]` (парсер их отбрасывает) — ветка
/// для значений, переданных напрямую (PROP-3 может читать сырой список).
fn is_inline_list(v: &str) -> bool {
    v.starts_with('[') && v.ends_with(']')
}

/// Эвристика типа по значению, когда имя НЕ в реестре. Порядок важен: forced-tags → bool → datetime →
/// date → number → list → text (datetime ДО date, bool ДО number, иначе `off`/`0` уехали бы не туда).
pub fn infer_type(key: &str, value: &str) -> PropertyType {
    if FORCED_TAGS.contains(&key.to_lowercase().as_str()) {
        return PropertyType::Tags;
    }
    let v = value.trim();
    if is_bool(v) {
        PropertyType::Checkbox
    } else if is_iso_datetime(v) {
        PropertyType::Datetime
    } else if is_iso_date(v) {
        PropertyType::Date
    } else if is_number(v) {
        PropertyType::Number
    } else if is_inline_list(v) {
        PropertyType::List
    } else {
        PropertyType::Text
    }
}

/// Тип свойства: ЯВНЫЙ из реестра (приоритет) ИЛИ эвристика по значению.
pub fn resolve_type(reg: &Registry, key: &str, value: &str) -> PropertyType {
    reg.get(key)
        .copied()
        .unwrap_or_else(|| infer_type(key, value))
}

/// Свойство заметки для Properties-панели (PROP-3): плоский frontmatter-скаляр + разрешённый тип-виджет.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteProperty {
    pub key: String,
    pub value: String,
    #[serde(rename = "type")]
    pub ty: PropertyType,
}

/// Свойства заметки: плоские frontmatter-скаляры (`fields`, как отдаёт `parser::parse().fields`) с
/// разрешённым типом (реестр+эвристика). Порядок — как в файле. Чистая (контент уже распарсен).
pub fn note_properties(reg: &Registry, fields: &[(String, String)]) -> Vec<NoteProperty> {
    fields
        .iter()
        .map(|(key, value)| NoteProperty {
            key: key.clone(),
            value: value.clone(),
            ty: resolve_type(reg, key, value),
        })
        .collect()
}

fn registry_path(root: &Path) -> PathBuf {
    root.join(".nexus").join("property-types.json")
}

/// Читает реестр. Нет файла / битый JSON → пустой (fail-safe: работает одна эвристика).
pub fn load(root: &Path) -> Registry {
    match std::fs::read_to_string(registry_path(root)) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "property-types.json битый — реестр сброшен в пустой");
            Registry::new()
        }),
        Err(_) => Registry::new(),
    }
}

/// Пишет реестр атомарно (каталог создаётся).
pub fn save(root: &Path, reg: &Registry) -> std::io::Result<()> {
    let path = registry_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(reg).expect("Registry сериализуем всегда");
    crate::vault::atomic_write_io(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn infer_heuristic_order_and_forced_tags() {
        assert_eq!(infer_type("tags", "что угодно"), PropertyType::Tags);
        assert_eq!(infer_type("aliases", "x"), PropertyType::Tags);
        assert_eq!(infer_type("done", "true"), PropertyType::Checkbox);
        assert_eq!(infer_type("done", "Off"), PropertyType::Checkbox); // bool ДО number
        assert_eq!(infer_type("ts", "2026-06-20T14:30"), PropertyType::Datetime);
        assert_eq!(
            infer_type("ts", "2026-06-20 14:30:00"),
            PropertyType::Datetime
        );
        assert_eq!(infer_type("due", "2026-06-20"), PropertyType::Date);
        assert_eq!(infer_type("priority", "3"), PropertyType::Number);
        assert_eq!(infer_type("priority", "2.5"), PropertyType::Number);
        assert_eq!(infer_type("authors", "[a, b]"), PropertyType::List);
        assert_eq!(infer_type("note", "Привет, мир"), PropertyType::Text); // CSV-текст ≠ список
        assert_eq!(infer_type("status", "todo"), PropertyType::Text);
        assert_eq!(infer_type("due", "скоро"), PropertyType::Text); // не-дата → text
    }

    #[test]
    fn resolve_explicit_overrides_heuristic() {
        let mut reg = Registry::new();
        reg.insert("priority".into(), PropertyType::Text);
        // Значение «3» эвристикой → Number, но явный тип Text из реестра выигрывает.
        assert_eq!(infer_type("priority", "3"), PropertyType::Number);
        assert_eq!(resolve_type(&reg, "priority", "3"), PropertyType::Text);
        // Имя не в реестре → эвристика.
        assert_eq!(resolve_type(&reg, "due", "2026-06-20"), PropertyType::Date);
    }

    #[test]
    fn note_properties_resolves_types_in_order() {
        let mut reg = Registry::new();
        reg.insert("priority".into(), PropertyType::Text); // явный override
        let fields = vec![
            ("status".to_string(), "todo".to_string()),
            ("due".to_string(), "2026-06-20".to_string()),
            ("priority".to_string(), "3".to_string()),
        ];
        let props = note_properties(&reg, &fields);
        assert_eq!(props.len(), 3);
        assert_eq!(props[0].key, "status"); // порядок как в файле
        assert_eq!(props[0].ty, PropertyType::Text);
        assert_eq!(props[1].ty, PropertyType::Date); // эвристика
        assert_eq!(props[2].ty, PropertyType::Text); // реестр > эвристика(Number)
    }

    #[test]
    fn save_then_load_round_trips_and_corrupt_is_empty() {
        let dir = TempDir::new().unwrap();
        let mut reg = Registry::new();
        reg.insert("status".into(), PropertyType::Text);
        reg.insert("done".into(), PropertyType::Checkbox);
        save(dir.path(), &reg).unwrap();
        assert_eq!(load(dir.path()), reg);

        // Нет файла → пустой.
        assert!(load(TempDir::new().unwrap().path()).is_empty());

        // Битый JSON → пустой (fail-safe), не паника.
        let p = registry_path(dir.path());
        std::fs::write(&p, "{ битый").unwrap();
        assert!(load(dir.path()).is_empty());
    }
}
