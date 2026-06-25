//! Команды раздела настроек «AI / Модели» (кросс-план #11): чтение/запись `.nexus/local.json` из UI
//! (без ручного редактирования файла) + проверка связи + ГОРЯЧЕЕ применение chat-провайдера.
//!
//! Chat применяется немедленно (он stateless per-request — команда `chat_rag` читает `ctx.ai.chat` из
//! state на каждый запрос). Embedding НЕ применяется на лету: на нём висит фоновый индексатор
//! (свой клон embedder + общий `vectors`), безопасный hot-swap требует остановки/респавна индексатора —
//! отдельный срез (#11b-full). Поэтому при смене embedding возвращаем `embedding_changed=true` → UI
//! просит перезапуск (на переоткрытии vault конфиг перечитается и переиндексация пройдёт).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::ai::{AiError, ChatConfig, ChatProvider, LocalConfig, OpenAiChatProvider};
use crate::error::{AppError, AppResult};
use crate::net::{EgressFeature, GuardedClient, NetError, RunCtx};
use crate::state::AppState;

/// Эндпоинт (chat/embedding) в форме настроек.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointDto {
    pub url: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// CONN-4: подключение агента (`ai.connection`) для UI-селектора. `mode` нормализован
/// (`embedded`|`local`|`remote`); `socket` — путь AF_UNIX для local (None → дефолт `<vault>/.nexus/
/// agentd.sock`). `url`/`auth_ref` (CONN-3 remote) НЕ сюда.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConnectionDto {
    pub mode: String,
    pub socket: Option<String>,
    /// ACP-1b: команда спавна ACP-агента как ОДНА строка (UI редактирует как командную строку, бэк
    /// парсит в argv по пробелам). `None`/пусто для не-acp. Зеркалит `ai.connection.acp_command` (Vec).
    pub acp_command: Option<String>,
    /// ACP-1b: cwd ACP-сессии (`None` → корень vault).
    pub acp_cwd: Option<String>,
    /// ACP-REMOTE-SSH: транспорт ACP (`"local"` — спавн `acp_command`; `"ssh"` — сборка ssh-команды).
    /// `None`/пусто → как `"local"`. Зеркалит `ai.connection.acp_transport`.
    pub acp_transport: Option<String>,
    /// ACP-REMOTE-SSH (ssh): `"user@host"`. Зеркалит `ai.connection.acp_ssh_host`.
    pub acp_ssh_host: Option<String>,
    /// ACP-REMOTE-SSH (ssh): путь к приватному ключу (опц.; пусто → ключ по умолчанию). `acp_ssh_key`.
    pub acp_ssh_key: Option<String>,
    /// ACP-REMOTE-SSH (ssh): команда запуска ACP-агента НА ХОСТЕ как ОДНА строка (split по пробелам на
    /// бэке). `acp_remote_command`.
    pub acp_remote_command: Option<String>,
}

impl Default for AgentConnectionDto {
    fn default() -> Self {
        Self {
            mode: "embedded".into(),
            socket: None,
            acp_command: None,
            acp_cwd: None,
            acp_transport: None,
            acp_ssh_host: None,
            acp_ssh_key: None,
            acp_remote_command: None,
        }
    }
}

/// Текущая AI-конфигурация для префилла формы.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigDto {
    pub chat: Option<EndpointDto>,
    pub embedding: Option<EndpointDto>,
    /// Утилитарная мелкая модель (`ai.fast`, напр. Qwen3-4B) — inline/судья/сводка reasoning/новости.
    pub fast: Option<EndpointDto>,
    /// CONN-4 `ai.connection`: режим подключения агента (embedded|local|remote) + сокет для local.
    pub connection: AgentConnectionDto,

    // --- Agent-флаги в `.nexus/local.json`. ПОСЛЕ AGENT-0.2/0.6 десктопный `agent_run` ЧИТАЕТ часть из
    // них рантаймом (`agent_actuator_enabled` / `ai.web` / `ai.agent_skills_dir`) — тогглы управляют И
    // десктоп-агентом вкладки Castor, И headless `nexus-agentd`. Автономию прогона десктоп всё ещё берёт
    // per-run из UI (`stores/agent.ts`); `ai.agent_autonomy` — дефолт-постура headless-коннектора.
    /// `ai.agent_autonomy` (`"confirm"`|`"auto"`): дефолт-постура headless-коннектора. `None` → confirm.
    pub agent_autonomy: Option<String>,
    /// `ai.agent_actuator_enabled`: мастер-свитч РЕАЛЬНЫХ действий агента в vault (создать/править
    /// заметку через approval-гейт). OFF (дефолт) → инструменты-заглушки, vault не трогается. Читается
    /// и десктоп-agent_run, и agentd.
    pub agent_actuator_enabled: bool,
    /// `ai.sandbox_enabled`: мастер-свитч OS-песочницы (Linux-only). Предпосылка для shell-exec.
    pub sandbox_enabled: bool,
    /// `ai.shell_enable`: host-exec в песочнице (Confirm, НИКОГДА Auto). Требует sandbox_enabled + Linux.
    pub shell_enable: bool,
    /// `ai.web.allow_public_fetch`: снимает allowlist с агентского `web.fetch` (публичный egress).
    pub web_allow_public_fetch: bool,
    /// W-10 `ai.skills.learning_enabled`: owner-gated мастер-свитч самообучения (агент авторствует
    /// навыки через гейт Confirm-never-Auto). Default-OFF — skill.save HardBlocked, пока выключен.
    pub skills_learning_enabled: bool,
    /// W-10 `ai.agent_skills_dir`: каталог SKILL.md (относительно vault или абсолютный). `None` — навыков
    /// нет (агент без скиллов, без регрессии).
    pub agent_skills_dir: Option<String>,
    /// W-24 `ai.delegation.enabled`: owner-gated мастер-свитч делегирования субагентам (default-OFF).
    /// OFF → delegate.run структурно отсутствует (без регрессии).
    pub delegation_enabled: bool,
    /// W-25 `ai.research.enabled`: owner-gated мастер-свитч deep-research (default-OFF). research.run
    /// регистрируется лишь при research+delegation+web+actuator (см. session.rs) — иначе инертен.
    pub research_enabled: bool,
    /// Может ли песочница/host-exec В ПРИНЦИПЕ работать на ЭТОЙ платформе (Linux-only). Фронт гейтит
    /// (disabled) тогглы sandbox/shell этим флагом — на macOS/Windows они структурно инертны.
    pub shell_supported: bool,
}

/// Записываемый поднабор agent-флагов (вход/выход `set_agent_flags`). Зеркалит TS `AgentFlagsDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFlagsDto {
    /// `"confirm"`|`"auto"`; иное/`None` → ключ не пишется (дефолт confirm у агентд).
    pub agent_autonomy: Option<String>,
    /// `ai.agent_actuator_enabled`: мастер-свитч реальных vault-действий агента (default-OFF).
    pub agent_actuator_enabled: bool,
    pub sandbox_enabled: bool,
    pub shell_enable: bool,
    pub web_allow_public_fetch: bool,
    /// W-10 `ai.skills.learning_enabled` (owner-gated, default-OFF).
    pub skills_learning_enabled: bool,
    /// W-10 `ai.agent_skills_dir`: каталог навыков (пустая строка/`None` → ключ убирается).
    pub agent_skills_dir: Option<String>,
    /// W-24 `ai.delegation.enabled` (owner-gated, default-OFF).
    pub delegation_enabled: bool,
    /// W-25 `ai.research.enabled` (owner-gated, default-OFF).
    pub research_enabled: bool,
}

