//! Модель прав плагина и проверка scope (**ADR-002**, §7.2/§7.4/§7.9). Это security-ядро
//! capability-broker'а — «РЕАЛЬНАЯ граница прав». Чистая, исчерпаемо тестируемая логика; рантайм-
//! брокер (сессии по порту, capability-токены, audit, dispatch, MessagePort-iframe) — Ф2-2.
//!
//! Принципы: path-scoped права (`vault:read: ["Notes/**", "!Private/**"]`), а НЕ весь vault; deny
//! (`!`) перекрывает allow; неизвестный метод → Denied (fail-closed); `..` в пути → отказ (защита
//! в глубину поверх `vault::resolve_vault_path`); сеть — только по allowlist; `ai:complete` несёт
//! `local_only`. Identity/токены проверяются рантаймом по порту (§7.9), не из payload.

use serde::Deserialize;

/// Объявленные плагином права (из `manifest.permissions`, §7.2). Отсутствие ключа = право не выдано.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Permissions {
    /// Glob-скоупы чтения vault (`!` — запрет, перекрывает allow).
    #[serde(rename = "vault:read", default)]
    pub vault_read: Vec<String>,
    /// Glob-скоупы записи vault.
    #[serde(rename = "vault:write", default)]
    pub vault_write: Vec<String>,
    /// Право на эмбеддинги.
    #[serde(rename = "ai:embed", default)]
    pub ai_embed: bool,
    /// Право на генерацию (с флагом `local_only`); `true` = без облака.
    #[serde(rename = "ai:complete", default)]
    pub ai_complete: Option<AiComplete>,
    /// Сетевой allowlist (хосты). Пусто = сеть запрещена.
    #[serde(rename = "net", default)]
    pub net: Vec<String>,
    /// Точки расширения UI (`sidebar-right`, `status-bar`, …).
    #[serde(rename = "ui", default)]
    pub ui: Vec<String>,
}

/// `ai:complete` в манифесте: либо `true`/`false`, либо `{ "local_only": bool }` (§7.2).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AiComplete {
    Flag(bool),
    Opts {
        #[serde(default)]
        local_only: bool,
    },
}

impl AiComplete {
    /// Выдано ли право вообще (`false` для `Flag(false)`).
    pub fn granted(&self) -> bool {
        !matches!(self, AiComplete::Flag(false))
    }
    /// Требует ли только локальную модель (запрет тихой отправки в облако).
    pub fn local_only(&self) -> bool {
        matches!(self, AiComplete::Opts { local_only: true })
    }
}

/// Причина отказа брокера (коды как в §7.9 wire-формате).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// Право на метод не объявлено в манифесте.
    NotGranted(&'static str),
    /// Путь вне выданного scope.
    OutOfScope(String),
    /// Хост не в сетевом allowlist.
    HostNotAllowed(String),
    /// Попытка выхода за пределы vault (`..` / абсолютный путь).
    PathEscape(String),
    /// Метод не существует / не поддерживается брокером (fail-closed).
    UnknownMethod(String),
    /// Метод требует аргумент (path/host), которого нет.
    MissingArg(&'static str),
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::NotGranted(m) => write!(f, "право не выдано: {m}"),
            Denied::OutOfScope(p) => write!(f, "путь вне scope: {p}"),
            Denied::HostNotAllowed(h) => write!(f, "хост не в allowlist: {h}"),
            Denied::PathEscape(p) => write!(f, "выход за пределы vault: {p}"),
            Denied::UnknownMethod(m) => write!(f, "неизвестный метод: {m}"),
            Denied::MissingArg(a) => write!(f, "нет аргумента: {a}"),
        }
    }
}

/// Запрос плагина к host-функции (после снятия с порта; identity/cap проверены рантаймом отдельно).
#[derive(Debug, Clone)]
pub struct ApiRequest<'a> {
    pub method: &'a str,
    pub path: Option<&'a str>,
    pub host: Option<&'a str>,
}

