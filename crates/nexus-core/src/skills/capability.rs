//! SKILL-3 (Фаза C): типизированная capability-модель + trust-tier + Phase-C resolve.
//!
//! SKILL-1 ЗАХВАТИЛ объявленные capabilities как СЫРЫЕ строки ([`super::Skill::capabilities`]); они
//! инертны. SKILL-3 связывает их с run-policy на approval-слое — но строго **аддитивно и сужающе**:
//!
//! > **Ключевой инвариант: декларация ЗАПРАШИВАЕТ, а НЕ ГРАНТИТ.** Эффективная способность =
//! > forced-база `{VaultRead, VaultWrite}` ∩ run-policy; всё остальное (shell / web_post /
//! > host_process / web_fetch) остаётся **ИНЕРТНЫМ** и СУРФЕЙСИТСЯ с явной причиной — никогда не
//! > гранится молча, никогда не расширяет blast-radius. Гейт только УЖЕСТОЧАЕТ.
//!
//! ## Почему shell/web/host НЕ МОГУТ стать granted (структурная инертность)
//! [`crate::actuator::action::ActionTarget`] — ТОЛЬКО vault-файлы (NoteCreate/NoteEdit/Frontmatter);
//! [`classify`](crate::actuator::classify::classify) матчит его EXHAUSTIVE, без catch-all. Нет действия, которое исполнит
//! shell/egress/host. Эта capability-модель НЕ открывает такой путь: [`resolve_capabilities`]
//! НИКОГДА не кладёт `WebFetch/WebPost/Shell/HostProcess` в `granted` в Фазе C (проверено тестом-
//! инвариантом). Trust-tier здесь — ADVISORY: захвачен + сурфейсится, но НЕ проведён в живой гейт.

use std::collections::BTreeSet;

/// Типизированная способность скилла. Объявленные строки (`capabilities:`/`allowed-tools:`)
/// разбираются в этот enum через [`parse_capabilities`]. Нераспознанный токен → [`Capability::Unknown`]
/// (сохраняется как advisory — не ошибка здесь; битый-СПИСОК был жёсткой ошибкой ещё в SKILL-1-парсере).
///
/// `VaultRead`/`VaultWrite` — единственные, что могут попасть в `granted` в Фазе C (forced-база).
/// Остальные — заявляемы, но СТРУКТУРНО инертны (нет actuator-пути; см. модульную доку).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// Чтение vault (заметки/ресурсы). Forced-база Фазы C.
    VaultRead,
    /// Запись в vault (создание/правка заметок через actuator-гейт). Forced-база Фазы C.
    VaultWrite,
    /// Исходящий HTTP GET (fetch). ИНЕРТНО в Фазе C — нет actuator-пути, egress только через
    /// `net::GuardedClient` вне skill-гейта.
    WebFetch,
    /// Исходящий HTTP POST (отправка данных наружу). ИНЕРТНО в Фазе C (деструктив-egress).
    WebPost,
    /// Исполнение shell-команд. ИНЕРТНО в Фазе C — нет shell-actuator (структурно).
    Shell,
    /// Запуск/контроль хост-процессов. ИНЕРТНО в Фазе C — нет host-actuator.
    HostProcess,
    /// Нераспознанный объявленный токен (сохранён как advisory). Никогда не гранится.
    Unknown(String),
}

impl Capability {
    /// Разбирает ОДИН объявленный токен в типизированную способность. Регистронезависимо по
    /// каноническим именам + распространённым алиасам (`net`/`http`/`fetch` → `WebFetch` и т.п.).
    /// Нераспознанный → [`Capability::Unknown`] (исходная строка сохраняется как есть).
    pub fn parse(token: &str) -> Self {
        let norm = token.trim().to_ascii_lowercase();
        match norm.as_str() {
            "vault_read" | "vault-read" | "vaultread" | "read" => Capability::VaultRead,
            "vault_write" | "vault-write" | "vaultwrite" | "write" => Capability::VaultWrite,
            "web_fetch" | "web-fetch" | "webfetch" | "fetch" | "net" | "http" | "web" => {
                Capability::WebFetch
            }
            "web_post" | "web-post" | "webpost" | "post" => Capability::WebPost,
            "shell" | "bash" | "exec" | "command" => Capability::Shell,
            "host_process" | "host-process" | "hostprocess" | "host" | "process" => {
                Capability::HostProcess
            }
            _ => Capability::Unknown(token.trim().to_string()),
        }
    }