/// Может ли host-exec/песочница работать на платформе сборки десктопа (Linux-only — rootless-Podman).
/// Фронт дизейблит sandbox/shell-тогглы, когда `false` (на этом десктопе они не дадут эффекта).
const fn shell_supported() -> bool {
    cfg!(target_os = "linux")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAiResult {
    /// Chat-провайдер применён немедленно (без перезапуска).
    pub chat_applied: bool,
    /// Embedding изменился → нужен перезапуск/переиндексация (индексатор на лету не пере-спавнится).
    pub embedding_changed: bool,
}

/// Применяет chat/embedding к JSON-документу `local.json`, СОХРАНЯЯ прочие ключи (`sync` и т.п.).
/// Возвращает, изменился ли embedding. Чистая — тестируется без `State`.
fn apply_ai(
    doc: &mut serde_json::Value,
    chat: Option<&EndpointDto>,
    embedding: Option<&EndpointDto>,
    fast: Option<&EndpointDto>,
) -> Result<bool, String> {
    if !doc.get("ai").map(|v| v.is_object()).unwrap_or(false) {
        doc["ai"] = serde_json::json!({});
    }
    let old_emb = doc.pointer("/ai/embedding").cloned();
    let ai = doc
        .get_mut("ai")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai не объект")?;
    match chat {
        Some(c) => {
            ai.insert(
                "chat".into(),
                serde_json::to_value(c).map_err(|e| e.to_string())?,
            );
        }
        None => {
            ai.remove("chat");
        }
    }
    match embedding {
        Some(e) => {
            ai.insert(
                "embedding".into(),
                serde_json::to_value(e).map_err(|e| e.to_string())?,
            );
        }
        None => {
            ai.remove("embedding");
        }
    }
    match fast {
        // Пустой URL = убрать секцию (тогда `n` падает на gemma-fast = chat-модель).
        Some(f) if !f.url.trim().is_empty() => {
            ai.insert(
                "fast".into(),
                serde_json::to_value(f).map_err(|e| e.to_string())?,
            );
        }
        _ => {
            ai.remove("fast");
        }
    }
    Ok(doc.pointer("/ai/embedding").cloned() != old_emb)
}

/// Мержит agent-флаги (агентд-only) в JSON `local.json`, СОХРАНЯЯ все прочие ключи (chat/embedding/
/// fast/sync/`web.url`/`web.enabled`/`agent_actuator_enabled`/…). Чистая — тестируется без `State`.
///
/// - `agent_autonomy`: только валидные `"confirm"`/`"auto"` пишутся; иное/`None` → ключ убирается
///   (агентд дефолтит на confirm — SAFE).
/// - `web.allow_public_fetch`: пишется ВНУТРЬ существующего/нового `ai.web` БЕЗ затирания `url`/`enabled`.
///   Новый `ai.web` создаётся лишь при `true` (с пустым `url` он инертен; парсится — `WebConfig.url`
///   имеет `#[serde(default)]`). При `false` без существующего `ai.web` — no-op (не плодим шум-ключи).
fn apply_agent_flags(doc: &mut serde_json::Value, flags: &AgentFlagsDto) -> Result<(), String> {
    if !doc.get("ai").map(|v| v.is_object()).unwrap_or(false) {
        doc["ai"] = serde_json::json!({});
    }
    let ai = doc
        .get_mut("ai")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai не объект")?;

    match flags.agent_autonomy.as_deref() {
        Some(v @ ("confirm" | "auto")) => {
            ai.insert("agent_autonomy".into(), serde_json::Value::String(v.into()));
        }
        _ => {
            ai.remove("agent_autonomy");
        }
    }
    // Мастер-свитч реальных vault-действий агента (default-OFF). Читает и десктоп-agent_run, и agentd.
    ai.insert(
        "agent_actuator_enabled".into(),
        serde_json::Value::Bool(flags.agent_actuator_enabled),
    );
    ai.insert(
        "sandbox_enabled".into(),
        serde_json::Value::Bool(flags.sandbox_enabled),
    );
    // КОГЕРЕНТНОСТЬ (fail-closed на trust-boundary, не только в UI): shell-exec невозможен без
    // песочницы → НИКОГДА не персистим `shell_enable=true` при `sandbox_enabled=false`. Прямой вызов
    // команды (минуя UI) не сможет записать инкогерентную пару. (Агентд и так fail-closed классификацией,
    // но конфиг тоже держим консистентным.)
    ai.insert(
        "shell_enable".into(),
        serde_json::Value::Bool(flags.shell_enable && flags.sandbox_enabled),
    );

    // web.allow_public_fetch: трогаем ai.web лишь если флаг true ИЛИ ai.web уже есть (иначе no-op).
    let web_is_obj = ai.get("web").map(|v| v.is_object()).unwrap_or(false);
    if flags.web_allow_public_fetch || web_is_obj {
        if !web_is_obj {
            ai.insert("web".into(), serde_json::json!({}));
        }
        let web = ai
            .get_mut("web")
            .and_then(|v| v.as_object_mut())
            .ok_or("ai.web не объект")?;
        web.insert(
            "allow_public_fetch".into(),
            serde_json::Value::Bool(flags.web_allow_public_fetch),
        );
    }

    // W-10 `ai.skills.learning_enabled` (owner-gated): пишем в объект `ai.skills` (создаём при нужде,
    // как `ai.web`). Default-OFF не меняем — флип это явное действие владельца из UI.
    if !ai.get("skills").map(|v| v.is_object()).unwrap_or(false) {
        ai.insert("skills".into(), serde_json::json!({}));
    }
    ai.get_mut("skills")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai.skills не объект")?
        .insert(
            "learning_enabled".into(),
            serde_json::Value::Bool(flags.skills_learning_enabled),
        );

    // W-24 `ai.delegation.enabled` (owner-gated): пишем в объект `ai.delegation` (создаём при нужде).
    // Капы (max_depth/fanout/total) НЕ трогаем — берутся из DelegationConfig::default при отсутствии.
    if !ai.get("delegation").map(|v| v.is_object()).unwrap_or(false) {
        ai.insert("delegation".into(), serde_json::json!({}));
    }
    ai.get_mut("delegation")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai.delegation не объект")?
        .insert(
            "enabled".into(),
            serde_json::Value::Bool(flags.delegation_enabled),
        );

    // W-25 `ai.research.enabled` (owner-gated): пишем в объект `ai.research` (создаём при нужде).
    // Капы (max_rounds/urls/…) НЕ трогаем — берутся из ResearchConfig::default при отсутствии.
    if !ai.get("research").map(|v| v.is_object()).unwrap_or(false) {
        ai.insert("research".into(), serde_json::json!({}));
    }
    ai.get_mut("research")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai.research не объект")?
        .insert(
            "enabled".into(),
            serde_json::Value::Bool(flags.research_enabled),
        );

    // W-10 `ai.agent_skills_dir`: непустой путь → пишем; пусто/None → убираем ключ (без шум-значений).
    match flags.agent_skills_dir.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => {
            ai.insert(
                "agent_skills_dir".into(),
                serde_json::Value::String(dir.to_string()),
            );
        }
        _ => {
            ai.remove("agent_skills_dir");
        }
    }
    Ok(())
}

/// CONN-4: пишет `ai.connection.{mode,socket}` в local.json (создаёт объект при нужде, как `ai.delegation`).
/// `mode` нормализуется: только `embedded|local|remote`, иначе → `embedded` (SAFE-default, как
/// `agent_autonomy`). `socket`: `Some(непустой)` → пишем, `Some("")` → убираем ключ, `None` → НЕ трогаем
/// (смена режима не должна сюрприз-удалять путь). `url`/`auth_ref` (CONN-3 remote) НЕ трогаем. Сохраняет
/// прочие ключи `ai.*`. Чистая — тестируется без `State`.
fn apply_connection(
    doc: &mut serde_json::Value,
    mode: &str,
    socket: Option<&str>,
) -> Result<(), String> {
    if !doc.get("ai").map(|v| v.is_object()).unwrap_or(false) {
        doc["ai"] = serde_json::json!({});
    }
    let ai = doc
        .get_mut("ai")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai не объект")?;
    if !ai.get("connection").map(|v| v.is_object()).unwrap_or(false) {
        ai.insert("connection".into(), serde_json::json!({}));
    }
    let conn = ai
        .get_mut("connection")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai.connection не объект")?;
    // SAFE-default: мусорный режим → embedded (как `agent_autonomy`).
    let norm = match mode {
        "local" => "local",
        "remote" => "remote",
        "acp" => "acp",
        _ => "embedded",
    };
    conn.insert("mode".into(), serde_json::Value::String(norm.into()));
    match socket {
        Some(s) if !s.trim().is_empty() => {
            conn.insert(
                "socket".into(),
                serde_json::Value::String(s.trim().to_string()),
            );
        }
        Some(_) => {
            conn.remove("socket"); // явная очистка пустым
        }
        None => {} // не трогаем существующий socket
    }
    Ok(())
}

