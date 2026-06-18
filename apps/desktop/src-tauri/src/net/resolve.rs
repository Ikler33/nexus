//! Общий DNS-rebinding/SSRF-гард для всего эгресса (P0-a, ADR-005-ext W-аддендум).
//!
//! ЕДИНЫЙ источник истины resolve-then-check-all-IPs-then-pin: домен резолвится (трейт [`Resolver`],
//! в тестах — мок), КАЖДЫЙ полученный IP проверяется на metadata/приватность, затем проверенный IP
//! **пинится** в `reqwest` (`resolve_to_addrs`) — коннект гарантированно идёт на проверенный адрес,
//! а не на повторный резолв атакующего DNS (TOCTOU между check и connect).
//!
//! Был раздублирован в `news::fetch`, `websearch::search`, `commands::plugin` И отсутствовал на
//! core-пути (`GuardedClient`) — публичный домен, резолвящийся в 169.254.169.254 / приватный IP,
//! проходил host-string-гейт и коннектился. Этот модуль закрывает дыру и снимает дубли.
//!
//! IP-уровневые проверки переиспользуют [`is_private_host`]/[`blocks_cloud_metadata`] (через
//! `IpAddr::to_string`) — те уже корректно судят IPv4-mapped (`::ffff:a.b.c.d`), NAT64
//! (`64:ff9b::/96`), 6to4 (`2002::/16`), IPv4-compatible по встроенному v4. Одна логика, без копий.

use std::net::IpAddr;

use async_trait::async_trait;

use super::EgressDenied;
use crate::plugin::{blocks_cloud_metadata, is_private_host};
use crate::redact::Redacted;

/// DNS-резолв для гарда — за трейтом ради офлайн-тестов (мок задаёт фиксированный список IP).
/// Возвращает `io::Result`, чтобы боевой [`SystemResolver`] отдавал нативную сетевую ошибку, а
/// вызывающий мог отличить «резолв упал» от «адрес запрещён».
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str) -> std::io::Result<Vec<IpAddr>>;
}

/// Боевой резолвер на tokio (системный DNS). Порт 0 — нам нужны только адреса, не сокет-таргет.
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str) -> std::io::Result<Vec<IpAddr>> {
        let addrs = tokio::net::lookup_host((host, 0)).await?;
        Ok(addrs.map(|sa| sa.ip()).collect())
    }
}

/// Гард над зарезолвленными адресами (чистая фн — тестируется напрямую, без сети).
///
/// - metadata (169.254.169.254 / IMDS-v6 / любая v4-туннелирующая форма) и link-local
///   (169.254.0.0/16, fe80::/10) отклоняются **ВСЕГДА**, независимо от фичи (E7/AC-EGR-12).
/// - при `deny_private` дополнительно отклоняются приватные/loopback/ULA/unspecified/CGNAT/… —
///   web-класс (NewsFeed/Web/plugin), которому LAN-исключение local-first не положено.
/// - пустой резолв → отказ (нечего пинить).
/// - адрес НЕ включается в ошибку (политика приватности как у [`EgressDenied`], host — [`Redacted`]).
///
/// metadata-проверка идёт ПЕРВОЙ и применяется даже когда `deny_private=false` (chat/embed/probe):
/// LAN-LLM живёт, но IMDS/link-local заблокированы. `is_private_host` уже включает весь link-local
/// диапазон (169.254/16 + fe80::/10) и сам metadata-IP, поэтому при `deny_private` он покрывает и
/// link-local; при `!deny_private` link-local добивает явная metadata-ветка + link-local-чек ниже.
pub fn check_resolved_ips(ips: &[IpAddr], deny_private: bool) -> Result<(), EgressDenied> {
    if ips.is_empty() {
        // Пустой резолв: нечего пинить, нечего проверять — fail-closed.
        return Err(EgressDenied::HostNotAllowed(Redacted::new(String::new())));
    }
    for ip in ips {
        // `to_string` даёт каноническую форму литерала; предикаты сами разбирают v4/v6 и все
        // v4-туннелирующие обёртки (mapped/NAT64/6to4/compatible) — одна логика, без копий.
        let s = ip.to_string();
        // metadata — безусловно (E7/AC-EGR-12).
        let denied = blocks_cloud_metadata(&s)
            // IMDS-v6 (AWS `fd00:ec2::254`) — ULA-литерал cloud-metadata: отклоняется БЕЗУСЛОВНО,
            // как 169.254.169.254. `is_private_host` ловит его лишь при `deny_private`, поэтому без
            // явной ветки rebind публичного AAAA на IMDS-v6 утёк бы инстанс-креды на chat-пути.
            || is_imds_v6(*ip)
            // link-local — безусловно (169.254/16, fe80::/10): rebind на IMDS-соседа/SLAAC-шлюз.
            || is_link_local(*ip)
            // приватный/loopback/ULA/… — только для web-класса.
            || (deny_private && is_private_host(&s));
        if denied {
            return Err(EgressDenied::HostNotAllowed(Redacted::new(s)));
        }
    }
    Ok(())
}