impl Permissions {
    /// Главная проверка прав (§7.4 `check_scoped_permission`). Fail-closed: всё, что явно не
    /// разрешено, — `Denied`. НЕ заменяет `vault::resolve_vault_path` (канонизация выполняется в
    /// рантайме до/вместе с этим), но дублирует анти-traversal как защиту в глубину.
    pub fn check(&self, req: &ApiRequest) -> Result<(), Denied> {
        match req.method {
            "vault.readFile" | "vault.listFiles" | "vault.onFileChanged" => {
                self.check_path(&self.vault_read, req, "vault:read")
            }
            "vault.writeFile" => self.check_path(&self.vault_write, req, "vault:write"),
            "ai.embed" | "ai.searchSemantic" => {
                if self.ai_embed {
                    Ok(())
                } else {
                    Err(Denied::NotGranted("ai:embed"))
                }
            }
            "ai.complete" => match &self.ai_complete {
                Some(a) if a.granted() => Ok(()),
                _ => Err(Denied::NotGranted("ai:complete")),
            },
            "net.fetch" => {
                let host = req.host.ok_or(Denied::MissingArg("host"))?;
                if self.net.iter().any(|h| h == host) {
                    Ok(())
                } else {
                    Err(Denied::HostNotAllowed(host.to_string()))
                }
            }
            // Регистрация команды требует объявленной ui-точки `command` (Ф2-3).
            "ui.registerCommand" => {
                if self.ui.iter().any(|p| p == "command") {
                    Ok(())
                } else {
                    Err(Denied::NotGranted("ui:command"))
                }
            }
            // Прочие ui.* (напр. `ui.addTranslations`) — требуют объявленной хотя бы одной ui-точки.
            m if m.starts_with("ui.") => {
                if self.ui.is_empty() {
                    Err(Denied::NotGranted("ui"))
                } else {
                    Ok(())
                }
            }
            other => Err(Denied::UnknownMethod(other.to_string())),
        }
    }

    fn check_path(
        &self,
        rules: &[String],
        req: &ApiRequest,
        perm: &'static str,
    ) -> Result<(), Denied> {
        if rules.is_empty() {
            return Err(Denied::NotGranted(perm));
        }
        let path = req.path.ok_or(Denied::MissingArg("path"))?;
        if is_escaping(path) {
            return Err(Denied::PathEscape(path.to_string()));
        }
        if path_in_scope(rules, path) {
            Ok(())
        } else {
            Err(Denied::OutOfScope(path.to_string()))
        }
    }
}

/// Путь вне vault: абсолютный, с пустым/`.`/`..`-сегментом или backslash (Windows-traversal).
fn is_escaping(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return true;
    }
    path.split('/')
        .any(|seg| seg == ".." || seg == "." || seg.is_empty())
}

/// Путь проходит scope: совпал хотя бы с одним allow-glob И ни с одним deny (`!`)-glob (deny > allow).
pub fn path_in_scope(rules: &[String], path: &str) -> bool {
    let mut allowed = false;
    for rule in rules {
        if let Some(deny) = rule.strip_prefix('!') {
            if glob_match(deny, path) {
                return false; // запрет перекрывает любое разрешение
            }
        } else if glob_match(rule, path) {
            allowed = true;
        }
    }
    allowed
}

/// Сегментный glob по пути (разделитель `/`). `**` — любое число сегментов (включая ноль);
/// `*` — любые символы кроме `/` внутри одного сегмента; прочее — посимвольно. Полное совпадение.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let txt: Vec<&str> = path.split('/').collect();
    seg_match(&pat, &txt)
}

fn seg_match(pat: &[&str], txt: &[&str]) -> bool {
    match pat.split_first() {
        None => txt.is_empty(),
        Some((&"**", rest)) => {
            // `**` поглощает 0..=N сегментов.
            (0..=txt.len()).any(|i| seg_match(rest, &txt[i..]))
        }
        Some((seg, rest)) => match txt.split_first() {
            Some((head, txt_rest)) if wildcard_seg(seg, head) => seg_match(rest, txt_rest),
            _ => false,
        },
    }
}

/// Совпадение одного сегмента с `*`-глобом (без `/`). Классический жадный алгоритм с backtrack.
fn wildcard_seg(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark): (Option<usize>, usize) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// SSRF-защита (AC-SEC-4): указывает ли хост-литерал на приватный/loopback/link-local/metadata-адрес.
/// Для доменных имён возвращает `false` — основной контроль для них это net-allowlist; защита от
/// DNS-rebinding (резолв + проверка адреса) — отдельная доработка. Применяется к `net.fetch` ПОВЕРХ
/// allowlist (даже разрешённый хост не должен указывать внутрь сети/на metadata).
pub fn is_private_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") || h.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local() // 169.254/16, incl. 169.254.169.254 (cloud metadata)
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
        Err(_) => false, // домен — контролируется allowlist
    }
}

