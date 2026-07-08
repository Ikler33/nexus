//! Сетевой фетчер ленты (NF-4, AC-NF-7/8): единственная реализация [`FeedFetcher`] для прода —
//! через `net::GuardedClient` с `EgressFeature::NewsFeed` и лимитами W3 (таймаут 20 с,
//! body-cap 4 МБ — превышение видимой ошибкой источника).
//!
//! **DNS-rebinding-гард** (W-аддендум, resolve-then-connect-check БЕЗ TOCTOU): домен источника
//! резолвится (трейт [`Resolver`], в тестах — мок), КАЖДЫЙ полученный IP проверяется на
//! приватность/metadata; затем проверенный IP **пинится** в клиент (`reqwest resolve override`) —
//! коннект гарантированно идёт на проверенный адрес, а не на повторный резолв атакующего DNS.
//! Политика (`EgressPolicy::check`) при этом отрабатывает как обычно поверх ИМЕНИ хоста.
//!
//! P0-a: общий гард вынесен в [`crate::net::resolve`] (единый источник истины); здесь — тонкая
//! обёртка `check_resolved_ips(host, ips)` (web-класс, `deny_private=true`) с прежним текстом ошибки.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;

use super::FeedFetcher;
// `Resolver`/`SystemResolver` — реэкспорт общего модуля (P0-a): один трейт на весь эгресс.
use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient, RunCtx};
pub use crate::net::{Resolver, SystemResolver};

/// W3: таймаут запроса фида и потолок тела ответа.
/// Таймауты фетча (W3, переосмысление 2026-06-11): прежний ЕДИНЫЙ 20-секундный `timeout()`
/// срабатывал ПОСРЕДИ тела у медленных-но-здоровых фидов (GitHub releases.atom отдаёт ~14 КБ/с →
/// 421 КБ за ~27 с) и маскировался под «error decoding response body». Анти-зависание теперь
/// держат connect-таймаут + inactivity-таймаут чтения; общий потолок — страховка от капельницы
/// (вместе с body-cap 4 МБ время всё равно ограничено).
/// User-Agent фетчера новостей/статей. Браузерный, НЕ honest-bot: антибот Хабра (Qrator) запросы
/// без UA / с bot-UA душит «капельницей» и рвёт соединение (замер 2026-06-11: без UA — 64 КБ и
/// Connection reset / таймаут; с браузерным UA — 176 КБ за 19 с целиком). Это локальный личный
/// ридер, статьи открывает человек кликом — UA обычного браузера честен по сути.
const FEED_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

const FEED_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const FEED_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const FEED_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// 2→4 МБ (2026-06-11): Substack-фиды с полными текстами (raschka — 2.3 МБ) легитимно больше 2 МБ;
/// потолок остаётся — это анти-DoS, а не норматив размера.
pub const FEED_BODY_CAP: usize = 4 * 1024 * 1024;

/// Гард web-класса (NewsFeed/Web): ВСЕ зарезолвленные IP обязаны быть публичными и не-metadata
/// (AC-NF-8) — иначе домен отклоняется ДО коннекта. Пустой резолв — тоже отказ (нечего пинить).
/// Тонкая обёртка над общим [`crate::net::check_resolved_ips`] (`deny_private=true`): прежний текст
/// ошибки (адрес НЕ утекает) сохранён для совместимости с вызывающими/тестами.
pub fn check_resolved_ips(host: &str, ips: &[std::net::IpAddr]) -> Result<(), String> {
    crate::net::check_resolved_ips(ips, true).map_err(|_| {
        if ips.is_empty() {
            "dns: пустой резолв".to_string()
        } else {
            format!("dns-гард: домен {host} резолвится в приватный/metadata адрес")
        }
    })
}

/// Прод-фетчер: на каждый запрос — резолв → гард → guarded-GET c пином проверенного IP.
pub struct GuardedNewsFetcher {
    policy: Arc<EgressPolicy>,
    audit: Arc<EgressAudit>,
    resolver: Arc<dyn Resolver>,
}

impl GuardedNewsFetcher {
    pub fn new(
        policy: Arc<EgressPolicy>,
        audit: Arc<EgressAudit>,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        Self {
            policy,
            audit,
            resolver,
        }
    }
}

#[async_trait]
impl FeedFetcher for GuardedNewsFetcher {
    async fn fetch(&self, url: &str) -> Result<String, String> {
        let parsed = reqwest::Url::parse(url).map_err(|_| "некорректный URL".to_string())?;
        let host = parsed
            .host_str()
            .ok_or_else(|| "URL без хоста".to_string())?
            .to_string();

        // Быстрый отказ политики ДО DNS (выключенная фича/офлайн/не в allowlist) — без сети.
        self.policy
            .check(&host, EgressFeature::NewsFeed)
            .map_err(|e| e.to_string())?;

        // DNS-гард (AC-NF-8): резолв → проверка ВСЕХ IP (общий [`check_resolved_ips`], P0-a) → пин
        // первого проверенного в клиент. Тот же `resolver` инъектится в core-`GuardedClient`, чтобы
        // его собственный P0-a-гард работал поверх ТОГО ЖЕ резолва (а не системного DNS повторно).
        let ips = self
            .resolver
            .resolve(&host)
            .await
            .map_err(|e| format!("dns: {e}"))?;
        check_resolved_ips(&host, &ips)?;
        let pinned = SocketAddr::new(ips[0], parsed.port_or_known_default().unwrap_or(443));

        let pin_host = host.clone();
        let client = GuardedClient::new(self.policy.clone(), self.audit.clone(), move |b| {
            b.user_agent(FEED_USER_AGENT)
                .connect_timeout(FEED_CONNECT_TIMEOUT)
                .read_timeout(FEED_READ_TIMEOUT)
                .timeout(FEED_TOTAL_TIMEOUT)
                .resolve_to_addrs(&pin_host, &[pinned])
        })
        .map_err(|e| e.to_string())?
        .with_resolver(self.resolver.clone());
        let resp = client
            // Лента новостей вне прогона агента → RunCtx::NONE.
            .get(url, EgressFeature::NewsFeed, RunCtx::NONE)
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("статус {}", resp.status()));
        }
        read_body_capped(resp, FEED_BODY_CAP).await
    }
}

