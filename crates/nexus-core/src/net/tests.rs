use super::*;
use std::io::{Read, Write};

/// Политика с отдельным kill-switch-атомиком (как в `AppState`) — для тестов.
fn policy_with_switch() -> (Arc<EgressPolicy>, Arc<AtomicBool>) {
    let offline = Arc::new(AtomicBool::new(false));
    (Arc::new(EgressPolicy::new(offline.clone())), offline)
}

/// Гард с боевым `SystemResolver` (литералы 127.0.0.1/IP резолвятся в себя без сети).
fn guarded(policy: Arc<EgressPolicy>) -> (GuardedClient, Arc<EgressAudit>) {
    let audit = Arc::new(EgressAudit::default());
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();
    (client, audit)
}

/// Гард с МОК-резолвером: любой хост → заданный список IP (DNS-rebinding-сценарии без сети).
fn guarded_with_ips(
    policy: Arc<EgressPolicy>,
    ips: Vec<std::net::IpAddr>,
) -> (GuardedClient, Arc<EgressAudit>) {
    let audit = Arc::new(EgressAudit::default());
    let resolver = Arc::new(resolve::test_support::FixedResolver::new(ips));
    let client = GuardedClient::new(policy, audit.clone(), |b| b)
        .unwrap()
        .with_resolver(resolver);
    (client, audit)
}

/// Мок-сервер одного запроса: отдаёт `resp` первой принятой связи (стиль прежнего ai/mod.rs).
fn serve_once(resp: &'static str) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(resp.as_bytes());
        }
    });
    (addr, handle)
}

/// AC-SEC-4 / ревью C5: core-HTTP-клиент НЕ следует редиректам. Локальный сервер отдаёт 302 на
/// metadata-адрес; клиент обязан вернуть сам 302, а не пойти по `Location` (иначе — SSRF).
#[tokio::test]
async fn core_client_does_not_follow_redirects() {
    let (addr, server) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n",
    );
    let client = core_client_builder().build().unwrap();
    let resp = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect("запрос к локальному серверу");
    assert_eq!(
        resp.status().as_u16(),
        302,
        "core-клиент НЕ должен следовать редиректу (анти-SSRF, AC-SEC-4)"
    );
    server.join().unwrap();
}

/// AC-EGR-7 (кейс 302→metadata через guarded): редирект не выполняется И прямой запрос на
/// metadata отклоняется политикой ВСЕГДА (AC-EGR-12) — ещё до сокета/DNS.
#[tokio::test]
async fn guarded_does_not_follow_redirect_to_metadata() {
    let (addr, server) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n",
    );
    let (client, _) = guarded(policy_with_switch().0);
    let resp = client
        .get(
            &format!("http://{addr}/"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback разрешён local-first");
    assert_eq!(
        resp.status().as_u16(),
        302,
        "redirect=none сохранён после рефактора"
    );
    server.join().unwrap();

    let denied = client
        .get(
            "http://169.254.169.254/latest/meta-data",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(
        matches!(
            denied,
            Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))
        ),
        "metadata отклоняется политикой, не сетевой ошибкой: {denied:?}"
    );
}

/// AC-EGR-12 (E7): metadata-блок — первый и безусловный: ни allowlist, ни kill-switch-ветка
/// «приватный хост жив» его не открывают.
#[test]
fn policy_rejects_metadata_unconditionally() {
    let (policy, offline) = policy_with_switch();
    policy.set_allowlist(["169.254.169.254".to_string()]);
    for off in [false, true] {
        offline.store(off, Ordering::Relaxed);
        assert!(
            matches!(
                policy.check("169.254.169.254", EgressFeature::Chat),
                Err(EgressDenied::HostNotAllowed(_))
            ),
            "metadata reject ВСЕГДА (offline={off})"
        );
    }
}