    /// Человекочитаемая метка для advisory-сообщений (стабильная, для сурфейсинга в активации/каталоге).
    pub fn label(&self) -> String {
        match self {
            Capability::VaultRead => "VaultRead".to_string(),
            Capability::VaultWrite => "VaultWrite".to_string(),
            Capability::WebFetch => "WebFetch".to_string(),
            Capability::WebPost => "WebPost".to_string(),
            Capability::Shell => "Shell".to_string(),
            Capability::HostProcess => "HostProcess".to_string(),
            Capability::Unknown(s) => format!("Unknown({s})"),
        }
    }
}

/// Разбирает СПИСОК объявленных capability-строк в типизированные способности (token-map). Порядок
/// сохраняется; дубликаты не схлопываются (вход уже дедуплен SKILL-1-парсером, но это не предполагается).
/// Никогда не ошибается здесь (нераспознанный → [`Capability::Unknown`]) — битый-СПИСОК уже был
/// жёсткой ошибкой [`super::SkillError::BadCapabilities`] на этапе [`super::parse_skill`].
pub fn parse_capabilities(declared: &[String]) -> Vec<Capability> {
    declared.iter().map(|s| Capability::parse(s)).collect()
}

/// Уровень доверия скилла, ВЫВЕДЕННЫЙ ИЗ ПУТИ (rel_path относительно skills_dir).
///
/// - [`TrustTier::Vendor`] — путь под `vendor/<bundle>/…` (вендоренный сторонний bundle: hash-pin +
///   обязательная лицензия применяются загрузчиком).
/// - [`TrustTier::TrustedLocal`] — всё прочее под корнем skills (локальный скилл владельца; не
///   hash-pinned).
///
/// **Tier — ADVISORY в Фазе C** (PLAN: «declared advisory; эффективный = forced ∩ run-policy»): он
/// ЗАХВАЧЕН и СУРФЕЙСИТСЯ, но НЕ проведён в живой actuator-гейт ([`classify`](crate::actuator::classify::classify)/
/// [`DecisionSource`](crate::actuator::decision::DecisionSource)). Маппинг «дефолт согласования» —
/// будущий (Фаза-3) шов: [`approval_default`] — ЧИСТЫЙ, протестированный, но ПОКА НЕ ВЫЗЫВАЕМЫЙ хелпер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTier {
    /// Локальный скилл владельца (под корнем skills, НЕ под `vendor/`). Не hash-pinned.
    TrustedLocal,
    /// Вендоренный сторонний bundle (`vendor/<bundle>/…`). Hash-pin + лицензия обязательны.
    Vendor,
}

impl TrustTier {
    /// Выводит tier из `rel_path` скилла (относительно skills_dir, `/`-разделён). Путь, первый
    /// компонент которого `vendor`, — [`TrustTier::Vendor`]; иначе [`TrustTier::TrustedLocal`].
    ///
    /// Лексический разбор (без ФС): `rel_path` уже прошёл path-scope-проверку discovery (см.
    /// [`super::discover_skills`]). Учитываем только ПЕРВЫЙ компонент — `foo/vendor/...` НЕ vendor
    /// (vendor-корень всегда на верхнем уровне skills_dir per PLAN-раскладке).
    pub fn from_rel_path(rel_path: &str) -> Self {
        let first = rel_path
            .split('/')
            .find(|seg| !seg.is_empty() && *seg != ".");
        if first == Some(VENDOR_DIR) {
            TrustTier::Vendor
        } else {
            TrustTier::TrustedLocal
        }
    }
}

/// Имя верхнеуровневого каталога вендоренных bundle'ов внутри skills_dir.
pub const VENDOR_DIR: &str = "vendor";

/// Класс риска ДЕЙСТВИЯ для будущего маппинга дефолта согласования ([`approval_default`]). НЕ путать с
/// [`crate::actuator::classify::RiskTier`] (тот — живой гейт по vault-путям). Это грубая ось «обратимое
/// vault-действие ↔ деструктив/egress/host» для Фазы-3-шва, который ПОКА НЕ ПОДКЛЮЧЁН.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskClass {
    /// Обратимое vault-действие (read / reversible write). Кандидат на auto при доверенном tier.
    ReversibleVault,
    /// Деструктив / egress-POST / host — ВСЕГДА требует human-DIFF, при ЛЮБОМ tier.
    DestructiveOrEgress,
}