/// Link-local (всегда запрещён, даже для chat/embed/probe): IPv4 169.254.0.0/16 и IPv6 fe80::/10.
/// v4-туннелирующие формы (mapped/NAT64/6to4/compatible) разворачиваются и судятся по встроенному v4.
fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = embedded_ipv4_v6(v6) {
                if v4.is_link_local() {
                    return true;
                }
            }
            (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
        }
    }
}

/// AWS IMDS-v6 (`fd00:ec2::254`) — ULA-литерал cloud-metadata. Отклоняется БЕЗУСЛОВНО (даже chat):
/// metadata-эндпоинт никогда не легитимная цель эгресса, а ULA `is_private_host` режет лишь при
/// `deny_private`. Без этого rebind на IMDS-v6 прошёл бы на chat-пути (актуально для cloud agentd).
fn is_imds_v6(ip: IpAddr) -> bool {
    matches!(
        ip,
        IpAddr::V6(v6) if v6 == std::net::Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)
    )
}

/// Извлекает встроенный IPv4 из v4-туннелирующих v6-форм (mapped/NAT64/6to4/compatible). Зеркало
/// `plugin::permission::embedded_ipv4` (та приватна модулю) — нужно для link-local-разбора v6 здесь.
fn embedded_ipv4_v6(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    use std::net::Ipv4Addr;
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4); // ::ffff:a.b.c.d
    }
    let s = v6.segments();
    let v4_from = |hi: u16, lo: u16| {
        Ipv4Addr::new(
            (hi >> 8) as u8,
            (hi & 0xff) as u8,
            (lo >> 8) as u8,
            (lo & 0xff) as u8,
        )
    };
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return Some(v4_from(s[6], s[7])); // NAT64 64:ff9b::/96
    }
    if s[0] == 0x2002 {
        return Some(v4_from(s[1], s[2])); // 6to4 2002:V4::/16
    }
    if s[..6] == [0, 0, 0, 0, 0, 0] && (s[6] != 0 || s[7] != 0) {
        return Some(v4_from(s[6], s[7])); // IPv4-compatible ::a.b.c.d
    }
    None
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Мок-резолвер: отдаёт фиксированный список IP, считает вызовы (для assert «резолв тронут/нет»).
    pub struct FixedResolver {
        ips: Vec<IpAddr>,
        pub calls: AtomicUsize,
    }

    impl FixedResolver {
        pub fn new(ips: Vec<IpAddr>) -> Self {
            Self {
                ips,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Resolver for FixedResolver {
        async fn resolve(&self, _host: &str) -> std::io::Result<Vec<IpAddr>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.ips.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FixedResolver;
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn is_denied(ips: &[IpAddr], deny_private: bool) -> bool {
        matches!(
            check_resolved_ips(ips, deny_private),
            Err(EgressDenied::HostNotAllowed(_))
        )
    }

    /// metadata IPv4 — отклоняется ВСЕГДА (и для chat, и для web-класса).
    #[test]
    fn metadata_v4_denied_always() {
        let m = [ip("169.254.169.254")];
        assert!(is_denied(&m, false), "metadata denied даже для chat");
        assert!(is_denied(&m, true), "metadata denied для web-класса");
    }

    /// IMDS IPv6 (`fd00:ec2::254`) — ULA-форма cloud-metadata: отклоняется ВСЕГДА, вкл. chat
    /// (metadata-эндпоинт не бывает легитимной целью; иначе rebind публичного AAAA на IMDS-v6 утёк
    /// бы инстанс-креды в облачном agentd, CORE-2). Прочие ULA на chat-пути живут (local-first).
    #[test]
    fn imds_ipv6_denied_always() {
        let m = [ip("fd00:ec2::254")];
        assert!(
            is_denied(&m, false),
            "IMDS-v6 denied даже для chat (безусловно)"
        );
        assert!(is_denied(&m, true), "IMDS-v6 denied для web-класса");
        // Не-IMDS ULA на chat-пути жив — это валидный приватный диапазон (local-first).
        assert_eq!(
            check_resolved_ips(&[ip("fd00:abcd::1")], false),
            Ok(()),
            "не-IMDS ULA для chat живёт (local-first)"
        );
    }

    /// link-local — отклоняется ВСЕГДА: IPv4 169.254.x и IPv6 fe80::.
    #[test]
    fn link_local_denied_always() {
        for s in ["169.254.1.1", "fe80::1", "fe80::dead:beef"] {
            assert!(is_denied(&[ip(s)], false), "{s} link-local denied для chat");
            assert!(is_denied(&[ip(s)], true), "{s} link-local denied для web");
        }
    }

    /// IPv4-mapped: судим по встроенному v4. `::ffff:169.254.169.254` — metadata (всегда);
    /// `::ffff:10.0.0.1` — приватный (только web-класс).
    #[test]
    fn ipv4_mapped_judged_by_embedded() {
        assert!(is_denied(&[ip("::ffff:169.254.169.254")], false));
        assert!(is_denied(&[ip("::ffff:169.254.169.254")], true));
        assert!(
            !is_denied(&[ip("::ffff:10.0.0.1")], false),
            "mapped-приватный для chat живёт (local-first)"
        );
        assert!(
            is_denied(&[ip("::ffff:10.0.0.1")], true),
            "mapped-приватный для web denied"
        );
    }

    /// NAT64 64:ff9b::a9fe:a9fe → встроенный 169.254.169.254 (metadata) — denied всегда.
    #[test]
    fn nat64_to_metadata_denied_always() {
        let m = [ip("64:ff9b::a9fe:a9fe")];
        assert!(is_denied(&m, false));
        assert!(is_denied(&m, true));
    }

    /// 6to4 2002:c0a8:1f:: → встроенный 192.168.0.31 (приватный) — denied для web-класса.
    #[test]
    fn sixtofour_to_private_denied_for_web() {
        let m = [ip("2002:c0a8:1f::")];
        assert!(is_denied(&m, true), "6to4→приватный denied для web");
        assert!(
            !is_denied(&m, false),
            "6to4→приватный для chat живёт (local-first)"
        );
    }

    /// Пустой резолв — всегда отказ (нечего пинить).
    #[test]
    fn empty_resolve_denied() {
        assert!(is_denied(&[], false));
        assert!(is_denied(&[], true));
    }

    /// Обычный приватный LAN-адрес LLM-сервера: chat ALLOW (local-first), web-класс DENY.
    #[test]
    fn plain_private_lan_chat_allow_web_deny() {
        let lan = [ip("192.168.0.31")];
        assert_eq!(
            check_resolved_ips(&lan, false),
            Ok(()),
            "LAN LLM-сервер живёт для chat (local-first)"
        );
        assert!(is_denied(&lan, true), "LAN для web-класса denied");
        // loopback — тоже: chat ALLOW, web DENY.
        assert_eq!(check_resolved_ips(&[ip("127.0.0.1")], false), Ok(()));
        assert!(is_denied(&[ip("127.0.0.1")], true));
    }

    /// Публичный адрес проходит для обоих классов.
    #[test]
    fn public_allowed_both() {
        let pub_ip = [ip("93.184.216.34")];
        assert_eq!(check_resolved_ips(&pub_ip, false), Ok(()));
        assert_eq!(check_resolved_ips(&pub_ip, true), Ok(()));
    }

    /// Multi-A: один публичный + один приватный → DENY для web-класса (ВСЕ обязаны пройти).
    /// Если ЛЮБОЙ адрес — metadata, DENY даже для chat (rebind через множественный A).
    #[test]
    fn multi_a_any_bad_denies() {
        let pub_plus_private = [ip("93.184.216.34"), ip("10.0.0.1")];
        assert!(
            is_denied(&pub_plus_private, true),
            "pub+private → web denied"
        );
        assert_eq!(
            check_resolved_ips(&pub_plus_private, false),
            Ok(()),
            "pub+private для chat — оба ОК (приватный — local-first)"
        );
        let pub_plus_metadata = [ip("93.184.216.34"), ip("169.254.169.254")];
        assert!(
            is_denied(&pub_plus_metadata, false),
            "любой metadata → denied даже для chat"
        );
        assert!(is_denied(&pub_plus_metadata, true));
    }

    /// Мок-резолвер отдаёт фиксированный список и считает вызовы.
    #[tokio::test]
    async fn fixed_resolver_returns_and_counts() {
        let r = FixedResolver::new(vec![ip("93.184.216.34")]);
        let got = r.resolve("example.com").await.unwrap();
        assert_eq!(got, vec![ip("93.184.216.34")]);
        assert_eq!(r.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