/// ACP-1b: парсит командную строку ACP-агента в argv (минимальный сплит по пробелам — БЕЗ shell-quoting;
/// аргументы с пробелами не поддерживаются, см. i18n-хинт). `hermes acp` → `["hermes","acp"]`.
fn parse_argv(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

/// ACP-1b/ACP-REMOTE-SSH: пишет ACP-поля `ai.connection.*` (тот же объект, что `apply_connection`).
/// `acp_command`: `Some(непустой)` → парсим argv и пишем JSON-массивом строк (как ждёт `de_tolerant_string_vec`);
/// `Some("")`/пустой-argv → убираем ключ; `None` → НЕ трогаем. `acp_cwd`/`acp_transport`/`acp_ssh_host`/
/// `acp_ssh_key`/`acp_remote_command`: tolerant-строки (Some-непустой → пишем; Some-пусто → убираем ключ;
/// `None` → не трогаем). Не трогает прочие `ai.connection.*` (mode/socket/url/auth_ref). Чистая.
#[allow(clippy::too_many_arguments)]
fn apply_acp(
    doc: &mut serde_json::Value,
    acp_command: Option<&str>,
    acp_cwd: Option<&str>,
    acp_transport: Option<&str>,
    acp_ssh_host: Option<&str>,
    acp_ssh_key: Option<&str>,
    acp_remote_command: Option<&str>,
) -> Result<(), String> {
    if !doc.get("ai").map(|v| v.is_object()).unwrap_or(false) {
        doc["ai"] = serde_json::json!({});
    }
    let ai = doc
        .get_mut("ai")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai не объект")?;
    if !ai.get("connection").map(|v| v.is_object()).unwrap_or(false) {
        ai.insert("connection".into(), serde_json::json!({}));
    }
    let conn = ai
        .get_mut("connection")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai.connection не объект")?;
    // Ключи snake_case: ConnectionConfig БЕЗ rename_all → десериализует `acp_command`/`acp_cwd`/… (как
    // socket/mode/url). camelCase не прочитался бы (→ None) — round-trip-reject это ловит в тесте.
    // `None` → НЕ трогаем существующую acp_command (смена режима не сюрприз-удаляет).
    if let Some(s) = acp_command {
        let argv = parse_argv(s);
        if argv.is_empty() {
            conn.remove("acp_command"); // пустая команда → очистка
        } else {
            conn.insert(
                "acp_command".into(),
                serde_json::Value::Array(argv.into_iter().map(serde_json::Value::String).collect()),
            );
        }
    }
    // Хелпер для tolerant-строковых ACP-полей (cwd/transport/ssh_host/ssh_key/remote_command):
    // Some(непустой после trim) → пишем (trimmed); Some(пусто) → убираем ключ; None → не трогаем.
    let mut set_opt = |key: &str, val: Option<&str>| match val {
        Some(s) if !s.trim().is_empty() => {
            conn.insert(key.into(), serde_json::Value::String(s.trim().to_string()));
        }
        Some(_) => {
            conn.remove(key);
        }
        None => {}
    };
    set_opt("acp_cwd", acp_cwd);
    set_opt("acp_transport", acp_transport);
    set_opt("acp_ssh_host", acp_ssh_host);
    set_opt("acp_ssh_key", acp_ssh_key);
    set_opt("acp_remote_command", acp_remote_command);
    Ok(())
}

/// W-3: зеркалит web-consent (`enabled` + `url`) в `ai.web` local.json. Веб-инструменты АГЕНТА
/// (`agent_run` читает `ai.web.enabled`+`url`) включаются ТЕМ ЖЕ тогглом, что Home/chat-веб
/// (`websearch.json`) — иначе тоггл писал только websearch.json, а у агента `ai.web` оставался пуст
/// (баг ST-C3/ST-G4: «найти в сети» не работало). Сохраняет прочие ключи `ai.web` (в т.ч.
/// `allow_public_fetch` из `set_agent_flags`). Чистая — тестируется без `State`.
pub(crate) fn apply_web_endpoint(
    doc: &mut serde_json::Value,
    enabled: bool,
    url: &str,
) -> Result<(), String> {
    if !doc.get("ai").map(|v| v.is_object()).unwrap_or(false) {
        doc["ai"] = serde_json::json!({});
    }
    let ai = doc
        .get_mut("ai")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai не объект")?;
    if !ai.get("web").map(|v| v.is_object()).unwrap_or(false) {
        ai.insert("web".into(), serde_json::json!({}));
    }
    let web = ai
        .get_mut("web")
        .and_then(|v| v.as_object_mut())
        .ok_or("ai.web не объект")?;
    web.insert("enabled".into(), serde_json::Value::Bool(enabled));
    web.insert("url".into(), serde_json::Value::String(url.to_string()));
    Ok(())
}

/// W-3: нужно ли зеркалить web-consent в `ai.web` vault (skip-if-equal — без лишних атомарных
/// записей при каждом открытии vault). `cur` = текущее `(enabled, url)` из vault local.json
/// (`None` = секции `ai.web` ещё нет). Пишем, если значения расходятся ИЛИ секции нет, но
/// глобальный consent непустой (web включён / задан url).
pub(crate) fn web_needs_mirror(
    cur: Option<(bool, &str)>,
    want_enabled: bool,
    want_url: &str,
) -> bool {
    match cur {
        Some((en, url)) => en != want_enabled || url != want_url,
        None => want_enabled || !want_url.is_empty(),
    }
}

/// W-3: применяет web-endpoint к vault `local.json` (read-modify-atomic-write, как `set_ai_config`).
/// Нет открытого vault → тихий no-op (агенту всё равно нужен открытый vault). Зовётся из
/// `set_websearch_config` — один тоггл «Веб» кормит и Home/chat (websearch.json), и агента (ai.web).
pub(crate) async fn mirror_web_to_vault(
    state: &AppState,
    enabled: bool,
    url: &str,
) -> AppResult<()> {
    let root = match state.vault().await {
        Ok(v) => v.root.clone(),
        Err(_) => return Ok(()),
    };
    let dir = root.join(".nexus");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("local.json");
    let raw = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut doc: serde_json::Value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("local.json не JSON: {e}")))?
    };
    apply_web_endpoint(&mut doc, enabled, url)?;
    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| AppError::Msg(e.to_string()))?;
    let path2 = path.clone();
    let bytes = pretty.into_bytes();
    tokio::task::spawn_blocking(move || crate::vault::atomic_write_io(&path2, &bytes))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    Ok(())
}

/// Текущая AI-конфигурация (из `.nexus/local.json`) — для префилла формы настроек.
#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> AppResult<AiConfigDto> {
    let root = state.vault().await?.root.clone();
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .unwrap_or_default();
    if raw.trim().is_empty() {
        // Пустой конфиг → дефолты, но `shell_supported` всё равно отражает платформу (на Linux=true).
        return Ok(AiConfigDto {
            shell_supported: shell_supported(),
            ..Default::default()
        });
    }
    let cfg = LocalConfig::parse(&raw).map_err(|e| AppError::Msg(e.to_string()))?;
    Ok(AiConfigDto {
        chat: cfg.ai.chat.map(|c| EndpointDto {
            url: c.url,
            model: c.model,
        }),
        embedding: cfg.ai.embedding.map(|e| EndpointDto {
            url: e.url,
            model: e.model,
        }),
        fast: cfg.ai.fast.map(|f| EndpointDto {
            url: f.url,
            model: f.model,
        }),
        agent_autonomy: cfg.ai.agent_autonomy.clone(),
        agent_actuator_enabled: cfg.ai.agent_actuator_enabled,
        sandbox_enabled: cfg.ai.sandbox_enabled,
        shell_enable: cfg.ai.shell_enable,
        web_allow_public_fetch: cfg
            .ai
            .web
            .as_ref()
            .map(|w| w.allow_public_fetch)
            .unwrap_or(false),
        skills_learning_enabled: cfg.ai.skills.learning_enabled,
        agent_skills_dir: cfg.ai.agent_skills_dir.clone(),
        delegation_enabled: cfg.ai.delegation.enabled,
        research_enabled: cfg.ai.research.enabled,
        connection: AgentConnectionDto {
            mode: match cfg.ai.connection.mode() {
                nexus_core::ai::ConnectionMode::Embedded => "embedded",
                nexus_core::ai::ConnectionMode::Local => "local",
                nexus_core::ai::ConnectionMode::Remote => "remote",
                nexus_core::ai::ConnectionMode::Acp => "acp",
            }
            .into(),
            socket: cfg.ai.connection.socket.clone(),
            // ACP-1b: Vec<String> argv → одна командная строка для UI (join по пробелу).
            acp_command: cfg.ai.connection.acp_command.as_ref().map(|v| v.join(" ")),
            acp_cwd: cfg.ai.connection.acp_cwd.clone(),
            acp_transport: cfg.ai.connection.acp_transport.clone(),
            acp_ssh_host: cfg.ai.connection.acp_ssh_host.clone(),
            acp_ssh_key: cfg.ai.connection.acp_ssh_key.clone(),
            acp_remote_command: cfg.ai.connection.acp_remote_command.clone(),
        },
        shell_supported: shell_supported(),
    })
}