/// AC-EGR-3 (E2): «офлайн» рубит публичный хост, LAN/loopback живут (local-first).
#[test]
fn policy_offline_blocks_public_keeps_lan() {
    let (policy, offline) = policy_with_switch();
    offline.store(true, Ordering::Relaxed);
    assert_eq!(
        policy.check("203.0.113.7", EgressFeature::Chat),
        Err(EgressDenied::Offline)
    );
    assert_eq!(
        policy.check("api.example.com", EgressFeature::Embed),
        Err(EgressDenied::Offline)
    );
    for lan in ["127.0.0.1", "192.168.0.29", "localhost"] {
        assert_eq!(
            policy.check(lan, EgressFeature::Chat),
            Ok(()),
            "{lan} живёт при офлайн (E2, local-first)"
        );
    }
}

/// AC-EGR-5 (E6): выключенная фича → `FeatureNotEnabled`; другие фичи не задеты; включение
/// возвращает доступ.
#[test]
fn policy_feature_opt_in_is_independent() {
    let (policy, _) = policy_with_switch();
    policy.set_feature_enabled(EgressFeature::Embed, false);
    assert_eq!(
        policy.check("127.0.0.1", EgressFeature::Embed),
        Err(EgressDenied::FeatureNotEnabled(EgressFeature::Embed))
    );
    assert_eq!(
        policy.check("127.0.0.1", EgressFeature::Chat),
        Ok(()),
        "отключение одной фичи не трогает другую (AC-EGR-5)"
    );
    policy.set_feature_enabled(EgressFeature::Embed, true);
    assert_eq!(policy.check("127.0.0.1", EgressFeature::Embed), Ok(()));
}

/// AC-EGR-2 (юнит): публичный хост вне allowlist → `HostNotAllowed`; в allowlist → проходит;
/// приватные проходят без allowlist (E4/local-first).
#[test]
fn policy_allowlist_or_private() {
    let (policy, _) = policy_with_switch();
    assert!(matches!(
        policy.check("api.example.com", EgressFeature::Chat),
        Err(EgressDenied::HostNotAllowed(_))
    ));
    policy.set_allowlist(["api.example.com".to_string()]);
    assert_eq!(policy.check("api.example.com", EgressFeature::Chat), Ok(()));
    assert_eq!(
        policy.check("192.168.0.172", EgressFeature::Chat),
        Ok(()),
        "LAN — без allowlist (local-first)"
    );
}

/// WEB-FETCH-PUBLIC: `web_allow_public` снимает требование allowlist для фичи `Web` на ПУБЛИЧНЫХ
/// хостах, СОХРАНЯЯ deny_private/metadata/offline; не распространяется на NewsFeed.
#[test]
fn policy_web_allow_public_lifts_allowlist_for_public_web_only() {
    let (policy, _) = policy_with_switch();
    policy.set_feature_enabled(EgressFeature::Web, true);
    // БЕЗ web_allow_public: публичный хост вне allowlist → отказ (allowlist-only).
    assert!(matches!(
        policy.check("example.com", EgressFeature::Web),
        Err(EgressDenied::HostNotAllowed(_))
    ));
    assert!(!policy.web_allow_public());
    policy.set_web_allow_public(true);
    assert!(policy.web_allow_public());
    // Любой ПУБЛИЧНЫЙ хост проходит БЕЗ allowlist.
    assert_eq!(policy.check("example.com", EgressFeature::Web), Ok(()));
    assert_eq!(policy.check("203.0.113.7", EgressFeature::Web), Ok(()));
    // Приватные/LAN/metadata всё равно режутся (deny_private/шаг 1).
    for blocked in ["192.168.0.10", "127.0.0.1", "10.0.0.1", "169.254.169.254"] {
        assert!(
            matches!(
                policy.check(blocked, EgressFeature::Web),
                Err(EgressDenied::HostNotAllowed(_))
            ),
            "{blocked} режется даже при web_allow_public"
        );
    }
    // Касается ТОЛЬКО Web: NewsFeed (тоже web-класс) — публичный без allowlist всё равно отказ.
    policy.set_feature_enabled(EgressFeature::NewsFeed, true);
    assert!(
        matches!(
            policy.check("example.com", EgressFeature::NewsFeed),
            Err(EgressDenied::HostNotAllowed(_))
        ),
        "web_allow_public НЕ распространяется на NewsFeed"
    );
}