/// Дефолт согласования для будущего (Фаза-3) контрол-плейна. **ПОКА НЕ ВЫЗЫВАЕТСЯ живым гейтом**
/// (см. CRITICAL-инвариант среза): trust-tier в Фазе C — ADVISORY. Хелпер ЧИСТЫЙ и протестирован,
/// чтобы Фаза-3 могла подключить его без переоткрытия security-границы.
///
/// Возвращает [`ApprovalDefault::Auto`] ТОЛЬКО для обратимого vault-действия доверенного tier'а;
/// **НИКОГДА** не возвращает `Auto` для [`RiskClass::DestructiveOrEgress`] — при ЛЮБОМ tier (инвариант,
/// проверенный тестом). Это та же fail-closed-философия, что и [`crate::actuator::decision`].
pub fn approval_default(tier: TrustTier, risk: RiskClass) -> ApprovalDefault {
    match risk {
        // ИНВАРИАНТ: деструктив/egress/host — human-DIFF ВСЕГДА, независимо от tier.
        RiskClass::DestructiveOrEgress => ApprovalDefault::HumanDiff,
        RiskClass::ReversibleVault => match tier {
            // Доверенный/вендоренный + обратимое vault → кандидат на auto (Фаза-3).
            TrustTier::TrustedLocal | TrustTier::Vendor => ApprovalDefault::Auto,
        },
    }
}

/// Решение [`approval_default`] (Фаза-3-шов, ПОКА НЕ ПОДКЛЮЧЁН).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDefault {
    /// Можно по умолчанию авто-согласовать (только обратимое vault + доверенный tier).
    Auto,
    /// Требует человеческого DIFF-подтверждения (деструктив/egress/host — ВСЕГДА).
    HumanDiff,
}

/// Phase-C run-policy: какие способности РАЗРЕШЕНЫ режимом прогона. В Фазе C это лишь vault-подмножество
/// (egress/shell/host недоступны режиму вовсе). Эффективный grant = forced-база ∩ эта политика.
///
/// Конструируется композиционным корнем под режим прогона; [`RunPolicy::phase_c_vault`] — дефолт Фазы C
/// (читать+писать vault). Узкие политики (только-чтение) выражаются меньшим набором.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPolicy {
    /// Способности, разрешённые режимом прогона. В Фазе C — подмножество `{VaultRead, VaultWrite}`.
    allowed: BTreeSet<Capability>,
}

impl RunPolicy {
    /// Произвольная политика из набора разрешённых способностей.
    pub fn new(allowed: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }

    /// Дефолт Фазы C: vault read+write (zero-setup для kepano-скиллов). Никакого egress/shell/host.
    pub fn phase_c_vault() -> Self {
        Self::new([Capability::VaultRead, Capability::VaultWrite])
    }

    /// Только-чтение vault (узкая политика; для прогонов, которым нельзя писать).
    pub fn read_only() -> Self {
        Self::new([Capability::VaultRead])
    }

    /// Разрешена ли способность этой политикой.
    pub fn allows(&self, cap: &Capability) -> bool {
        self.allowed.contains(cap)
    }
}

/// Forced-база способностей Фазы C: vault read+write, выдаются скиллам zero-setup. Эффективный grant
/// = эта база ∩ run-policy. НИЧЕГО за пределами этого набора не может попасть в `granted` в Фазе C.
pub fn forced_base() -> [Capability; 2] {
    [Capability::VaultRead, Capability::VaultWrite]
}

/// Результат [`resolve_capabilities`]: что РЕАЛЬНО выдано (`granted`) и что ОБЪЯВЛЕНО-НО-ИНЕРТНО
/// (`inert`, каждая с человекочитаемой причиной). `granted` — всегда ⊆ forced-база ∩ run-policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityResolution {
    /// Реально выданные способности (forced-база ∩ run-policy). В Фазе C — только vault-подмножество.
    pub granted: Vec<Capability>,
    /// Объявленные, но НЕ выданные способности, каждая с причиной инертности (для сурфейсинга).
    pub inert: Vec<(Capability, String)>,
}