/// Читает тело чанками до `cap`; превышение — видимая ошибка источника (W3, no silent caps).
pub async fn read_body_capped(mut resp: reqwest::Response, cap: usize) -> Result<String, String> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len() + chunk.len() > cap {
            return Err(format!("body-cap: тело фида больше {} КБ", cap / 1024));
        }
        buf.extend_from_slice(&chunk);
    }
    // Фиды иногда в legacy-кодировках (latin-1/cp1252) — строгий reject ронял бы их целиком. Lossy
    // (невалидные байты → U+FFFD), как уже делает CDATA-парсер; контент недоверенный в любом случае (аудит).
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FixedResolver {
        ips: Vec<IpAddr>,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl Resolver for FixedResolver {
        async fn resolve(&self, _host: &str) -> std::io::Result<Vec<IpAddr>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.ips.clone())
        }
    }

    fn news_policy(hosts: &[&str]) -> Arc<EgressPolicy> {
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        policy.set_feature_enabled(EgressFeature::NewsFeed, true);
        policy.set_scoped_allowlist("news", hosts.iter().map(|s| s.to_string()));
        policy
    }

    /// AC-NF-8: домен из allowlist, резолвящийся в приватный (192.168.x) или metadata
    /// (169.254.169.254) адрес, отклоняется ДО коннекта — резолвер вызван, сокета нет.
    #[tokio::test]
    async fn dns_guard_rejects_private_and_metadata_resolution() {
        // IPv4-mapped IPv6 (::ffff:…) тоже отклоняются — иначе обход гарда через v6-форму (аудит 2026-06).
        for bad in [
            "192.168.1.10",
            "169.254.169.254",
            "127.0.0.1",
            "::ffff:192.168.1.10",
            "::ffff:169.254.169.254",
        ] {
            let resolver = Arc::new(FixedResolver {
                ips: vec![bad.parse().unwrap()],
                calls: AtomicUsize::new(0),
            });
            let fetcher = GuardedNewsFetcher::new(
                news_policy(&["feeds.example.com"]),
                Arc::new(EgressAudit::default()),
                resolver.clone(),
            );
            let err = fetcher
                .fetch("https://feeds.example.com/rss.xml")
                .await
                .expect_err("обязан отклонить");
            assert!(err.contains("dns-гард"), "{bad}: {err}");
            assert!(!err.contains(bad), "адрес не утекает в текст ошибки: {err}");
            assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        }
        // И даже один плохой адрес среди хороших — отказ (rebinding через множественный A).
        let mixed = Arc::new(FixedResolver {
            ips: vec![
                "93.184.216.34".parse().unwrap(),
                "10.0.0.1".parse().unwrap(),
            ],
            calls: AtomicUsize::new(0),
        });
        let fetcher = GuardedNewsFetcher::new(
            news_policy(&["feeds.example.com"]),
            Arc::new(EgressAudit::default()),
            mixed,
        );
        assert!(fetcher.fetch("https://feeds.example.com/x").await.is_err());
    }

    /// AC-NF-7: политика режет ДО DNS — выключенная фича и хост вне allowlist не доходят
    /// до резолвера (0 вызовов).
    #[tokio::test]
    async fn policy_denies_before_dns() {
        let resolver = Arc::new(FixedResolver {
            ips: vec!["93.184.216.34".parse().unwrap()],
            calls: AtomicUsize::new(0),
        });
        // Фича выключена (дефолт).
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        let fetcher =
            GuardedNewsFetcher::new(policy, Arc::new(EgressAudit::default()), resolver.clone());
        assert!(fetcher.fetch("https://feeds.example.com/x").await.is_err());
        // Хост вне allowlist (фича включена).
        let fetcher2 = GuardedNewsFetcher::new(
            news_policy(&["other.example.com"]),
            Arc::new(EgressAudit::default()),
            resolver.clone(),
        );
        assert!(fetcher2.fetch("https://feeds.example.com/x").await.is_err());
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 0, "DNS не тронут");
    }

    /// W3 body-cap: тело больше лимита → видимая ошибка (читаем чанками, не копим бесконечно).
    #[tokio::test]
    async fn body_cap_rejects_oversized_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let body = vec![b'x'; 4096];
                let _ = sock.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
                );
                let _ = sock.write_all(&body);
            }
        });
        // Кап-логика отдельно от news-политики: unchecked-клиент (loopback живёт у Chat-фич).
        let client = GuardedClient::unchecked();
        let resp = client
            .get(
                &format!("http://{addr}/big.xml"),
                EgressFeature::Probe,
                RunCtx::NONE,
            )
            .await
            .unwrap();
        let err = read_body_capped(resp, 1024).await.expect_err("больше капа");
        assert!(err.contains("body-cap"), "{err}");
        server.join().unwrap();
    }
}