/// Записывает AI-конфиг в `.nexus/local.json` (сохраняя прочие ключи) и ГОРЯЧО применяет chat.
#[tauri::command]
pub async fn set_ai_config(
    state: State<'_, AppState>,
    chat: Option<EndpointDto>,
    embedding: Option<EndpointDto>,
    fast: Option<EndpointDto>,
) -> AppResult<SetAiResult> {
    let root = state.vault().await?.root.clone();
    let dir = root.join(".nexus");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("local.json");
    let raw = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut doc: serde_json::Value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("local.json не JSON: {e}")))?
    };
    // `apply_ai` отдаёт `String`-ошибку (serde) → поднимается как `AppError::Msg` через `From<String>`.
    let embedding_changed = apply_ai(&mut doc, chat.as_ref(), embedding.as_ref(), fast.as_ref())?;
    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| AppError::Msg(e.to_string()))?;
    // Атомарно (blocking → spawn_blocking): обрыв между записью и rename не оставляет усечённый
    // local.json (находка аудита: truncate-then-write мог потерять конфиг ИИ/эгресса).
    let path2 = path.clone();
    let bytes = pretty.clone().into_bytes();
    tokio::task::spawn_blocking(move || crate::vault::atomic_write_io(&path2, &bytes))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;

    // Allowlist эгресса пересобирается из ИТОГОВОГО local.json (E4: явные `ai.*`-хосты; consent на
    // pull-changed URL — срез 2 с персистом политики). Один policy на приложение (AC-EGR-13).
    if let Ok(cfg) = LocalConfig::parse(&pretty) {
        state.egress_policy.set_allowlist(cfg.egress_hosts());
    }
    // INFER-CFG: connect-таймаут хот-пересобранных клиентов берём из СОХРАНЁННОГО конфига (EndpointDto
    // из UI не несёт таймаутов). Дефолт 30с; кастомный `ai.chat.connect_timeout_secs` применится сразу,
    // не дожидаясь переоткрытия vault.
    let saved_cfg = LocalConfig::parse(&pretty).ok();
    let saved_chat = saved_cfg.as_ref().and_then(|cf| cf.ai.chat.clone());
    let chat_connect_timeout = saved_chat
        .as_ref()
        .map(ChatConfig::connect_timeout)
        .unwrap_or_else(|| Duration::from_secs(ChatConfig::DEFAULT_CONNECT_TIMEOUT_SECS));
    // Температура и таймауты стрима/retry хот-провайдеров — из сохранённого `ai.chat` (или дефолты).
    // Все хот-провайдеры бьют по chat-серверу, поэтому единый профиль `saved_chat` корректен.
    let chat_temperature = saved_chat.as_ref().map(ChatConfig::temperature);
    let apply_chat_cfg = |p: OpenAiChatProvider| -> OpenAiChatProvider {
        match saved_chat.as_ref() {
            Some(c) => p
                .with_first_token_timeout(c.first_token_timeout())
                .with_idle_timeout(c.idle_timeout())
                .with_retry_attempts(c.retry_attempts()),
            None => p,
        }
    };

    // Горячее применение chat (stateless per-request → безопасно): пересобираем УЖЕ-guarded клиент
    // от того же policy/audit (AC-EGR-13). Embedding — через перезапуск (cold, на нём индексатор).
    let chat_provider: Option<Arc<dyn ChatProvider>> = match &chat {
        Some(c) => {
            let model = c.model.clone().unwrap_or_else(|| "chat".to_string());
            let guarded = GuardedClient::for_chat(
                state.egress_policy.clone(),
                state.egress_audit.clone(),
                chat_connect_timeout,
            )
            .map_err(AiError::from)?;
            Some(Arc::new(apply_chat_cfg(OpenAiChatProvider::new(
                &guarded,
                EgressFeature::Chat,
                &c.url,
                &model,
                chat_temperature,
            ))))
        }
        None => None,
    };
    // Горячее применение `n` (утилитарная мелкая модель `ai.fast`): задан непустой URL → строим
    // провайдер; иначе fallback на gemma-fast (chat-модель). Так смена сервера/очистка fast в UI
    // сразу чинит новости/дайджест/противоречия/сводку-reasoning (баг 2026-06-11: `ai.fast` оставался
    // на старом мёртвом хосте, эти фичи дохли).
    let fast_url = fast
        .as_ref()
        .map(|f| f.url.trim())
        .filter(|u| !u.is_empty())
        // Пустой fast → утилитарная = chat-модель (gemma-fast).
        .or_else(|| chat.as_ref().map(|c| c.url.as_str()));
    let fast_provider: Option<Arc<dyn ChatProvider>> = match fast_url {
        Some(url) => {
            let model = fast
                .as_ref()
                .and_then(|f| f.model.clone())
                .or_else(|| chat.as_ref().and_then(|c| c.model.clone()))
                .unwrap_or_else(|| "chat".to_string());
            let guarded = GuardedClient::for_chat(
                state.egress_policy.clone(),
                state.egress_audit.clone(),
                chat_connect_timeout,
            )
            .map_err(AiError::from)?;
            // R2 как в open_vault (`build_util_chat`): примитивам CoT не нужен, а на ai.fast может
            // жить reasoning-модель (баг 2026-06-11: gemma12 думала ~40 с над 6-словной сводкой R1).
            Some(Arc::new(
                apply_chat_cfg(OpenAiChatProvider::new(
                    &guarded,
                    EgressFeature::Chat,
                    url,
                    &model,
                    chat_temperature,
                ))
                .without_reasoning(),
            ))
        }
        None => None,
    };
    // gemma-fast (chat_fast, R2 без reasoning на ОСНОВНОЙ модели) пересобираем вместе с chat —
    // иначе после смены chat-URL дайджест бил бы по старому хосту.
    let chat_fast_provider: Option<Arc<dyn ChatProvider>> = match &chat {
        Some(c) => {
            let model = c.model.clone().unwrap_or_else(|| "chat".to_string());
            let guarded = GuardedClient::for_chat(
                state.egress_policy.clone(),
                state.egress_audit.clone(),
                chat_connect_timeout,
            )
            .map_err(AiError::from)?;
            // Хот-апплай #153 забыл R2: после сохранения настроек дайджест становился «думающим»
            // до переоткрытия vault. Теперь зеркалит `build_chat`.
            Some(Arc::new(
                apply_chat_cfg(OpenAiChatProvider::new(
                    &guarded,
                    EgressFeature::Chat,
                    &c.url,
                    &model,
                    chat_temperature,
                ))
                .without_reasoning(),
            ))
        }
        None => None,
    };
    if let Some(ctx) = state.vault.write().await.as_mut() {
        ctx.ai.chat = chat_provider;
        ctx.ai.chat_fast = chat_fast_provider;
        ctx.ai.chat_util = fast_provider;
    }
    Ok(SetAiResult {
        chat_applied: true,
        embedding_changed,
    })
}

/// Персистит agent-флаги (агентд-only) в `.nexus/local.json`, СОХРАНЯЯ прочие ключи. В ОТЛИЧИЕ от
/// `set_ai_config` — НЕ делает hot-apply провайдеров и НЕ пересобирает egress-allowlist: эти флаги
/// читает ТОЛЬКО headless-агентд при своём старте (десктоп их рантаймом не применяет). Мгновенно (как
/// тогглы egress/websearch). Возвращает НОРМАЛИЗОВАННЫЙ набор (невалидная autonomy → `None` = confirm).
#[tauri::command]
pub async fn set_agent_flags(
    state: State<'_, AppState>,
    flags: AgentFlagsDto,
) -> AppResult<AgentFlagsDto> {
    let root = state.vault().await?.root.clone();
    let dir = root.join(".nexus");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("local.json");
    let raw = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut doc: serde_json::Value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("local.json не JSON: {e}")))?
    };
    apply_agent_flags(&mut doc, &flags)?;
    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| AppError::Msg(e.to_string()))?;
    // Атомарно (как set_ai_config): обрыв между write и rename не оставит усечённый local.json.
    let path2 = path.clone();
    let bytes = pretty.into_bytes();
    tokio::task::spawn_blocking(move || crate::vault::atomic_write_io(&path2, &bytes))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    // Эхо НОРМАЛИЗОВАННОГО набора (то, что реально записано): невалидная autonomy → None (confirm),
    // shell_enable когерентен с sandbox (см. apply_agent_flags). Фронт берёт его за источник истины.
    Ok(AgentFlagsDto {
        agent_autonomy: match flags.agent_autonomy.as_deref() {
            Some("confirm") | Some("auto") => flags.agent_autonomy.clone(),
            _ => None,
        },
        agent_actuator_enabled: flags.agent_actuator_enabled,
        sandbox_enabled: flags.sandbox_enabled,
        shell_enable: flags.shell_enable && flags.sandbox_enabled,
        web_allow_public_fetch: flags.web_allow_public_fetch,
        skills_learning_enabled: flags.skills_learning_enabled,
        // Эхо нормализованного пути: пусто → None (ключ не пишется).
        agent_skills_dir: flags
            .agent_skills_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        delegation_enabled: flags.delegation_enabled,
        research_enabled: flags.research_enabled,
    })
}