impl CapabilityResolution {
    /// Есть ли инертные (объявленные-но-недоступные) способности — нужен ли advisory-блок в активации.
    pub fn has_inert(&self) -> bool {
        !self.inert.is_empty()
    }
}

/// **Phase-C resolve: declared ЗАПРАШИВАЕТ, forced∩policy ГРАНТИТ.**
///
/// `granted` = forced-база (`{VaultRead, VaultWrite}`) ∩ `run_policy`. Объявленные способности —
/// ADVISORY: они НЕ расширяют `granted`. `inert` = каждая ОБЪЯВЛЕННАЯ способность, которая НЕ попала
/// в `granted`, с причиной («недоступно в этом режиме» / «egress выключен» / «shell не поддержан в
/// Фазе C» / …). `trust_tier` принимается для сигнатуры будущего шва (Фаза-3), но в Фазе C НЕ влияет
/// на `granted` — это и есть «advisory» (намеренно `_`-помечен).
///
/// **ИНВАРИАНТ (проверен тестом):** `WebFetch`/`WebPost`/`Shell`/`HostProcess` НИКОГДА не в `granted`
/// в Фазе C — даже если скилл их объявил и даже под Vendor-tier. forced-база их не содержит, а declared
/// не расширяет grant → структурно невозможно.
pub fn resolve_capabilities(
    declared: &[Capability],
    _trust_tier: TrustTier,
    run_policy: &RunPolicy,
) -> CapabilityResolution {
    // granted = forced-база ∩ run-policy. ТОЛЬКО отсюда — declared НЕ добавляет.
    let granted: Vec<Capability> = forced_base()
        .into_iter()
        .filter(|c| run_policy.allows(c))
        .collect();

    // inert = объявленные способности, которых нет в granted. Дедуп с сохранением порядка
    // (объявить дважды одну → одна inert-строка). Причина — по типу способности.
    let mut inert: Vec<(Capability, String)> = Vec::new();
    for cap in declared {
        if granted.contains(cap) {
            continue; // объявлено И выдано — не инертно.
        }
        if inert.iter().any(|(c, _)| c == cap) {
            continue; // уже зафиксировано.
        }
        inert.push((cap.clone(), inert_reason(cap, run_policy)));
    }

    CapabilityResolution { granted, inert }
}

