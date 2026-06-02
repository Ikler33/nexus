//! Plugin loader (минимум, Ф0-13): чтение `manifest.json` + проверка совместимости версии
//! Plugin API. **С-13**: `min_api_version` — это МИНИМУМ версии ядра (не `"^1.0"`), поэтому
//! несовместимость ловится ДО загрузки. Broker, исполнение JS/WASM, права — Фаза 2 (§7).

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Версия Plugin API ядра (Приложение B: v1.0 — первая).
pub const CORE_API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

/// Версия API вида `major.minor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u32,
    pub minor: u32,
}

impl ApiVersion {
    /// Парсит `"1.2"` / `"1"`; отвергает каретку и прочий не-числовой ввод (С-13).
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = match parts.next() {
            Some(m) => m.parse().ok()?,
            None => 0,
        };
        if parts.next().is_some() {
            return None; // больше двух компонент — не наш формат
        }
        Some(ApiVersion { major, minor })
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Манифест плагина (подмножество §7.2, нужное для загрузки/совместимости в Ф0).
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub min_api_version: String,
    #[serde(default)]
    pub max_api_version: Option<String>,
    #[serde(default)]
    pub entry: Option<String>,
}

/// Ошибки загрузки/совместимости плагина.
#[derive(Debug, Error)]
pub enum PluginError {
    #[error("manifest: {0}")]
    Parse(String),
    #[error("неверный формат версии API: {0}")]
    BadVersion(String),
    #[error("плагину '{id}' нужно API ≥ {min}, ядро — {core}")]
    TooNew {
        id: String,
        min: ApiVersion,
        core: ApiVersion,
    },
    #[error("плагину '{id}' нужно API ≤ {max}, ядро — {core}")]
    TooOld {
        id: String,
        max: ApiVersion,
        core: ApiVersion,
    },
}

/// Разбирает manifest (без проверки совместимости).
pub fn parse_manifest(json: &str) -> Result<PluginManifest, PluginError> {
    serde_json::from_str(json).map_err(|e| PluginError::Parse(e.to_string()))
}

/// Проверяет, что версия ядра попадает в `[min_api_version, max_api_version]` (С-13).
pub fn check_compatibility(m: &PluginManifest, core: ApiVersion) -> Result<(), PluginError> {
    let min = ApiVersion::parse(&m.min_api_version)
        .ok_or_else(|| PluginError::BadVersion(m.min_api_version.clone()))?;
    if core < min {
        return Err(PluginError::TooNew {
            id: m.id.clone(),
            min,
            core,
        });
    }
    if let Some(max_raw) = &m.max_api_version {
        let max =
            ApiVersion::parse(max_raw).ok_or_else(|| PluginError::BadVersion(max_raw.clone()))?;
        if core > max {
            return Err(PluginError::TooOld {
                id: m.id.clone(),
                max,
                core,
            });
        }
    }
    Ok(())
}

/// Разбирает manifest и проверяет совместимость с версией ядра.
pub fn load_manifest(json: &str, core: ApiVersion) -> Result<PluginManifest, PluginError> {
    let manifest = parse_manifest(json)?;
    check_compatibility(&manifest, core)?;
    Ok(manifest)
}

/// Статус установленного плагина для UI (Plugin Manager появится в Ф2).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub dir: String,
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub compatible: bool,
    pub error: Option<String>,
}

