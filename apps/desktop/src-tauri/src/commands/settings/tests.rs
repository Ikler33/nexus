use super::*;

/// Fix BF-1 №3a: детектор `localhost`-хоста для IPv6-подсказки. Только имя `localhost` (не 127.0.0.1,
/// не `[::1]`, не суффиксы вроде `notlocalhost`); порт/схема/userinfo не мешают.
#[test]
fn localhost_host_detected_for_ipv6_hint() {
    assert!(url_host_is_localhost("http://localhost:8080/v1"));
    assert!(url_host_is_localhost("https://LocalHost"));
    assert!(url_host_is_localhost("http://user@localhost:1234"));
    assert!(!url_host_is_localhost("http://127.0.0.1:8080"));
    assert!(!url_host_is_localhost("http://[::1]:8080"));
    assert!(!url_host_is_localhost("http://notlocalhost:8080"));
    assert!(!url_host_is_localhost("http://192.168.0.31:8080/v1"));
    // Подсказка добавляется только для localhost.
    assert!(
        with_localhost_ipv6_hint("http://localhost:8080", "нет связи".into()).contains("127.0.0.1")
    );
    assert_eq!(
        with_localhost_ipv6_hint("http://127.0.0.1:8080", "нет связи".into()),
        "нет связи"
    );
}

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

fn flags(autonomy: Option<&str>, sandbox: bool, shell: bool, public_fetch: bool) -> AgentFlagsDto {
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

// ── R-3e: hot-apply провайдеров `set_ai_config` УНИФИЦИРОВАН на канон (фикстурная таблица) ───────
//
// Прежний особый путь UI СНЯТ (решение владельца): `build_hot_providers` теперь строит каноном
// `bootstrap::ProviderSet::from_config` из ИТОГОВОГО сохранённого `local.json`. Таблица пинит уже
// КАНОНИЧЕСКУЮ семантику; ДЕКЛАРИРУЕМАЯ дельта против прежнего пути (задекларирована в CHANGELOG):
// (1) утилитарный fast получает СОБСТВЕННЫЙ профиль таймаутов/температуры секции `ai.fast` (прежде —
//     единый профиль сохранённого `ai.chat`);
// (2) дефолт-модель fast → "fast" (прежде — цепочка fast.model → chat.model → "chat");
// (3) пустой/отсутствующий fast → chat_util = ТОТ ЖЕ Arc, что chat_fast (прежде — ОТДЕЛЬНЫЙ Arc на
//     chat-URL).
// Смягчение: в реальном UI-потоке `apply_ai` замещает секции формой url+model → кастомные таймауты
// стёрты ДО hot-apply → оба пути дают дефолтный профиль; расхождение видно лишь на рукописном
// `local.json` (ряд `hot_custom_*`). Снимки — с КАНОНИЧЕСКОГО кода команды.

/// Хот-провайдеры путём `set_ai_config` → `(chat, chat_fast, chat_util)`: канонный
/// [`build_hot_providers`] из ИТОГОВОГО сохранённого `local.json` (как в реальной команде —
/// `saved_cfg` уже содержит записанные `apply_ai` секции `ai.chat`/`ai.fast`).
async fn hot_build(saved_json: &str) -> HotProviders {
    use std::sync::atomic::AtomicBool;
    let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
    let audit = Arc::new(EgressAudit::default());
    let saved_cfg = LocalConfig::parse(saved_json).expect("итоговый local.json парсится");
    build_hot_providers(&saved_cfg, &policy, &audit).await
}

fn dto(url: &str, model: Option<&str>) -> EndpointDto {
    EndpointDto {
        url: url.into(),
        model: model.map(str::to_string),
    }
}

/// Итоговый local.json РЕАЛЬНОГО потока команды: старый doc → `apply_ai` (chat/fast секции
/// замещаются целиком формой url+model) → pretty. Именно его парсит hot-apply как `saved_cfg`.
fn saved_after_apply_ai(chat: Option<&EndpointDto>, fast: Option<&EndpointDto>) -> String {
    let mut doc = serde_json::json!({});
    apply_ai(&mut doc, chat, None, fast).unwrap();
    serde_json::to_string_pretty(&doc).unwrap()
}

/// UI-сейв chat+fast (обе модели заданы): chat-пара — reasoning ON/OFF на chat-URL; утилитарный —
/// на fast-URL/model, БЕЗ reasoning; профиль ВЕЗДЕ дефолтный (свежезаписанные `ai.chat`/`ai.fast` из
/// EndpointDto таймаутов не содержат → канон и прежний путь совпадают — снимки не менялись).
#[tokio::test]
async fn hot_full_ui_save_snapshots() {
    let chat = dto("http://192.168.0.28:8080", Some("qwen3-30b"));
    let fast = dto("http://192.168.0.28:8084", Some("gemma-4b"));
    let saved = saved_after_apply_ai(Some(&chat), Some(&fast));
    let (c, cf, util) = hot_build(&saved).await;
    assert_eq!(
        c.expect("chat задан → провайдер").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: true }"#
    );
    assert_eq!(
        cf.expect("chat задан → быстрый").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
    );
    assert_eq!(
        util.expect("fast задан → утилитарный").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8084/v1/chat/completions", model: "gemma-4b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
    );
}