/// Причина инертности конкретной объявленной-но-невыданной способности (для сурфейсинга юзеру/модели).
fn inert_reason(cap: &Capability, run_policy: &RunPolicy) -> String {
    match cap {
        // vault-способность объявлена, но НЕ в granted ⇒ её отсёк run-policy (узкий режим).
        Capability::VaultRead | Capability::VaultWrite => {
            if run_policy.allows(cap) {
                // Теоретически недостижимо (была бы в granted), но честно подстрахуемся.
                "недоступно в этом режиме".to_string()
            } else {
                "недоступно в этом режиме (запрещено политикой прогона)".to_string()
            }
        }
        Capability::WebFetch => "egress выключен (web_fetch недоступен в Фазе C)".to_string(),
        Capability::WebPost => "egress выключен (web_post недоступен в Фазе C)".to_string(),
        Capability::Shell => "shell не поддержан в Фазе C".to_string(),
        Capability::HostProcess => "host-процессы не поддержаны в Фазе C".to_string(),
        Capability::Unknown(s) => format!("неизвестная способность «{s}» — игнорируется"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_capabilities ──────────────────────────────────────────────────────────────────────

    /// Известные токены → типизированные способности.
    #[test]
    fn parse_known_tokens() {
        let declared: Vec<String> = [
            "vault_read",
            "vault_write",
            "web_fetch",
            "web_post",
            "shell",
            "host_process",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let caps = parse_capabilities(&declared);
        assert_eq!(
            caps,
            vec![
                Capability::VaultRead,
                Capability::VaultWrite,
                Capability::WebFetch,
                Capability::WebPost,
                Capability::Shell,
                Capability::HostProcess,
            ]
        );
    }

    /// Алиасы (read/write/net/bash/host) и регистр распознаются.
    #[test]
    fn parse_aliases_and_case() {
        assert_eq!(Capability::parse("READ"), Capability::VaultRead);
        assert_eq!(Capability::parse("Write"), Capability::VaultWrite);
        assert_eq!(Capability::parse("net"), Capability::WebFetch);
        assert_eq!(Capability::parse("http"), Capability::WebFetch);
        assert_eq!(Capability::parse("Bash"), Capability::Shell);
        assert_eq!(Capability::parse("host"), Capability::HostProcess);
    }

    /// Нераспознанный токен → Unknown(сохранён как есть), не ошибка.
    #[test]
    fn parse_unknown_kept() {
        let caps = parse_capabilities(&["frobnicate".to_string()]);
        assert_eq!(caps, vec![Capability::Unknown("frobnicate".to_string())]);
    }

    /// Пустой список → пустой результат.
    #[test]
    fn parse_empty_is_empty() {
        assert!(parse_capabilities(&[]).is_empty());
    }

    // ── resolve_capabilities ────────────────────────────────────────────────────────────────────

    /// Объявлено {VaultRead, VaultWrite} под дефолт-политикой Фазы C → всё granted, inert пуст.
    #[test]
    fn resolve_vault_only_all_granted_no_inert() {
        let declared = vec![Capability::VaultRead, Capability::VaultWrite];
        let res = resolve_capabilities(&declared, TrustTier::Vendor, &RunPolicy::phase_c_vault());
        assert_eq!(
            res.granted,
            vec![Capability::VaultRead, Capability::VaultWrite]
        );
        assert!(res.inert.is_empty(), "нет инертных: {:?}", res.inert);
        assert!(!res.has_inert());
    }

    /// Объявлено {Shell, WebPost, VaultRead} → granted {VaultRead, VaultWrite (forced)}; inert
    /// {Shell, WebPost} с причинами. forced-база даёт VaultWrite даже без объявления.
    #[test]
    fn resolve_mixed_grants_vault_inerts_rest() {
        let declared = vec![
            Capability::Shell,
            Capability::WebPost,
            Capability::VaultRead,
        ];
        let res = resolve_capabilities(
            &declared,
            TrustTier::TrustedLocal,
            &RunPolicy::phase_c_vault(),
        );
        // granted = forced ∩ policy = {VaultRead, VaultWrite} (порядок forced-базы).
        assert_eq!(
            res.granted,
            vec![Capability::VaultRead, Capability::VaultWrite]
        );
        // inert = объявленные не-granted: Shell, WebPost (VaultRead — granted, не инертна).
        let inert_caps: Vec<&Capability> = res.inert.iter().map(|(c, _)| c).collect();
        assert_eq!(inert_caps, vec![&Capability::Shell, &Capability::WebPost]);
        // У каждой инертной — непустая причина.
        for (_, reason) in &res.inert {
            assert!(!reason.is_empty(), "причина не пуста");
        }
        assert!(res.has_inert());
    }

    /// **ИНВАРИАНТ (параметризован):** Shell/WebPost/HostProcess/WebFetch НИКОГДА не в granted в Фазе C —
    /// для ЛЮБОГО tier и даже если объявлены. forced-база их не содержит, declared не расширяет.
    #[test]
    fn invariant_dangerous_caps_never_granted() {
        let dangerous = [
            Capability::WebFetch,
            Capability::WebPost,
            Capability::Shell,
            Capability::HostProcess,
        ];
        for tier in [TrustTier::TrustedLocal, TrustTier::Vendor] {
            for policy in [
                RunPolicy::phase_c_vault(),
                RunPolicy::read_only(),
                RunPolicy::new(dangerous.clone()),
            ] {
                // Объявляем ВСЕ опасные способности (declared = запрос).
                let declared: Vec<Capability> = dangerous.to_vec();
                let res = resolve_capabilities(&declared, tier, &policy);
                for d in &dangerous {
                    assert!(
                        !res.granted.contains(d),
                        "{:?} НЕ должна быть granted (tier={tier:?}): granted={:?}",
                        d,
                        res.granted
                    );
                }
            }
        }
    }

    /// Узкая read-only политика отсекает VaultWrite из forced-базы: granted={VaultRead}; объявленный
    /// VaultWrite становится inert (forced∩policy сужает).
    #[test]
    fn resolve_read_only_policy_narrows_grant() {
        let declared = vec![Capability::VaultRead, Capability::VaultWrite];
        let res = resolve_capabilities(&declared, TrustTier::Vendor, &RunPolicy::read_only());
        assert_eq!(res.granted, vec![Capability::VaultRead]);
        let inert_caps: Vec<&Capability> = res.inert.iter().map(|(c, _)| c).collect();
        assert_eq!(inert_caps, vec![&Capability::VaultWrite]);
    }

    /// trust_tier НЕ влияет на granted (advisory): один declared+policy даёт одинаковый granted при
    /// разных tier.
    #[test]
    fn resolve_trust_tier_is_advisory_not_wired() {
        let declared = vec![Capability::VaultRead, Capability::Shell];
        let local = resolve_capabilities(
            &declared,
            TrustTier::TrustedLocal,
            &RunPolicy::phase_c_vault(),
        );
        let vendor =
            resolve_capabilities(&declared, TrustTier::Vendor, &RunPolicy::phase_c_vault());
        assert_eq!(local.granted, vendor.granted, "tier не меняет grant");
    }

    /// Unknown-способность объявлена → инертна с причиной (не гранится, не ошибка).
    #[test]
    fn resolve_unknown_is_inert() {
        let declared = vec![Capability::Unknown("frob".to_string())];
        let res = resolve_capabilities(
            &declared,
            TrustTier::TrustedLocal,
            &RunPolicy::phase_c_vault(),
        );
        assert!(res
            .granted
            .iter()
            .all(|c| matches!(c, Capability::VaultRead | Capability::VaultWrite)));
        assert_eq!(res.inert.len(), 1);
        assert!(matches!(res.inert[0].0, Capability::Unknown(_)));
    }

    /// Дубль объявленной инертной способности → одна inert-строка (дедуп).
    #[test]
    fn resolve_dedup_inert() {
        let declared = vec![Capability::Shell, Capability::Shell];
        let res = resolve_capabilities(
            &declared,
            TrustTier::TrustedLocal,
            &RunPolicy::phase_c_vault(),
        );
        assert_eq!(res.inert.len(), 1, "дубль схлопнут: {:?}", res.inert);
    }

    // ── TrustTier::from_rel_path ─────────────────────────────────────────────────────────────────

    /// Путь под vendor/ → Vendor.
    #[test]
    fn tier_vendor_from_path() {
        assert_eq!(
            TrustTier::from_rel_path("vendor/kepano/obsidian-markdown/SKILL.md"),
            TrustTier::Vendor
        );
    }

    /// Путь НЕ под vendor/ → TrustedLocal.
    #[test]
    fn tier_trusted_local_from_path() {
        assert_eq!(
            TrustTier::from_rel_path("my-skill/SKILL.md"),
            TrustTier::TrustedLocal
        );
        assert_eq!(TrustTier::from_rel_path("flat.md"), TrustTier::TrustedLocal);
    }

    /// `vendor` не на верхнем уровне (`foo/vendor/...`) → НЕ Vendor (vendor-корень всегда верхний).
    #[test]
    fn tier_vendor_only_top_level() {
        assert_eq!(
            TrustTier::from_rel_path("foo/vendor/bar/SKILL.md"),
            TrustTier::TrustedLocal
        );
    }

    // ── approval_default (Фаза-3-шов, НЕ подключён) ──────────────────────────────────────────────

    /// **ИНВАРИАНТ:** деструктив/egress/host → НИКОГДА Auto, при ЛЮБОМ tier.
    #[test]
    fn approval_default_never_auto_for_destructive() {
        for tier in [TrustTier::TrustedLocal, TrustTier::Vendor] {
            assert_eq!(
                approval_default(tier, RiskClass::DestructiveOrEgress),
                ApprovalDefault::HumanDiff,
                "tier={tier:?}: деструктив/egress ВСЕГДА human-DIFF"
            );
        }
    }

    /// Обратимое vault + доверенный/вендоренный tier → Auto (Фаза-3-кандидат).
    #[test]
    fn approval_default_reversible_vault_is_auto() {
        assert_eq!(
            approval_default(TrustTier::TrustedLocal, RiskClass::ReversibleVault),
            ApprovalDefault::Auto
        );
        assert_eq!(
            approval_default(TrustTier::Vendor, RiskClass::ReversibleVault),
            ApprovalDefault::Auto
        );
    }
}