/// WEB-FETCH-PUBLIC не обходит офлайн-kill: публичный web под офлайном всё равно `Offline`.
#[test]
fn policy_web_allow_public_respects_offline() {
    let (policy, offline) = policy_with_switch();
    policy.set_feature_enabled(EgressFeature::Web, true);
    policy.set_web_allow_public(true);
    offline.store(true, Ordering::Relaxed);
    assert_eq!(
        policy.check("example.com", EgressFeature::Web),
        Err(EgressDenied::Offline)
    );
}

/// NF-4 (AC-NF-7/8): NewsFeed — web-класс. Выключена из коробки (consent W2); после
/// включения публичный хост из "news"-скоупа проходит, а приватный/LAN запрещён ДАЖЕ из
/// allowlist (`allow_private=false`, W-аддендум); скоупы "ai"/"news" независимы; local-first
/// для Chat не задет.
#[test]
fn news_feed_is_web_class_private_denied_even_allowlisted() {
    let (policy, _) = policy_with_switch();
    assert!(
        matches!(
            policy.check("feeds.example.com", EgressFeature::NewsFeed),
            Err(EgressDenied::FeatureNotEnabled(EgressFeature::NewsFeed))
        ),
        "web-класс не из коробки"
    );
    policy.set_feature_enabled(EgressFeature::NewsFeed, true);
    assert!(matches!(
        policy.check("feeds.example.com", EgressFeature::NewsFeed),
        Err(EgressDenied::HostNotAllowed(_))
    ));
    policy.set_scoped_allowlist(
        "news",
        ["feeds.example.com".to_string(), "192.168.0.5".to_string()],
    );
    assert_eq!(
        policy.check("feeds.example.com", EgressFeature::NewsFeed),
        Ok(())
    );
    assert!(
        matches!(
            policy.check("192.168.0.5", EgressFeature::NewsFeed),
            Err(EgressDenied::HostNotAllowed(_))
        ),
        "allow_private=false: приватный запрещён даже из allowlist"
    );
    // Скоупы независимы: ai-замещение не трогает news.
    policy.set_allowlist(["api.other.com".to_string()]);
    assert_eq!(
        policy.check("feeds.example.com", EgressFeature::NewsFeed),
        Ok(()),
        "news-скоуп пережил замену ai-скоупа"
    );
    // Local-first для Chat не задет web-правилом.
    assert_eq!(policy.check("192.168.0.5", EgressFeature::Chat), Ok(()));
}

/// AC-EGR-2 (интеграция): отказ происходит ДО сокета — мок-listener обязан НЕ принять
/// соединение. Отказ — через выключенную фичу (любой отказ режется в одной точке authorize).
#[tokio::test]
async fn denied_request_never_touches_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let (policy, _) = policy_with_switch();
    policy.set_feature_enabled(EgressFeature::Chat, false);
    let (client, audit) = guarded(policy);

    let res = client
        .post_json(
            &format!("http://{addr}/v1/chat/completions"),
            EgressFeature::Chat,
            &serde_json::json!({"messages": []}),
            RunCtx::NONE,
        )
        .await;
    assert!(matches!(
        res,
        Err(NetError::Denied(EgressDenied::FeatureNotEnabled(
            EgressFeature::Chat
        )))
    ));
    assert!(
        matches!(
            listener.accept(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
        ),
        "0 сетевых коннектов: listener не должен был принять соединение (AC-EGR-2)"
    );
    let entries = audit.entries();
    assert_eq!(entries.len(), 1, "ровно одна audit-запись на отказ");
    assert!(!entries[0].allowed);
}

