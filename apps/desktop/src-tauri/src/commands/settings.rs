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

use crate::ai::{AiError, ChatProvider, LocalConfig, OpenAiChatProvider};
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
pub struct AiConfigDto {
    pub chat: Option<EndpointDto>,
    pub embedding: Option<EndpointDto>,
    /// Утилитарная мелкая модель (`ai.fast`, напр. Qwen3-4B) — inline/судья/сводка reasoning/новости.
    pub fast: Option<EndpointDto>,
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

/// Текущая AI-конфигурация (из `.nexus/local.json`) — для префилла формы настроек.
#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> AppResult<AiConfigDto> {
    let root = state.vault().await?.root.clone();
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(AiConfigDto::default());
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

    // Горячее применение chat (stateless per-request → безопасно): пересобираем УЖЕ-guarded клиент
    // от того же policy/audit (AC-EGR-13). Embedding — через перезапуск (cold, на нём индексатор).
    let chat_provider: Option<Arc<dyn ChatProvider>> = match &chat {
        Some(c) => {
            let model = c.model.clone().unwrap_or_else(|| "chat".to_string());
            let guarded =
                GuardedClient::for_chat(state.egress_policy.clone(), state.egress_audit.clone())
                    .map_err(AiError::from)?;
            Some(Arc::new(OpenAiChatProvider::new(
                &guarded,
                EgressFeature::Chat,
                &c.url,
                &model,
                None,
            )))
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
            let guarded =
                GuardedClient::for_chat(state.egress_policy.clone(), state.egress_audit.clone())
                    .map_err(AiError::from)?;
            // R2 как в open_vault (`build_util_chat`): примитивам CoT не нужен, а на ai.fast может
            // жить reasoning-модель (баг 2026-06-11: gemma12 думала ~40 с над 6-словной сводкой R1).
            Some(Arc::new(
                OpenAiChatProvider::new(&guarded, EgressFeature::Chat, url, &model, None)
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
            let guarded =
                GuardedClient::for_chat(state.egress_policy.clone(), state.egress_audit.clone())
                    .map_err(AiError::from)?;
            // Хот-апплай #153 забыл R2: после сохранения настроек дайджест становился «думающим»
            // до переоткрытия vault. Теперь зеркалит `build_chat`.
            Some(Arc::new(
                OpenAiChatProvider::new(&guarded, EgressFeature::Chat, &c.url, &model, None)
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