/// CONN-4: персистит режим подключения агента (`ai.connection.{mode,socket}`) в local.json (сохраняя
/// прочие ключи) и НЕМЕДЛЕННО свопает активный бэкенд (тот же выбор, что `open_vault` — без переоткрытия
/// vault). Возвращает НОРМАЛИЗОВАННЫЙ набор (мусорный mode → embedded). `socket=None` → не трогаем путь.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn set_agent_connection(
    state: State<'_, AppState>,
    mode: String,
    socket: Option<String>,
    // ACP-1b: командная строка + cwd ACP-агента (None → не трогаем).
    acp_command: Option<String>,
    acp_cwd: Option<String>,
    // ACP-REMOTE-SSH: транспорт + ssh-поля (None → не трогаем существующее).
    acp_transport: Option<String>,
    acp_ssh_host: Option<String>,
    acp_ssh_key: Option<String>,
    acp_remote_command: Option<String>,
) -> AppResult<AgentConnectionDto> {
    let root = state.vault().await?.root.clone();
    let dir = root.join(".nexus");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("local.json");
    let raw = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut doc: serde_json::Value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("local.json не JSON: {e}")))?
    };
    apply_connection(&mut doc, &mode, socket.as_deref())?;
    apply_acp(
        &mut doc,
        acp_command.as_deref(),
        acp_cwd.as_deref(),
        acp_transport.as_deref(),
        acp_ssh_host.as_deref(),
        acp_ssh_key.as_deref(),
        acp_remote_command.as_deref(),
    )?;
    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| AppError::Msg(e.to_string()))?;
    let path2 = path.clone();
    let bytes = pretty.clone().into_bytes();
    tokio::task::spawn_blocking(move || crate::vault::atomic_write_io(&path2, &bytes))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    // Немедленный своп бэкенда (CONN-4): тот же хелпер, что в open_vault. ConnectedBackend::new ленив —
    // отсутствие демона НЕ ломает смену режима (соединение откроется на первом прогоне).
    let parsed = LocalConfig::parse(&pretty).ok();
    *state.agent_backend.write().await =
        crate::agent_backend::select_agent_backend(parsed.as_ref(), &root);
    // Эхо нормализованного (что реально записано/распарсено) — фронт берёт за источник истины.
    let echo = parsed
        .as_ref()
        .map(|c| AgentConnectionDto {
            mode: match c.ai.connection.mode() {
                nexus_core::ai::ConnectionMode::Embedded => "embedded",
                nexus_core::ai::ConnectionMode::Local => "local",
                nexus_core::ai::ConnectionMode::Remote => "remote",
                nexus_core::ai::ConnectionMode::Acp => "acp",
            }
            .to_string(),
            socket: c.ai.connection.socket.clone(),
            acp_command: c.ai.connection.acp_command.as_ref().map(|v| v.join(" ")),
            acp_cwd: c.ai.connection.acp_cwd.clone(),
            acp_transport: c.ai.connection.acp_transport.clone(),
            acp_ssh_host: c.ai.connection.acp_ssh_host.clone(),
            acp_ssh_key: c.ai.connection.acp_ssh_key.clone(),
            acp_remote_command: c.ai.connection.acp_remote_command.clone(),
        })
        .unwrap_or_default();
    Ok(echo)
}

/// CONN-4: классификация сокета ДО connect (внятная диагностика). Чистая — тестируется без демона.
#[cfg(unix)]
fn classify_socket(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_socket() => Err(format!(
            "путь {} существует, но это НЕ сокет (проверь ai.connection.socket)",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "agentd не запущен? сокет {} не найден",
            path.display()
        )),
        _ => Ok(()),
    }
}

/// CONN-4/ACP-1b: проверка связи с агент-бэкендом. Ветвится по `ai.connection.mode()`:
/// `local` → handshake `initialize` по AF_UNIX (только Unix); `acp` → спавн ACP-агента подпроцессом +
/// ACP `initialize` + kill; `embedded`/`remote` → проверять нечего. Кросс-платформенная (acp-проба не
/// требует Unix). Возвращает версию протокола или внятную ошибку.
#[tauri::command]
pub async fn test_agent_connection(state: State<'_, AppState>) -> AppResult<String> {
    let root = state.vault().await?.root.clone();
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .unwrap_or_default();
    let cfg = LocalConfig::parse(&raw).unwrap_or_default();
    match cfg.ai.connection.mode() {
        nexus_core::ai::ConnectionMode::Local => probe_local_socket(&cfg, &root).await,
        nexus_core::ai::ConnectionMode::Acp => probe_acp(&cfg, &root).await,
        nexus_core::ai::ConnectionMode::Embedded | nexus_core::ai::ConnectionMode::Remote => Err(
            AppError::Msg("проверка доступна для режимов «локальный» и «ACP-агент»".into()),
        ),
    }
}

/// CONN-4: проба локального agentd по AF_UNIX (`initialize`). Лёгкая: без read-loop/forward-таска.
#[cfg(unix)]
async fn probe_local_socket(cfg: &LocalConfig, root: &std::path::Path) -> AppResult<String> {
    use nexus_core::agent::connect::{connect_unix, RpcMessage, Transport};
    let socket = cfg
        .ai
        .connection
        .socket
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(".nexus").join("agentd.sock"));
    classify_socket(&socket).map_err(AppError::Msg)?;
    let transport = connect_unix(&socket)
        .await
        .map_err(|e| AppError::Msg(format!("agent недоступен на {}: {e}", socket.display())))?;
    transport
        .send(RpcMessage::request(
            1,
            "initialize",
            serde_json::json!({ "supportedVersions": ["1.0"] }),
        ))
        .await
        .map_err(|_| AppError::Msg("не удалось отправить initialize (сокет закрылся)".into()))?;
    let resp = tokio::time::timeout(std::time::Duration::from_secs(5), transport.recv())
        .await
        .map_err(|_| AppError::Msg("таймаут ответа initialize".into()))?
        .ok_or_else(|| AppError::Msg("сокет закрыт без ответа".into()))?;
    match resp {
        RpcMessage::Response { result: Ok(v), .. } => Ok(v
            .get("version")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string()),
        RpcMessage::Response { result: Err(e), .. } => Err(AppError::Msg(format!(
            "agentd ответил ошибкой: {}",
            e.message
        ))),
        other => Err(AppError::Msg(format!("неожиданный ответ: {other:?}"))),
    }
}

/// CONN-4: на не-Unix локальный коннектор (AF_UNIX) недоступен — структурно.
#[cfg(not(unix))]
async fn probe_local_socket(_cfg: &LocalConfig, _root: &std::path::Path) -> AppResult<String> {
    Err(AppError::Msg(
        "локальный коннектор (AF_UNIX) доступен только на Unix".into(),
    ))
}

/// ACP-1b/ACP-REMOTE-SSH: проба ACP-агента — спавн РАЗРЕШЁННОЙ команды (ssh-сборка при
/// `acp_transport="ssh"`, иначе локальный `acp_command`) + ACP `initialize` (10с) + версия. Подпроцесс
/// убивается при дропе (`kill_on_drop`). Кросс-платформенная. Не сконфигурировано / не найден бинарь /
/// провал handshake → внятная ошибка. «Проверить» тестирует ИМЕННО ту команду, что пойдёт в прод.
async fn probe_acp(cfg: &LocalConfig, root: &std::path::Path) -> AppResult<String> {
    use nexus_core::agent::connect::acp::{AcpClient, ACP_PROTOCOL_VERSION};
    use nexus_core::agent::connect::StdioTransport;
    use std::sync::Arc;
    let cmd = cfg.ai.connection.acp_spawn_argv().ok_or_else(|| {
        // Внятная диагностика по транспорту: ssh без host/команды vs local без команды.
        if cfg.ai.connection.acp_transport.as_deref() == Some("ssh") {
            AppError::Msg("ACP не сконфигурирован: укажите хост и команду".into())
        } else {
            AppError::Msg("команда не задана".into())
        }
    })?;
    let cwd = cfg
        .ai
        .connection
        .acp_cwd
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());
    let (program, args) = cmd
        .split_first()
        .expect("acp_spawn_argv непустой (Some → ≥1 элемент)");
    let transport = StdioTransport::spawn(program, args, &cwd)
        .await
        .map_err(|e| AppError::Msg(format!("ACP-агент не запустился (`{program}`): {e}")))?;
    let (client, _updates, _perms) = AcpClient::new(Arc::new(transport));
    let res = client
        .request(
            "initialize",
            serde_json::json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false
                }
            }),
            Some(std::time::Duration::from_secs(10)),
        )
        .await
        .map_err(|e| AppError::Msg(format!("ACP initialize не прошёл: {}", e.message)))?;
    let v = res
        .get("protocolVersion")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    Ok(format!("ACP v{v}"))
    // client + transport дропаются здесь → read-loop abort + kill_on_drop убивает подпроцесс.
}

/// Проверка связи с LLM-эндпоинтом: пробный GET `/v1/models` (OpenAI-совместимо). Любой ответ сервера →
/// достижим; сетевая ошибка → нет. Через [`GuardedClient`] с `Feature::Probe` (AC-EGR-6): url с фронта
/// проверяется политикой ДО сети — «первый egress-вектор» (произвольный GET из доверенного ядра) закрыт.
#[tauri::command]
pub async fn test_ai_connection(state: State<'_, AppState>, url: String) -> AppResult<()> {
    let probe = GuardedClient::for_probe(
        state.egress_policy.clone(),
        state.egress_audit.clone(),
        Duration::from_secs(5),
    )
    .map_err(AiError::from)?;
    probe_endpoint(&probe, &url).await
}