/// AC-EGR-2: `HostNotAllowed` режется до DNS — иначе `.invalid`-домен дал бы сетевую
/// (resolve) ошибку `NetError::Http`, а не структурированный отказ.
#[tokio::test]
async fn host_not_allowed_denied_before_dns() {
    let (client, _) = guarded(policy_with_switch().0);
    let res = client
        .get(
            "http://egress-foundation-test.invalid/v1/models",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(
        matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
        "ожидали отказ политики ДО DNS: {res:?}"
    );
}

/// AC-EGR-3/9 (интеграция): при kill-switch=офлайн loopback-эгресс реально работает
/// (локальный LLM жив), а публичный отклоняется типизированно.
#[tokio::test]
async fn offline_keeps_loopback_alive() {
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    let (policy, offline) = policy_with_switch();
    offline.store(true, Ordering::Relaxed);
    let (client, _) = guarded(policy);

    let resp = client
        .get(
            &format!("http://{addr}/v1/models"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback живёт при офлайн (E2)");
    assert_eq!(resp.status().as_u16(), 200);
    server.join().unwrap();

    let denied = client
        .get("http://203.0.113.7/", EgressFeature::Probe, RunCtx::NONE)
        .await;
    assert!(matches!(
        denied,
        Err(NetError::Denied(EgressDenied::Offline))
    ));
}

/// AC-EGR-4: успех И отказ → по одной записи `{feature, host, bytes_out?, decision}`;
/// Debug записи НЕ печатает хост (`Redacted`); публичного мутатора/clear у журнала нет.
#[tokio::test]
async fn audit_records_success_and_denial_with_redacted_host() {
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    let (policy, _) = policy_with_switch();
    let (client, audit) = guarded(policy.clone());

    client
        .get(
            &format!("http://{addr}/v1/models"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback разрешён");
    server.join().unwrap();
    let denied = client
        .get(
            "http://api.example.com/v1/models",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(matches!(denied, Err(NetError::Denied(_))));

    let entries = audit.entries();
    assert_eq!(entries.len(), 2, "каждый вызов — ровно одна запись");
    assert!(entries[0].allowed && entries[1].denied_reason.is_some());
    assert_eq!(entries[1].feature, EgressFeature::Probe);
    let dump = format!("{entries:?}");
    assert!(
        !dump.contains("127.0.0.1") && !dump.contains("api.example.com"),
        "host в audit — Redacted, в Debug не утекает (AC-EGR-4): {dump}"
    );
    assert_eq!(
        entries[0].host.expose(),
        "127.0.0.1",
        "явный expose() работает"
    );
}

/// AC-EGR-10: `bytes_out` — best-effort размер тела ЗАПРОСА: `Some(len)` для JSON-post
/// (длина сериализованного тела), `None` для GET.
#[tokio::test]
async fn bytes_out_is_request_body_best_effort() {
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    let (policy, _) = policy_with_switch();
    let (client, audit) = guarded(policy);

    let body =
        serde_json::json!({"model": "gemma", "messages": [{"role": "user", "content": "hi"}]});
    let expected = serde_json::to_vec(&body).unwrap().len();
    client
        .post_json(
            &format!("http://{addr}/v1/chat/completions"),
            EgressFeature::Chat,
            &body,
            RunCtx::NONE,
        )
        .await
        .expect("loopback разрешён");
    server.join().unwrap();
    let denied_get = client
        .get(
            "http://api.example.com/x",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(denied_get.is_err());

    let entries = audit.entries();
    assert_eq!(
        entries[0].bytes_out,
        Some(expected),
        "post: длина тела запроса"
    );
    assert!(
        entries[0].bytes_out.unwrap() >= 2,
        "Content-Length >= len(body)"
    );
    assert_eq!(entries[1].bytes_out, None, "get: тела нет");
}

/// P0-a (DNS-rebinding на CORE-пути): chat-хост, резолвящийся в metadata 169.254.169.254,
/// отклоняется ДО коннекта — типизированным отказом (не сетевой ошибкой) и аудитится как denial.
/// Мок-listener обязан НЕ принять соединение.
#[tokio::test]
async fn chat_host_resolving_to_metadata_is_denied_before_connect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();

    let (policy, _) = policy_with_switch();
    policy.set_allowlist(["chat.example.com".to_string()]); // host-string-гейт пропустит
    let (client, audit) = guarded_with_ips(policy, vec!["169.254.169.254".parse().unwrap()]);

    let res = client
        .post_json(
            "http://chat.example.com/v1/chat/completions",
            EgressFeature::Chat,
            &serde_json::json!({"messages": []}),
            RunCtx::NONE,
        )
        .await;
    assert!(
        matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
        "rebind на metadata режется типизированно: {res:?}"
    );
    assert!(
        matches!(
            listener.accept(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
        ),
        "0 коннектов: DNS-гард отрезал ДО сокета (P0-a)"
    );
    let entries = audit.entries();
    assert_eq!(entries.len(), 1, "ровно одна audit-запись на отказ");
    assert!(!entries[0].allowed, "rebind аудитится как denial");
}

/// P0-a (local-first сохранён): chat-хост, резолвящийся в loopback/LAN, ДОПУСКАЕТСЯ — приватные
/// IP для chat живут (LAN-LLM). Реальный коннект на loopback-мок проходит (пин не ломает loopback).
#[tokio::test]
async fn chat_host_resolving_to_loopback_or_lan_is_allowed() {
    // Реальный loopback-сервер; мок-резолвер отдаёт его адрес как «резолв» публичного имени.
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    let (policy, _) = policy_with_switch();
    policy.set_allowlist(["chat.example.com".to_string()]);
    let (client, audit) = guarded_with_ips(policy, vec![addr.ip()]);

    // URL-порт должен совпасть с портом пина → используем порт мок-сервера в URL.
    let url = format!("http://chat.example.com:{}/v1/models", addr.port());
    let resp = client
        .get(&url, EgressFeature::Chat, RunCtx::NONE)
        .await
        .expect("loopback/LAN для chat живёт (local-first)");
    assert_eq!(resp.status().as_u16(), 200);
    server.join().unwrap();
    assert!(audit.entries()[0].allowed, "успех аудитится как allowed");

    // И чисто политически: LAN-IP (192.168.x) для chat проходит ip-гард.
    let (policy2, _) = policy_with_switch();
    policy2.set_allowlist(["lan.example.com".to_string()]);
    let (client2, _) = guarded_with_ips(policy2, vec!["192.168.0.31".parse().unwrap()]);
    // Коннекта к 192.168.0.31 не будет (нет сервера) — но гард обязан ПРОПУСТИТЬ (ip-allow),
    // отказ может прийти только сетевой (Http), не Denied. Проверяем именно это разграничение.
    let res = client2
        .get(
            "http://lan.example.com/v1/models",
            EgressFeature::Chat,
            RunCtx::NONE,
        )
        .await;
    assert!(
        !matches!(res, Err(NetError::Denied(_))),
        "LAN для chat НЕ отклоняется политикой/гардом (local-first): {res:?}"
    );
}

/// P0-a (web-класс): NewsFeed-хост, резолвящийся в приватный LAN-IP, отклоняется (deny_private).
#[tokio::test]
async fn web_class_host_resolving_to_private_is_denied() {
    let (policy, _) = policy_with_switch();
    policy.set_feature_enabled(EgressFeature::NewsFeed, true);
    policy.set_scoped_allowlist("news", ["feeds.example.com".to_string()]);
    let (client, audit) = guarded_with_ips(policy, vec!["10.0.0.7".parse().unwrap()]);

    let res = client
        .get(
            "https://feeds.example.com/rss",
            EgressFeature::NewsFeed,
            RunCtx::NONE,
        )
        .await;
    assert!(
        matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
        "web-класс: приватный резолв denied: {res:?}"
    );
    assert!(!audit.entries()[0].allowed);
}

/// WEB-FETCH-PUBLIC ∩ SSRF (P0-a, регресс-замок): даже с `web_allow_public=true` ПУБЛИЧНОЕ имя,
/// резолвящееся в ПРИВАТНЫЙ IP (DNS-rebind), отклоняется на РЕЗОЛВНУТОМ IP — `authorize` →
/// `check_resolved_ips(deny_private=true для Web)` НЕ зависит от `web_allow_public`. Снятие
/// string-allowlist НЕ снимает rebind-гард. (Если `Web.denies_private()` когда-нибудь станет false —
/// этот тест упадёт, не дав молча открыть SSRF.)
#[tokio::test]
async fn web_allow_public_still_blocks_dns_rebind_to_private() {
    let (policy, _) = policy_with_switch();
    policy.set_feature_enabled(EgressFeature::Web, true);
    policy.set_web_allow_public(true); // string-allowlist снят для публичных
    let (client, audit) = guarded_with_ips(policy, vec!["10.0.0.7".parse().unwrap()]);
    let res = client
        .get(
            "https://totally-public.example/page",
            EgressFeature::Web,
            RunCtx::NONE,
        )
        .await;
    assert!(
        matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
        "rebind публичного имени в приватный IP режется ДАЖЕ при web_allow_public: {res:?}"
    );
    assert!(!audit.entries()[0].allowed, "denial зафиксирован в audit");
}

/// P0-a: пустой резолв → типизированный отказ (нечего пинить), аудит как denial, без сети.
#[tokio::test]
async fn empty_resolution_is_denied() {
    let (policy, _) = policy_with_switch();
    policy.set_allowlist(["chat.example.com".to_string()]);
    let (client, audit) = guarded_with_ips(policy, vec![]);
    let res = client
        .get(
            "http://chat.example.com/v1/models",
            EgressFeature::Chat,
            RunCtx::NONE,
        )
        .await;
    assert!(matches!(
        res,
        Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))
    ));
    assert_eq!(audit.len(), 1);
    assert!(!audit.entries()[0].allowed);
}

/// URL без хоста: типизированный `BadUrl`, одна audit-запись, в сеть не уходим.
#[tokio::test]
async fn bad_url_is_rejected_and_audited() {
    let (client, audit) = {
        let (policy, _) = policy_with_switch();
        guarded(policy)
    };
    let res = client
        .get("definitely not a url", EgressFeature::Probe, RunCtx::NONE)
        .await;
    assert!(matches!(res, Err(NetError::BadUrl)));
    assert_eq!(audit.len(), 1);
    assert!(!audit.entries()[0].allowed);
}

/// Открывает временную vault-БД (миграции применены, в т.ч. 020 egress_audit). `(Database, TempDir)`
/// в таком порядке: при выходе из scope сначала закрывается БД, потом удаляется каталог.
async fn temp_db() -> (crate::db::Database, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db = crate::db::Database::open(dir.path().join(".nexus/nexus.db"))
        .await
        .expect("open db");
    (db, dir)
}

/// Снимок durable-журнала: `(feature, host, allowed, denied_is_some, run_id)` в порядке вставки.
async fn durable_rows(db: &crate::db::Database) -> Vec<(String, String, bool, bool, Option<i64>)> {
    db.reader()
        .query(|c| {
            let mut stmt = c.prepare(
                "SELECT feature, host, allowed, denied_reason, run_id \
                 FROM egress_audit ORDER BY id",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, Option<String>>(3)?.is_some(),
                        r.get::<_, Option<i64>>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

/// P0-b (durable persist): с подключённым writer `record()` персистит строку в `egress_audit` —
/// реальный host, decision, run_id=None (scaffold). Успех И отказ оба durable.
#[tokio::test]
async fn durable_record_persists_row_with_writer() {
    let (db, _dir) = temp_db().await;
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");

    let audit = Arc::new(EgressAudit::default());
    audit.set_writer(db.writer().clone());
    let (policy, _) = policy_with_switch();
    policy.set_allowlist(["api.example.com".to_string()]);
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

    // Успех на loopback.
    client
        .get(
            &format!("http://{addr}/v1/models"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback разрешён");
    server.join().unwrap();
    // Отказ: публичный хост вне allowlist (api.example.com в allowlist → используем другой).
    let denied = client
        .get(
            "http://blocked.example.com/x",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(matches!(denied, Err(NetError::Denied(_))));

    let rows = durable_rows(&db).await;
    assert_eq!(rows.len(), 2, "оба эгресса durable-персистнуты");
    // Строка 1 — успех на реальном loopback-хосте (НЕ Redacted в БД).
    assert_eq!(rows[0].0, "probe");
    assert_eq!(
        rows[0].1, "127.0.0.1",
        "host хранится РЕАЛЬНЫЙ, не Redacted"
    );
    assert!(
        rows[0].2 && !rows[0].3,
        "успех: allowed=1, denied_reason=NULL"
    );
    assert_eq!(rows[0].4, None, "run_id scaffold: None");
    // Строка 2 — отказ.
    assert_eq!(rows[1].1, "blocked.example.com");
    assert!(
        !rows[1].2 && rows[1].3,
        "отказ: allowed=0, denied_reason set"
    );
}

/// P0-b (write-before-act ORDERING): durable denial-строка существует ДО возврата `authorize` —
/// т.е. ДО того, как мог бы уйти сокет. Гард-denied запрос (DNS-гард режет до коннекта) оставляет
/// durable-строку, причём listener-мок НЕ принимает соединение (0 коннектов). Так как `authorize`
/// awaits `record()` синхронно перед I/O, наличие строки сразу после await доказывает порядок.
#[tokio::test]
async fn durable_denial_row_exists_before_authorize_returns() {
    let (db, _dir) = temp_db().await;
    // Listener, который НЕ должен принять соединение (отказ — до сокета).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();

    let audit = Arc::new(EgressAudit::default());
    audit.set_writer(db.writer().clone());
    let (policy, _) = policy_with_switch();
    policy.set_allowlist(["chat.example.com".to_string()]); // host-гейт пропустит
                                                            // DNS-гард: хост резолвится в metadata → denied ДО коннекта (P0-a).
    let resolver = Arc::new(resolve::test_support::FixedResolver::new(vec![
        "169.254.169.254".parse().unwrap(),
    ]));
    let client = GuardedClient::new(policy, audit.clone(), |b| b)
        .unwrap()
        .with_resolver(resolver);

    let res = client
        .post_json(
            "http://chat.example.com/v1/chat/completions",
            EgressFeature::Chat,
            &serde_json::json!({"messages": []}),
            RunCtx::NONE,
        )
        .await;
    assert!(
        matches!(res, Err(NetError::Denied(EgressDenied::HostNotAllowed(_)))),
        "rebind на metadata режется типизированно: {res:?}"
    );
    // 0 коннектов: гард отрезал ДО сокета.
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "denied-запрос не должен был коснуться сокета (write-before-act)"
    );
    // Durable-строка УЖЕ есть сразу после возврата authorize (record awaited перед любым I/O):
    // строка существует, а сокета не было → запись произошла ДО (несуществующей) отправки.
    let rows = durable_rows(&db).await;
    assert_eq!(rows.len(), 1, "denial durable-персистнут write-before-act");
    assert_eq!(rows[0].0, "chat");
    assert!(!rows[0].2, "denial: allowed=0");
    assert!(rows[0].3, "denial_reason set");
}

/// P0-b (pre-vault окно / тесты): БЕЗ writer'а `record()` всё равно работает — пишет только in-memory.
/// Это сценарий pre-vault эгресса (БД ещё не открыта) и всех тестов с `EgressAudit::default()`.
#[tokio::test]
async fn record_without_writer_is_in_memory_only() {
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    let (client, audit) = guarded(policy_with_switch().0);
    client
        .get(
            &format!("http://{addr}/v1/models"),
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await
        .expect("loopback разрешён");
    server.join().unwrap();
    assert_eq!(audit.len(), 1, "in-memory работает без writer (pre-vault)");
    assert!(audit.entries()[0].allowed);
}

/// P0-b (write-before-act ORDERING, SUCCESS-путь): durable success-строка (`allowed=1`) существует
/// ДО возврата `authorize` — т.е. ДО любого сокета/send. Зовём приватный `authorize` напрямую (НЕ
/// `get`), поэтому МЕЖДУ awaited `record()` и проверкой БД сетевого I/O нет вообще: наличие строки
/// сразу после `authorize().await` доказывает, что success-`record()` закоммичен ПЕРЕД отправкой.
/// Регрессия, делающая success-`record()` fire-and-forget (не awaited внутри authorize), валит тест.
/// Listener — принимающий loopback (mirror denial-теста), но на коннект мы НЕ полагаемся: `authorize`
/// возвращает только пин-клиент, send не делается, так что сокет остаётся нетронутым.
#[tokio::test]
async fn durable_success_row_exists_before_authorize_returns() {
    let (db, _dir) = temp_db().await;
    // Принимающий loopback-listener (как в success-кейсе durable_record_persists_*). На приём
    // соединения тест НЕ полагается — он лишь даёт реальный адрес, резолвящийся в 127.0.0.1.
    let (addr, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");

    let audit = Arc::new(EgressAudit::default());
    audit.set_writer(db.writer().clone());
    let (policy, _) = policy_with_switch();
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

    // Вызываем ПРИВАТНЫЙ authorize напрямую: проходит host-гейт (loopback local-first) + DNS/SSRF-гард
    // (127.0.0.1 резолвится в себя), затем success-`record()` AWAITED, затем строится пин-клиент.
    // send НЕ делается → между коммитом записи и проверкой БД сетевого I/O нет.
    let url = format!("http://{addr}/v1/models");
    let authorized = client
        .authorize(&url, EgressFeature::Probe, None, RunCtx::NONE)
        .await;
    assert!(authorized.is_ok(), "loopback success-путь: {authorized:?}");

    // Durable success-строка УЖЕ есть сразу после возврата authorize, ДО какого-либо send.
    // Будь success-`record()` fire-and-forget — строки тут могло не быть (тест бы упал).
    let rows = durable_rows(&db).await;
    assert_eq!(
        rows.len(),
        1,
        "success durable-персистнут write-before-act (ДО send)"
    );
    assert_eq!(rows[0].0, "probe");
    assert_eq!(rows[0].1, "127.0.0.1", "host хранится РЕАЛЬНЫЙ");
    assert!(
        rows[0].2 && !rows[0].3,
        "success: allowed=1, denied_reason=NULL"
    );

    // listener так и не принял соединение (send не вызывался) — закрываем его, дренируя поток.
    drop(client);
    drop(server);
}

/// P0-b (vault re-open writer swap): `record()` перечитывает writer ПЕР-вызов (под мьютексом), поэтому
/// после `set_writer(B)` записи идут ТОЛЬКО в B, а старая БД A остаётся со своей единственной строкой.
/// Доказывает атомарность подмены стока на переоткрытии vault и отсутствие stale-writer-в-старую-БД.
/// Эгресс — denied (host-гейт режет публичный хост вне allowlist) для простоты: durable-строка пишется
/// и на отказе (write-before-act), сеть не нужна.
#[tokio::test]
async fn writer_swap_on_vault_reopen_routes_to_new_db_only() {
    let (db_a, _dir_a) = temp_db().await;
    let (db_b, _dir_b) = temp_db().await;

    let audit = Arc::new(EgressAudit::default());
    let (policy, _) = policy_with_switch();
    let client = GuardedClient::new(policy, audit.clone(), |b| b).unwrap();

    // Сток = A. Один denied-эгресс → строка в A.
    audit.set_writer(db_a.writer().clone());
    let denied_a = client
        .get(
            "http://first.example.com/x",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(matches!(denied_a, Err(NetError::Denied(_))));

    let rows_a1 = durable_rows(&db_a).await;
    assert_eq!(rows_a1.len(), 1, "1-я строка в A");
    assert_eq!(rows_a1[0].1, "first.example.com");
    assert!(durable_rows(&db_b).await.is_empty(), "B ещё пуста");

    // Переоткрытие vault: подменяем сток на B. Следующий эгресс должен попасть ТОЛЬКО в B.
    audit.set_writer(db_b.writer().clone());
    let denied_b = client
        .get(
            "http://second.example.com/y",
            EgressFeature::Probe,
            RunCtx::NONE,
        )
        .await;
    assert!(matches!(denied_b, Err(NetError::Denied(_))));

    // B содержит ровно 2-ю строку; A — по-прежнему только 1-ю (stale-writer в A не писал).
    let rows_b = durable_rows(&db_b).await;
    assert_eq!(rows_b.len(), 1, "2-я строка ТОЛЬКО в B");
    assert_eq!(rows_b[0].1, "second.example.com");
    let rows_a2 = durable_rows(&db_a).await;
    assert_eq!(rows_a2.len(), 1, "A не изменилась после подмены стока");
    assert_eq!(
        rows_a2[0].1, "first.example.com",
        "A хранит свою 1-ю строку"
    );
}
