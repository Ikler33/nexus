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

/// Текущая AI-конфигурация для префилла формы.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigDto {
    pub chat: Option<EndpointDto>,
    pub embedding: Option<EndpointDto>,
    /// Утилитарная мелкая модель (`ai.fast`, напр. Qwen3-4B) — inline/судья/сводка reasoning/новости.
    pub fast: Option<EndpointDto>,

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
    })
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
        }
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