/// R-3e: унификация на канон, решение владельца (дельта №3 — Arc-идентичность fallback). Нет
/// секции `ai.fast` → chat_util = ТОТ ЖЕ Arc, что chat_fast (канон-fallback `build_util_chat` →
/// `chat_fast.clone()`). Прежний особый путь строил ОТДЕЛЬНЫЙ Arc на chat-URL.
#[tokio::test]
async fn hot_empty_fast_falls_back_to_chat_fast_same_arc() {
    let chat = dto("http://192.168.0.28:8080", Some("qwen3-30b"));
    let saved = saved_after_apply_ai(Some(&chat), None);
    let (_, cf, util) = hot_build(&saved).await;
    let cf = cf.expect("chat задан → быстрый");
    let util = util.expect("fallback на chat_fast");
    assert!(
        Arc::ptr_eq(&cf, &util),
        "канон-fallback: chat_util = ТОТ ЖЕ Arc, что chat_fast"
    );
}

/// R-3e: унификация на канон, решение владельца (дельта №2 — дефолт-модель fast). fast-URL задан
/// БЕЗ модели → канон `build_util_chat` берёт дефолт "fast" (прежний путь брал chat.model
/// "qwen3-30b" по цепочке fast.model → chat.model → "chat"). Профиль — дефолтный (свежезаписанный
/// `ai.fast` таймаутов не несёт).
#[tokio::test]
async fn hot_fast_without_model_takes_fast_default() {
    let chat = dto("http://192.168.0.28:8080", Some("qwen3-30b"));
    let fast = dto("http://192.168.0.28:8084", None);
    let saved = saved_after_apply_ai(Some(&chat), Some(&fast));
    let (_, _, util) = hot_build(&saved).await;
    assert_eq!(
        util.expect("fast задан → утилитарный").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8084/v1/chat/completions", model: "fast", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
    );
}

/// R-3e: унификация на канон, решение владельца (ГЛАВНЫЙ ряд дельты №1 — собственный профиль fast).
/// Рукописный local.json с КАСТОМНЫМИ профилями chat И fast: канон вешает chat-профиль (connect 5s /
/// ft 45s / idle 10s / retry 7 / temp 0.9) на chat-ПАРУ, а утилитарный получает СОБСТВЕННЫЙ профиль
/// `ai.fast` (connect 2s / ft 20s / idle 4s / retry 1 / temp 0.05) и дефолт-модель "fast" (дельта
/// №2). Прежний особый путь вешал chat-профиль ВЕЗДЕ и брал модель "chat". NB: в реальном UI-потоке
/// `apply_ai` замещает секции формой url+model (кастомы стираются ДО hot-apply) — ряд фиксирует
/// величину поведенческого изменения на рукописном конфиге (в UI-потоке дельта не видна).
#[tokio::test]
async fn hot_custom_saved_profiles_own_sections() {
    let saved = r#"{
      "ai": {
        "chat": {
          "url": "http://127.0.0.1:9201/v1",
          "connect_timeout_secs": 5,
          "first_token_timeout_secs": 45,
          "idle_timeout_secs": 10,
          "retry_attempts": 7,
          "temperature": 0.9
        },
        "fast": {
          "url": "http://127.0.0.1:9202",
          "connect_timeout_secs": 2,
          "first_token_timeout_secs": 20,
          "idle_timeout_secs": 4,
          "retry_attempts": 1,
          "temperature": 0.05
        }
      }
    }"#;
    let (c, cf, util) = hot_build(saved).await;
    assert_eq!(
        c.expect("chat задан → провайдер").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=5s)", feature: Chat, endpoint: "http://127.0.0.1:9201/v1/chat/completions", model: "chat", temperature: 0.9, first_token_timeout: 45s, idle_timeout: 10s, retry: RetryPolicy { max_attempts: 7, base: 300ms, cap: 2s }, enable_thinking: true }"#
    );
    assert_eq!(
        cf.expect("chat задан → быстрый").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=5s)", feature: Chat, endpoint: "http://127.0.0.1:9201/v1/chat/completions", model: "chat", temperature: 0.9, first_token_timeout: 45s, idle_timeout: 10s, retry: RetryPolicy { max_attempts: 7, base: 300ms, cap: 2s }, enable_thinking: false }"#
    );
    assert_eq!(
        util.expect("fast задан → утилитарный").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=2s)", feature: Chat, endpoint: "http://127.0.0.1:9202/v1/chat/completions", model: "fast", temperature: 0.05, first_token_timeout: 20s, idle_timeout: 4s, retry: RetryPolicy { max_attempts: 1, base: 300ms, cap: 2s }, enable_thinking: false }"#
    );
}

/// chat=None (секция удалена), fast задан: chat-пары нет; утилитарный живёт на fast-URL с
/// ПРОВАЙДЕР-дефолтами (секция `ai.fast` без таймаутов → дефолты конструктора: температура 0.3,
/// connect 30s, модель из `ai.fast` "gemma-4b"). Канон и прежний путь совпадают — снимок не менялся.
#[tokio::test]
async fn hot_chat_none_fast_on_provider_defaults() {
    let fast = dto("http://192.168.0.28:8084", Some("gemma-4b"));
    let saved = saved_after_apply_ai(None, Some(&fast));
    let (c, cf, util) = hot_build(&saved).await;
    assert!(c.is_none(), "chat=None → провайдера нет");
    assert!(cf.is_none(), "chat=None → быстрого нет");
    assert_eq!(
        util.expect("fast задан → утилитарный").debug_params(),
        r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8084/v1/chat/completions", model: "gemma-4b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
    );
}

/// Оба DTO пусты → ни одного хот-провайдера (AI выключается до переоткрытия vault).
#[tokio::test]
async fn hot_nothing_configured_builds_nothing() {
    let saved = saved_after_apply_ai(None, None);
    let (c, cf, util) = hot_build(&saved).await;
    assert!(c.is_none() && cf.is_none() && util.is_none());
}