/// Сканирует `plugins_dir` (`.nexus/plugins/*/manifest.json`) и возвращает статус каждого.
/// Не исполняет код (Ф2): только манифесты и совместимость. Несовместимые отмечаются ошибкой.
pub fn scan_plugins(plugins_dir: &Path) -> Vec<PluginInfo> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return out; // нет каталога плагинов — пусто
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let manifest_path = path.join("manifest.json");
        let info = match std::fs::read_to_string(&manifest_path) {
            Err(e) => PluginInfo {
                dir,
                id: None,
                name: None,
                version: None,
                compatible: false,
                error: Some(format!("нет manifest.json: {e}")),
            },
            Ok(json) => match parse_manifest(&json) {
                Err(e) => PluginInfo {
                    dir,
                    id: None,
                    name: None,
                    version: None,
                    compatible: false,
                    error: Some(e.to_string()),
                },
                Ok(m) => {
                    let compat = check_compatibility(&m, CORE_API_VERSION);
                    PluginInfo {
                        dir,
                        id: Some(m.id),
                        name: Some(m.name),
                        version: Some(m.version),
                        compatible: compat.is_ok(),
                        error: compat.err().map(|e| e.to_string()),
                    }
                }
            },
        };
        out.push(info);
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const V1_0: ApiVersion = ApiVersion { major: 1, minor: 0 };

    fn manifest(min: &str, max: Option<&str>) -> String {
        let max_field = max
            .map(|m| format!(",\"max_api_version\":\"{m}\""))
            .unwrap_or_default();
        format!(
            "{{\"id\":\"p\",\"name\":\"P\",\"version\":\"1.0.0\",\"min_api_version\":\"{min}\"{max_field}}}"
        )
    }

    #[test]
    fn loads_compatible_manifest() {
        let m = load_manifest(&manifest("1.0", None), V1_0).unwrap();
        assert_eq!(m.id, "p");
    }

    #[test]
    fn rejects_plugin_needing_newer_core() {
        // С-13: плагин под API 1.2 не грузится на ядре 1.0 (а не «любой 1.x»).
        let err = load_manifest(&manifest("1.2", None), V1_0).unwrap_err();
        assert!(matches!(err, PluginError::TooNew { .. }), "{err}");
    }

    #[test]
    fn rejects_plugin_with_too_low_max() {
        let err = load_manifest(&manifest("1.0", Some("0.9")), V1_0).unwrap_err();
        assert!(matches!(err, PluginError::TooOld { .. }), "{err}");
    }

    #[test]
    fn accepts_within_min_max_range() {
        assert!(load_manifest(&manifest("1.0", Some("2.0")), V1_0).is_ok());
    }

    #[test]
    fn caret_range_is_rejected_not_treated_as_any() {
        // "^1.0" — не наш формат (С-13): отвергаем, а не трактуем как «любой 1.x».
        let err = load_manifest(&manifest("^1.0", None), V1_0).unwrap_err();
        assert!(matches!(err, PluginError::BadVersion(_)), "{err}");
    }

    #[test]
    fn bad_json_is_parse_error() {
        let err = load_manifest("{ not json", V1_0).unwrap_err();
        assert!(matches!(err, PluginError::Parse(_)), "{err}");
    }

    #[test]
    fn scan_reports_compatible_and_incompatible() {
        let dir = TempDir::new().unwrap();
        let plugins = dir.path();
        fs::create_dir(plugins.join("good")).unwrap();
        fs::write(plugins.join("good/manifest.json"), manifest("1.0", None)).unwrap();
        fs::create_dir(plugins.join("future")).unwrap();
        fs::write(plugins.join("future/manifest.json"), manifest("1.5", None)).unwrap();
        fs::create_dir(plugins.join("broken")).unwrap();
        fs::write(plugins.join("broken/manifest.json"), "{ bad").unwrap();

        let infos = scan_plugins(plugins);
        assert_eq!(infos.len(), 3);
        let by_dir = |d: &str| infos.iter().find(|i| i.dir == d).unwrap();
        assert!(by_dir("good").compatible);
        assert!(!by_dir("future").compatible && by_dir("future").error.is_some());
        assert!(!by_dir("broken").compatible && by_dir("broken").id.is_none());
    }

    #[test]
    fn scan_missing_dir_is_empty() {
        let dir = TempDir::new().unwrap();
        assert!(scan_plugins(&dir.path().join("nope")).is_empty());
    }
}