/// Cloud-metadata-блок (E7, AC-EGR-12): хост — это `169.254.169.254` (IMDS AWS/GCP/Azure/…).
/// ОТДЕЛЬНЫЙ предикат, НЕ реюз [`is_private_host`]: тот склеивает `{private|loopback|link_local}`
/// в один `bool`, которым нельзя отклонить metadata, не отклонив `192.168.*` (LAN-LLM by design).
/// Точный IP, а не весь `169.254/16`: остальной link-local для ядра решает общая политика; правило
/// «metadata — никогда» применяется ПЕРВЫМ и безусловно (даже к allowlist). Домены (`metadata.google.
/// internal`) не резолвим — DNS-rebinding-гард приходит с web-срезом (ADR-005-ext W-аддендум).
pub fn blocks_cloud_metadata(host: &str) -> bool {
    const METADATA_V4: std::net::Ipv4Addr = std::net::Ipv4Addr::new(169, 254, 169, 254);
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4 == METADATA_V4,
        // `::ffff:169.254.169.254` — тот же адрес в IPv4-mapped-форме (обход через v6-литерал).
        Ok(std::net::IpAddr::V6(v6)) => v6.to_ipv4_mapped() == Some(METADATA_V4),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(json: &str) -> Permissions {
        serde_json::from_str(json).unwrap()
    }
    fn read_req<'a>(path: &'a str) -> ApiRequest<'a> {
        ApiRequest {
            method: "vault.readFile",
            path: Some(path),
            host: None,
        }
    }

    // ── glob ──────────────────────────────────────────────────────────────────────────────────
    #[test]
    fn glob_doublestar_matches_subtree_incl_zero() {
        assert!(glob_match("Notes/**", "Notes/a.md"));
        assert!(glob_match("Notes/**", "Notes/sub/deep/x.md"));
        assert!(glob_match("Notes/**", "Notes")); // ** = ноль сегментов
        assert!(!glob_match("Notes/**", "Other/a.md"));
    }

    #[test]
    fn glob_single_star_is_one_segment_only() {
        assert!(glob_match("Notes/*", "Notes/a.md"));
        assert!(!glob_match("Notes/*", "Notes/sub/a.md")); // * не пересекает '/'
        assert!(glob_match("*.md", "a.md"));
        assert!(!glob_match("*.md", "a.txt"));
        assert!(glob_match("draft-*", "draft-2026"));
    }

    #[test]
    fn glob_exact_and_edge() {
        assert!(glob_match("README.md", "README.md"));
        assert!(!glob_match("README.md", "readme.md")); // регистрозависимо
        assert!(glob_match("**", "anything/at/all.md"));
    }

    // ── scope (deny перекрывает allow) ──────────────────────────────────────────────────────────
    #[test]
    fn scope_deny_overrides_allow_any_order() {
        let r = vec!["Notes/**".into(), "!Notes/Private/**".into()];
        assert!(path_in_scope(&r, "Notes/ok.md"));
        assert!(!path_in_scope(&r, "Notes/Private/secret.md"));
        // обратный порядок правил — тот же результат
        let r2 = vec!["!Notes/Private/**".into(), "Notes/**".into()];
        assert!(!path_in_scope(&r2, "Notes/Private/secret.md"));
        assert!(path_in_scope(&r2, "Notes/ok.md"));
    }

    #[test]
    fn scope_requires_explicit_allow() {
        assert!(!path_in_scope(&[], "Notes/a.md"));
        assert!(!path_in_scope(&["!x/**".into()], "Notes/a.md")); // только deny → ничего не разрешено
    }

    // ── check: vault ────────────────────────────────────────────────────────────────────────────
    #[test]
    fn check_vault_read_scope() {
        let p = perms(r#"{"vault:read":["Notes/**","!Notes/Private/**"]}"#);
        assert!(p.check(&read_req("Notes/a.md")).is_ok());
        assert_eq!(
            p.check(&read_req("Notes/Private/s.md")),
            Err(Denied::OutOfScope("Notes/Private/s.md".into()))
        );
        assert_eq!(
            p.check(&read_req("Other/a.md")),
            Err(Denied::OutOfScope("Other/a.md".into()))
        );
    }

    #[test]
    fn check_write_needs_write_perm_not_read() {
        let p = perms(r#"{"vault:read":["**"]}"#); // только чтение
        let w = ApiRequest {
            method: "vault.writeFile",
            path: Some("Notes/a.md"),
            host: None,
        };
        assert_eq!(p.check(&w), Err(Denied::NotGranted("vault:write")));
    }

    #[test]
    fn check_path_escape_blocked() {
        let p = perms(r#"{"vault:read":["**"]}"#);
        for bad in [
            "../etc/passwd",
            "/etc/passwd",
            "Notes/../../x",
            "a\\b",
            "Notes//x",
            "",
        ] {
            assert!(
                matches!(p.check(&read_req(bad)), Err(Denied::PathEscape(_))),
                "должен быть PathEscape: {bad:?}"
            );
        }
    }

    // ── check: ai / net / ui / unknown ──────────────────────────────────────────────────────────
    #[test]
    fn check_ai_and_local_only() {
        let p = perms(r#"{"ai:embed":true,"ai:complete":{"local_only":true}}"#);
        assert!(p
            .check(&ApiRequest {
                method: "ai.embed",
                path: None,
                host: None
            })
            .is_ok());
        assert!(p
            .check(&ApiRequest {
                method: "ai.complete",
                path: None,
                host: None
            })
            .is_ok());
        assert!(p.ai_complete.as_ref().unwrap().local_only());

        let none = perms(r#"{}"#);
        assert_eq!(
            none.check(&ApiRequest {
                method: "ai.embed",
                path: None,
                host: None
            }),
            Err(Denied::NotGranted("ai:embed"))
        );
    }

    #[test]
    fn check_ai_complete_flag_false_is_not_granted() {
        let p = perms(r#"{"ai:complete":false}"#);
        assert_eq!(
            p.check(&ApiRequest {
                method: "ai.complete",
                path: None,
                host: None
            }),
            Err(Denied::NotGranted("ai:complete"))
        );
    }

    #[test]
    fn check_net_allowlist() {
        let p = perms(r#"{"net":["api.example.com"]}"#);
        let ok = ApiRequest {
            method: "net.fetch",
            path: None,
            host: Some("api.example.com"),
        };
        let bad = ApiRequest {
            method: "net.fetch",
            path: None,
            host: Some("evil.com"),
        };
        assert!(p.check(&ok).is_ok());
        assert_eq!(
            p.check(&bad),
            Err(Denied::HostNotAllowed("evil.com".into()))
        );
    }

    #[test]
    fn check_unknown_method_fail_closed() {
        let p = perms(r#"{"vault:read":["**"]}"#);
        assert!(matches!(
            p.check(&ApiRequest {
                method: "vault.deleteEverything",
                path: Some("x"),
                host: None
            }),
            Err(Denied::UnknownMethod(_))
        ));
    }

    #[test]
    fn check_register_command_needs_ui_point() {
        let req = ApiRequest {
            method: "ui.registerCommand",
            path: None,
            host: None,
        };
        // Без ui-точки `command` — отказ; с ней — ок.
        assert_eq!(
            perms(r#"{"vault:read":["**"]}"#).check(&req),
            Err(Denied::NotGranted("ui:command"))
        );
        assert!(perms(r#"{"ui":["command"]}"#).check(&req).is_ok());
    }

    #[test]
    fn check_other_ui_method_needs_some_ui_point() {
        let req = ApiRequest {
            method: "ui.addTranslations",
            path: None,
            host: None,
        };
        // Без объявленной ui-точки — отказ; с любой — ок.
        assert_eq!(perms(r#"{}"#).check(&req), Err(Denied::NotGranted("ui")));
        assert!(perms(r#"{"ui":["command"]}"#).check(&req).is_ok());
    }

    #[test]
    fn ssrf_blocks_private_loopback_metadata() {
        for h in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.5",
            "169.254.169.254", // cloud metadata
            "0.0.0.0",
            "::1",
            "[::1]",
            "fe80::1",
            "fc00::1",
        ] {
            assert!(is_private_host(h), "{h} должен быть заблокирован (SSRF)");
        }
        for h in ["example.com", "93.184.216.34", "api.openai.com", "8.8.8.8"] {
            assert!(!is_private_host(h), "{h} НЕ должен блокироваться");
        }
    }

    /// E7/AC-EGR-12: metadata-предикат бьёт ТОЧНО по 169.254.169.254 (вкл. IPv4-mapped-v6-форму),
    /// не задевая LAN/loopback (их судьбу решает общая политика) и прочий link-local.
    #[test]
    fn cloud_metadata_predicate_is_exact() {
        for h in [
            "169.254.169.254",
            " 169.254.169.254 ",
            "::ffff:169.254.169.254",
            "[::ffff:169.254.169.254]",
        ] {
            assert!(
                blocks_cloud_metadata(h),
                "{h} — cloud metadata, блок всегда"
            );
        }
        for h in [
            "192.168.1.5", // LAN — НЕ metadata (E7: «LAN ок, metadata — никогда»)
            "127.0.0.1",   // loopback
            "169.254.0.1", // link-local, но не metadata-IP
            "example.com", // домен — без резолва (web-срез)
            "metadata.google.internal",
            "8.8.8.8",
        ] {
            assert!(
                !blocks_cloud_metadata(h),
                "{h} НЕ должен попадать под metadata-блок"
            );
        }
    }

    #[test]
    fn empty_permissions_deny_all_vault() {
        let p = Permissions::default();
        assert_eq!(
            p.check(&read_req("Notes/a.md")),
            Err(Denied::NotGranted("vault:read"))
        );
    }
}