/// Тестируемое ядро probe (команда — тонкая обёртка над managed state). Отказ политики →
/// типизированный [`AiError::Denied`] (НЕ reqwest-строка); сетевые ошибки — текстом, как раньше
/// (i18n-канал — AC-EGR-14, фронт-срез).
async fn probe_endpoint(probe: &GuardedClient, url: &str) -> AppResult<()> {
    let target = format!("{}/v1/models", crate::ai::api_base(url));
    // «Проверить связь» — вне прогона агента → RunCtx::NONE.
    match probe.get(&target, EgressFeature::Probe, RunCtx::NONE).await {
        Ok(_) => Ok(()),
        Err(NetError::Denied(d)) => Err(AiError::Denied(d).into()),
        Err(NetError::BadUrl) => Err(AppError::Msg("некорректный URL".into())),
        Err(NetError::Http(e)) => Err(AppError::Msg(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_ai_sets_fields_preserves_others_and_detects_embedding_change() {
        let mut doc = serde_json::json!({ "sync": { "remote": "x" } });
        let chat = EndpointDto {
            url: "http://h:8080".into(),
            model: Some("gemma-4-26B-A4B-it".into()),
        };
        let emb = EndpointDto {
            url: "http://192.168.0.29:8083".into(),
            model: Some("bge-m3".into()),
        };
        let fast = EndpointDto {
            url: "http://h:8084".into(),
            model: Some("qwen3-4b".into()),
        };
        let changed = apply_ai(&mut doc, Some(&chat), Some(&emb), Some(&fast)).unwrap();
        assert!(changed, "embedding появился → изменился");
        assert_eq!(doc.pointer("/ai/chat/url").unwrap(), "http://h:8080");
        assert_eq!(doc.pointer("/ai/embedding/model").unwrap(), "bge-m3");
        assert_eq!(doc.pointer("/ai/fast/url").unwrap(), "http://h:8084");
        assert_eq!(
            doc.pointer("/sync/remote").unwrap(),
            "x",
            "прочие ключи сохранены"
        );

        // Повторно тот же embedding → НЕ изменился; убрать chat → удаляется;
        // пустой fast-URL → секция fast убирается (fallback на gemma-fast).
        let empty_fast = EndpointDto {
            url: "  ".into(),
            model: None,
        };
        let changed2 = apply_ai(&mut doc, None, Some(&emb), Some(&empty_fast)).unwrap();
        assert!(!changed2, "embedding тот же");
        assert!(doc.pointer("/ai/chat").is_none(), "chat=None удаляет ключ");
        assert!(
            doc.pointer("/ai/fast").is_none(),
            "пустой fast-URL удаляет секцию"
        );
    }

    fn flags(
        autonomy: Option<&str>,
        sandbox: bool,
        shell: bool,
        public_fetch: bool,
    ) -> AgentFlagsDto {
        AgentFlagsDto {
            agent_autonomy: autonomy.map(str::to_string),
            agent_actuator_enabled: false, // отдельный тест ниже проверяет actuator-ключ
            sandbox_enabled: sandbox,
            shell_enable: shell,
            web_allow_public_fetch: public_fetch,
            skills_learning_enabled: false,
            agent_skills_dir: None,
            delegation_enabled: false,
            research_enabled: false,
        }
    }

    /// W-10: `apply_agent_flags` пишет `ai.skills.learning_enabled` (создаёт `ai.skills`) +
    /// `ai.agent_skills_dir` (trim; пусто → ключ убирается). Round-trip через `LocalConfig`.
    #[test]
    fn apply_agent_flags_writes_skills_learning_and_dir() {
        let mut doc = serde_json::json!({});
        let mut f = flags(Some("confirm"), false, false, false);
        f.skills_learning_enabled = true;
        f.agent_skills_dir = Some("  .nexus/skills  ".into());
        apply_agent_flags(&mut doc, &f).unwrap();
        assert_eq!(doc.pointer("/ai/skills/learning_enabled").unwrap(), true);
        assert_eq!(
            doc.pointer("/ai/agent_skills_dir").unwrap(),
            ".nexus/skills"
        );
        let cfg = crate::ai::LocalConfig::parse(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert!(cfg.ai.skills.learning_enabled);
        assert_eq!(cfg.ai.agent_skills_dir.as_deref(), Some(".nexus/skills"));

        // Пустой/пробельный dir → ключ убирается (без шум-значений).
        f.agent_skills_dir = Some("   ".into());
        apply_agent_flags(&mut doc, &f).unwrap();
        assert!(doc.pointer("/ai/agent_skills_dir").is_none());
        assert_eq!(doc.pointer("/ai/skills/learning_enabled").unwrap(), true);
    }

    /// W-24: `apply_agent_flags` пишет `ai.delegation.enabled` (создаёт `ai.delegation`), default-OFF
    /// не трогает капы. Round-trip через `LocalConfig`.
    #[test]
    fn apply_agent_flags_writes_delegation_enabled() {
        let mut doc = serde_json::json!({ "ai": { "chat": { "url": "http://h:8080" } } });
        let mut f = flags(Some("confirm"), false, false, false);
        f.delegation_enabled = true;
        apply_agent_flags(&mut doc, &f).unwrap();
        assert_eq!(doc.pointer("/ai/delegation/enabled").unwrap(), true);
        // Round-trip: парсится без коррапта и enabled виден в конфиге.
        let cfg = nexus_core::ai::LocalConfig::parse(&doc.to_string()).unwrap();
        assert!(cfg.ai.delegation.enabled);
    }

    /// W-25: `apply_agent_flags` пишет `ai.research.enabled` (создаёт `ai.research`), default-OFF не
    /// трогает капы. Round-trip через `LocalConfig`.
    #[test]
    fn apply_agent_flags_writes_research_enabled() {
        let mut doc = serde_json::json!({ "ai": { "chat": { "url": "http://h:8080" } } });
        let mut f = flags(Some("confirm"), false, false, false);
        f.research_enabled = true;
        apply_agent_flags(&mut doc, &f).unwrap();
        assert_eq!(doc.pointer("/ai/research/enabled").unwrap(), true);
        let cfg = nexus_core::ai::LocalConfig::parse(&doc.to_string()).unwrap();
        assert!(cfg.ai.research.enabled);
    }

    /// CONN-4: `apply_connection` пишет `ai.connection.{mode,socket}`, СОХРАНЯЯ прочие ключи; round-trip
    /// через `LocalConfig`. Мусорный mode → embedded; пустой socket → ключ убран; `url`/`auth_ref` целы.
    #[test]
    fn apply_connection_round_trips_mode_and_socket() {
        // local + socket, при существующих ai.chat и ai.connection.url (CONN-3) — не трогаем url.
        let mut doc = serde_json::json!({
            "ai": { "chat": { "url": "http://h:8080" }, "connection": { "url": "wss://x", "auth_ref": "k" } }
        });
        apply_connection(&mut doc, "local", Some("/tmp/a.sock")).unwrap();
        assert_eq!(doc.pointer("/ai/connection/mode").unwrap(), "local");
        assert_eq!(doc.pointer("/ai/connection/socket").unwrap(), "/tmp/a.sock");
        assert_eq!(doc.pointer("/ai/connection/url").unwrap(), "wss://x"); // CONN-3 не тронут
        assert_eq!(doc.pointer("/ai/connection/auth_ref").unwrap(), "k");
        assert_eq!(doc.pointer("/ai/chat/url").unwrap(), "http://h:8080"); // прочее цело
        let cfg = nexus_core::ai::LocalConfig::parse(&doc.to_string()).unwrap();
        assert_eq!(
            cfg.ai.connection.mode(),
            nexus_core::ai::ConnectionMode::Local
        );
        assert_eq!(cfg.ai.connection.socket.as_deref(), Some("/tmp/a.sock"));

        // мусорный mode → embedded (SAFE); пустой socket → ключ удалён.
        apply_connection(&mut doc, "garbage", Some("  ")).unwrap();
        assert_eq!(doc.pointer("/ai/connection/mode").unwrap(), "embedded");
        assert!(doc.pointer("/ai/connection/socket").is_none());

        // socket=None → существующий путь не трогаем (смена режима не должна сюрприз-удалять).
        apply_connection(&mut doc, "local", Some("/tmp/b.sock")).unwrap();
        apply_connection(&mut doc, "embedded", None).unwrap();
        assert_eq!(doc.pointer("/ai/connection/mode").unwrap(), "embedded");
        assert_eq!(doc.pointer("/ai/connection/socket").unwrap(), "/tmp/b.sock");
    }

    /// ACP-1b: `apply_acp` пишет `ai.connection.{acp_command(массив),acp_cwd}`, не трогая mode/socket/url;
    /// пустая команда → ключ удалён; `None` → существующее не тронуто; итог парсится `LocalConfig`.
    #[test]
    fn apply_acp_round_trips_command_and_cwd() {
        let mut doc = serde_json::json!({
            "ai": { "connection": { "mode": "acp", "socket": "/tmp/s.sock", "url": "wss://x" } }
        });
        apply_acp(
            &mut doc,
            Some("hermes acp --stdio"),
            Some("/vault/root"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            doc.pointer("/ai/connection/acp_command").unwrap(),
            &serde_json::json!(["hermes", "acp", "--stdio"])
        );
        assert_eq!(
            doc.pointer("/ai/connection/acp_cwd").unwrap(),
            "/vault/root"
        );
        // mode/socket/url НЕ тронуты.
        assert_eq!(doc.pointer("/ai/connection/socket").unwrap(), "/tmp/s.sock");
        assert_eq!(doc.pointer("/ai/connection/url").unwrap(), "wss://x");
        let cfg = LocalConfig::parse(&doc.to_string()).unwrap();
        assert_eq!(
            cfg.ai.connection.acp_command.as_deref(),
            Some(["hermes".to_string(), "acp".into(), "--stdio".into()].as_slice())
        );
        assert_eq!(cfg.ai.connection.acp_cwd.as_deref(), Some("/vault/root"));

        // None → existing untouched; пустая команда/cwd → ключ удалён.
        apply_acp(&mut doc, None, None, None, None, None, None).unwrap();
        assert!(doc.pointer("/ai/connection/acp_command").is_some());
        apply_acp(&mut doc, Some("   "), Some(""), None, None, None, None).unwrap();
        assert!(doc.pointer("/ai/connection/acp_command").is_none());
        assert!(doc.pointer("/ai/connection/acp_cwd").is_none());
    }

    /// ACP-REMOTE-SSH: `apply_acp` пишет 4 ssh-поля (snake_case), не трогая mode/socket; round-trip через
    /// `LocalConfig` → `acp_spawn_argv()` собирает ssh-команду. `None` → существующее не тронуто; пусто → ключ убран.
    #[test]
    fn apply_acp_round_trips_ssh_fields() {
        let mut doc = serde_json::json!({
            "ai": { "connection": { "mode": "acp", "socket": "/tmp/s.sock" } }
        });
        apply_acp(
            &mut doc,
            None,
            Some("/tmp"),
            Some("ssh"),
            Some("artanov@192.168.0.28"),
            Some("~/.ssh/id_ed25519"),
            Some("docker exec -i hermes hermes acp"),
        )
        .unwrap();
        assert_eq!(doc.pointer("/ai/connection/acp_transport").unwrap(), "ssh");
        assert_eq!(
            doc.pointer("/ai/connection/acp_ssh_host").unwrap(),
            "artanov@192.168.0.28"
        );
        assert_eq!(
            doc.pointer("/ai/connection/acp_ssh_key").unwrap(),
            "~/.ssh/id_ed25519"
        );
        assert_eq!(
            doc.pointer("/ai/connection/acp_remote_command").unwrap(),
            "docker exec -i hermes hermes acp"
        );
        // socket НЕ тронут.
        assert_eq!(doc.pointer("/ai/connection/socket").unwrap(), "/tmp/s.sock");
        // Round-trip → резолвер собирает ssh-argv.
        let cfg = LocalConfig::parse(&doc.to_string()).unwrap();
        assert_eq!(
            cfg.ai.connection.acp_spawn_argv().unwrap(),
            vec![
                "ssh",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "BatchMode=yes",
                "-i",
                "~/.ssh/id_ed25519",
                "artanov@192.168.0.28",
                "docker",
                "exec",
                "-i",
                "hermes",
                "hermes",
                "acp"
            ]
        );

        // None → existing untouched; пустой ключ/хост → ключ удалён.
        apply_acp(&mut doc, None, None, None, None, None, None).unwrap();
        assert!(doc.pointer("/ai/connection/acp_ssh_host").is_some());
        apply_acp(&mut doc, None, None, None, Some("  "), Some(""), None).unwrap();
        assert!(doc.pointer("/ai/connection/acp_ssh_host").is_none());
        assert!(doc.pointer("/ai/connection/acp_ssh_key").is_none());
    }

    /// ACP-1b: probe без заданной команды (local) → внятная ошибка (не паника, не spawn).
    #[tokio::test]
    async fn probe_acp_without_command_errors() {
        let cfg = LocalConfig::parse("{}").unwrap();
        let e = probe_acp(&cfg, std::path::Path::new("/tmp"))
            .await
            .unwrap_err();
        assert!(format!("{e}").contains("команда не задана"), "got: {e}");
    }

    /// ACP-REMOTE-SSH: probe в ssh-транспорте без host/команды → внятная ssh-ошибка (не паника, не spawn).
    #[tokio::test]
    async fn probe_acp_ssh_without_host_errors() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{"connection":{"mode":"acp","acp_transport":"ssh","acp_remote_command":"hermes acp"}}}"#,
        )
        .unwrap();
        let e = probe_acp(&cfg, std::path::Path::new("/tmp"))
            .await
            .unwrap_err();
        assert!(
            format!("{e}").contains("укажите хост и команду"),
            "got: {e}"
        );
    }

    /// CONN-4: `classify_socket` без демона → внятная «не запущен» ошибка (не паника, не connect).
    #[cfg(unix)]
    #[test]
    fn classify_socket_not_found_is_clear_error() {
        let missing = std::path::Path::new("/tmp/nexus-conn4-nope-12345.sock");
        let e = classify_socket(missing).unwrap_err();
        assert!(e.contains("не запущен"), "got: {e}");
    }

    /// AGENT-0.6: `apply_agent_flags` пишет `ai.agent_actuator_enabled` (мастер-свитч записи агента),
    /// СОХРАНЯЯ прочие ключи. Round-trip через `LocalConfig` (нет коррапта).
    #[test]
    fn apply_agent_flags_writes_actuator_flag() {
        let mut doc = serde_json::json!({ "ai": { "chat": { "url": "http://h:8080" } } });
        let f = AgentFlagsDto {
            agent_autonomy: None,
            agent_actuator_enabled: true,
            sandbox_enabled: false,
            shell_enable: false,
            web_allow_public_fetch: false,
            skills_learning_enabled: false,
            agent_skills_dir: None,
            delegation_enabled: false,
            research_enabled: false,
        };
        apply_agent_flags(&mut doc, &f).unwrap();
        assert_eq!(doc["ai"]["agent_actuator_enabled"], serde_json::json!(true));
        // chat сохранён; итог валиден для LocalConfig.
        assert_eq!(doc["ai"]["chat"]["url"], serde_json::json!("http://h:8080"));
        let parsed = LocalConfig::parse(&doc.to_string()).unwrap();
        assert!(parsed.ai.agent_actuator_enabled);
    }

    /// apply_agent_flags пишет 4 флага, СОХРАНЯЯ chat/sync; `web.allow_public_fetch` мержится в
    /// существующий `ai.web` БЕЗ затирания `url`/`enabled`; итог парсится `LocalConfig` (нет коррапта).
    #[test]
    fn apply_agent_flags_sets_flags_and_preserves_chat_sync_and_web_url() {
        let mut doc = serde_json::json!({
            "sync": { "remote": "x" },
            "ai": {
                "chat": { "url": "http://h:8080", "model": "m" },
                "web": { "url": "http://searx:8888", "enabled": true }
            }
        });
        apply_agent_flags(&mut doc, &flags(Some("auto"), true, true, true)).unwrap();

        assert_eq!(doc.pointer("/ai/agent_autonomy").unwrap(), "auto");
        assert_eq!(doc.pointer("/ai/sandbox_enabled").unwrap(), true);
        assert_eq!(doc.pointer("/ai/shell_enable").unwrap(), true);
        assert_eq!(doc.pointer("/ai/web/allow_public_fetch").unwrap(), true);
        // Прочие ключи целы.
        assert_eq!(doc.pointer("/sync/remote").unwrap(), "x");
        assert_eq!(doc.pointer("/ai/chat/url").unwrap(), "http://h:8080");
        assert_eq!(
            doc.pointer("/ai/web/url").unwrap(),
            "http://searx:8888",
            "web.url НЕ затёрт"
        );
        assert_eq!(
            doc.pointer("/ai/web/enabled").unwrap(),
            true,
            "web.enabled НЕ затёрт"
        );

        // Round-trip: документ остаётся валидным local.json (chat не потерян).
        let pretty = serde_json::to_string(&doc).unwrap();
        let cfg = crate::ai::LocalConfig::parse(&pretty).unwrap();
        assert_eq!(cfg.ai.agent_autonomy.as_deref(), Some("auto"));
        assert!(cfg.ai.shell_enable && cfg.ai.sandbox_enabled);
        assert!(cfg.ai.web.as_ref().unwrap().allow_public_fetch);
        assert_eq!(cfg.ai.chat.unwrap().url, "http://h:8080");
    }

    /// Невалидная/None autonomy → ключ УБИРАЕТСЯ (дефолт confirm у агентд). `allow_public_fetch=false`
    /// БЕЗ существующего `ai.web` — НЕ создаёт шум-ключ `ai.web` (no-op).
    #[test]
    fn apply_agent_flags_removes_invalid_autonomy_and_skips_empty_web() {
        // Старт с уже записанной autonomy="auto"; новый набор с невалидной → ключ удаляется.
        let mut doc = serde_json::json!({ "ai": { "agent_autonomy": "auto" } });
        apply_agent_flags(&mut doc, &flags(Some("nonsense"), false, false, false)).unwrap();
        assert!(
            doc.pointer("/ai/agent_autonomy").is_none(),
            "невалидная autonomy → ключ убран (SAFE confirm)"
        );
        assert_eq!(doc.pointer("/ai/sandbox_enabled").unwrap(), false);
        assert_eq!(doc.pointer("/ai/shell_enable").unwrap(), false);
        assert!(
            doc.pointer("/ai/web").is_none(),
            "public_fetch=false без существующего ai.web → не создаём ai.web"
        );

        // None autonomy → тоже без ключа.
        let mut d2 = serde_json::json!({});
        apply_agent_flags(&mut d2, &flags(None, false, false, false)).unwrap();
        assert!(d2.pointer("/ai/agent_autonomy").is_none());
    }

    /// КОГЕРЕНТНОСТЬ trust-boundary: прямой вызов с `shell=true` при `sandbox=false` (минуя UI-гейт)
    /// НИКОГДА не персистит `shell_enable=true` — exec невозможен без песочницы (fail-closed в конфиге).
    #[test]
    fn apply_agent_flags_forces_shell_off_when_sandbox_off() {
        let mut doc = serde_json::json!({});
        apply_agent_flags(&mut doc, &flags(None, false, true, false)).unwrap();
        assert_eq!(
            doc.pointer("/ai/shell_enable").unwrap(),
            false,
            "shell без sandbox → форсим false (нельзя записать инкогерентную пару)"
        );
        assert_eq!(doc.pointer("/ai/sandbox_enabled").unwrap(), false);

        // При sandbox=true тот же shell=true проходит (когерентно).
        let mut on = serde_json::json!({});
        apply_agent_flags(&mut on, &flags(None, true, true, false)).unwrap();
        assert_eq!(on.pointer("/ai/shell_enable").unwrap(), true);
    }

    /// `allow_public_fetch=true` БЕЗ предыдущего `ai.web` → создаётся `ai.web` с пустым url (ИНЕРТЕН) —
    /// и весь документ остаётся парсимым (`WebConfig.url` `#[serde(default)]`, баг-корапт закрыт).
    #[test]
    fn apply_agent_flags_public_fetch_without_web_stays_parseable() {
        let mut doc = serde_json::json!({ "ai": { "chat": { "url": "http://h:8080" } } });
        apply_agent_flags(&mut doc, &flags(Some("confirm"), false, false, true)).unwrap();
        assert_eq!(doc.pointer("/ai/web/allow_public_fetch").unwrap(), true);

        let pretty = serde_json::to_string(&doc).unwrap();
        let cfg = crate::ai::LocalConfig::parse(&pretty).expect("local.json остаётся валидным");
        let web = cfg.ai.web.unwrap();
        assert!(web.url.is_empty(), "url пуст → web инертен");
        assert!(web.allow_public_fetch);
        assert_eq!(cfg.ai.chat.unwrap().url, "http://h:8080", "chat не потерян");
    }

    /// W-3: `apply_web_endpoint` зеркалит web-consent в `ai.web` — агент получает тот же url+enabled,
    /// что Home/chat-веб. Сохраняет `allow_public_fetch` и прочие ключи `ai`.
    #[test]
    fn apply_web_endpoint_mirrors_and_preserves_other_keys() {
        let mut doc = serde_json::json!({
            "sync": { "remote": "x" },
            "ai": {
                "chat": { "url": "http://h:8080" },
                "web": { "allow_public_fetch": true }
            }
        });
        apply_web_endpoint(&mut doc, true, "http://192.168.0.28:8888").unwrap();
        assert_eq!(doc.pointer("/ai/web/enabled").unwrap(), true);
        assert_eq!(
            doc.pointer("/ai/web/url").unwrap(),
            "http://192.168.0.28:8888"
        );
        // allow_public_fetch и прочие ключи целы.
        assert_eq!(doc.pointer("/ai/web/allow_public_fetch").unwrap(), true);
        assert_eq!(doc.pointer("/ai/chat/url").unwrap(), "http://h:8080");
        assert_eq!(doc.pointer("/sync/remote").unwrap(), "x");

        // Round-trip: агент-конфиг видит web с url+enabled (значит enable_web_tools включится).
        let pretty = serde_json::to_string(&doc).unwrap();
        let cfg = crate::ai::LocalConfig::parse(&pretty).unwrap();
        let web = cfg.ai.web.unwrap();
        assert!(web.enabled);
        assert_eq!(web.url, "http://192.168.0.28:8888");
        assert!(web.allow_public_fetch);
    }

    /// Пустой документ → `apply_web_endpoint` создаёт `ai.web`. Выключение пишет `enabled=false`
    /// (агент теряет веб-инструмент тем же тогглом).
    #[test]
    fn apply_web_endpoint_creates_section_and_can_disable() {
        let mut doc = serde_json::json!({});
        apply_web_endpoint(&mut doc, true, "http://searx:8888").unwrap();
        assert_eq!(doc.pointer("/ai/web/enabled").unwrap(), true);

        // Выключаем — enabled=false, url остаётся (history), агент инертен по enabled.
        apply_web_endpoint(&mut doc, false, "http://searx:8888").unwrap();
        assert_eq!(doc.pointer("/ai/web/enabled").unwrap(), false);
        let cfg = crate::ai::LocalConfig::parse(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert!(!cfg.ai.web.unwrap().enabled);
    }

    /// W-3 (skip-if-equal на открытии vault): зеркалить только при расхождении или отсутствии секции
    /// с непустым consent. Совпадение / (нет секции + пустой выключенный consent) → НЕ писать.
    #[test]
    fn web_needs_mirror_skips_equal_and_writes_on_drift() {
        // Совпадение — не пишем.
        assert!(!web_needs_mirror(
            Some((true, "http://s:8888")),
            true,
            "http://s:8888"
        ));
        // Расхождение url / enabled — пишем.
        assert!(web_needs_mirror(
            Some((true, "http://old")),
            true,
            "http://new"
        ));
        assert!(web_needs_mirror(
            Some((false, "http://s")),
            true,
            "http://s"
        ));
        // Секции нет, но глобальный consent активен (enabled или непустой url) — пишем.
        assert!(web_needs_mirror(None, true, ""));
        assert!(web_needs_mirror(None, false, "http://s:8888"));
        // Секции нет и consent пустой/выключенный — НЕ пишем (нет шум-записи на каждое открытие).
        assert!(!web_needs_mirror(None, false, ""));
    }

    /// AC-EGR-6: probe «Проверить связь» идёт через guarded с `Feature::Probe` — url вне политики
    /// отклоняется ТИПИЗИРОВАННО ДО сети (`.invalid`-домен дал бы DNS-ошибку, дойди запрос до
    /// сокета), выключенный Probe-opt-in режет даже loopback, а живой loopback-сервер достижим.
    #[tokio::test]
    async fn probe_endpoint_is_guarded() {
        use crate::ai::AiError;
        use crate::net::{EgressAudit, EgressDenied, EgressPolicy};
        use std::io::{Read, Write};
        use std::sync::atomic::AtomicBool;

        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        let probe = GuardedClient::for_probe(
            policy.clone(),
            Arc::new(EgressAudit::default()),
            Duration::from_secs(5),
        )
        .unwrap();

        // Публичный хост вне allowlist → Denied (НЕ DNS/reqwest-ошибка) — «первый egress-вектор».
        let denied = probe_endpoint(&probe, "http://probe-egress.invalid").await;
        assert!(
            matches!(
                denied,
                Err(AppError::Ai(AiError::Denied(EgressDenied::HostNotAllowed(
                    _
                ))))
            ),
            "ожидали типизированный отказ политики: {denied:?}"
        );

        // Выключенный Probe-opt-in режет и loopback — тег фичи у probe именно `Probe` (AC-EGR-5/6).
        policy.set_feature_enabled(EgressFeature::Probe, false);
        let feature_off = probe_endpoint(&probe, "http://127.0.0.1:9").await;
        assert!(matches!(
            feature_off,
            Err(AppError::Ai(AiError::Denied(
                EgressDenied::FeatureNotEnabled(EgressFeature::Probe)
            )))
        ));
        policy.set_feature_enabled(EgressFeature::Probe, true);

        // Живой loopback-сервер: любой HTTP-ответ → связь есть (local-first без consent, E6).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        probe_endpoint(&probe, &format!("http://{addr}"))
            .await
            .expect("loopback-probe проходит без consent");
        server.join().unwrap();
    }
}
