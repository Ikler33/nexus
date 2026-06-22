//! `DecisionSource` (AGENT-3d) — ОДИН путь принятия решения по предложенному changeset'у, МНОГО входов.
//!
//! Когда автономный гейт ([`super::orchestrate`]) РЕШАЕТ предложить (confirm-run, или Confirm-тир при
//! любой автономии, или Auto-тир за blast-radius-кэпом), он собирает [`ProposalBatch`] и спрашивает
//! [`DecisionSource::decide`]. Источник возвращает [`BatchDecision`] — пер-айтемное Approve/Reject.
//! Гейт применяет ТОЛЬКО одобренные; всё остальное — отклоняет. Это шов «предложение → решение»: за
//! одним трейтом — разные стороны, кормящие решение (политика по умолчанию, тест, будущий UI/контрол-
//! плейн agentd). 3d не проводит его в живой agentd (это 3e) — здесь источник конструируется и тестится.
//!
//! ## Fail-closed по умолчанию (keystone)
//! [`PolicyDefault`] возвращает **Reject для ВСЕХ** айтемов. Это дефолт unattended-agentd: пока нет
//! реального контрол-плейна/UI, кормящего решения, headless-агент НИКОГДА не «само-одобряет» Confirm.
//! И [`BatchDecision`] fail-closed на ВТОРОМ рубеже: любой айтем БЕЗ явной записи в `per_item` трактуется
//! как [`ItemDecision::Reject`] ([`BatchDecision::decision_for`]). Так даже частичный/битый ответ
//! источника (пропустил айтем) НЕ приводит к применению — пропуск == отказ.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::classify::RiskTier;

/// Один предложенный айтем в батче — то, по чему принимается решение. Несёт ledger-`action_id`
/// (адрес решения), относительный путь цели, тир риска (почему предложено) и пер-файловый диф (add/del).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalItem {
    /// `id` строки `agent_actions` (state=proposed) — ключ решения в [`BatchDecision::per_item`].
    pub action_id: i64,
    /// vault-rel путь цели (для отображения/лога).
    pub target_rel: String,
    /// Тир риска, из-за которого айтем попал в предложение (Confirm-тир, либо Auto за blast-radius-кэпом).
    pub tier: RiskTier,
    /// Простой line-diff (current → proposed): добавлено / удалено строк.
    pub add: u32,
    /// Удалено строк.
    pub del: u32,
}

/// Батч предложений одного прогона, переданный [`DecisionSource`] на решение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalBatch {
    /// Прогон-владелец (корреляция).
    pub run_id: i64,
    /// Предложенные айтемы (каждый адресуется своим `action_id`).
    pub items: Vec<ProposalItem>,
}

/// Решение по ОДНОМУ айтему.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemDecision {
    /// Одобрить — действие пойдёт в apply.
    Approve,
    /// Отклонить — действие НЕ применяется (диск не трогаем).
    Reject,
}

/// Решение по батчу — пер-айтемная карта `action_id → решение`. **Fail-closed:** отсутствующий ключ
/// трактуется как [`ItemDecision::Reject`] ([`decision_for`](BatchDecision::decision_for)). Источник
/// НЕ обязан перечислять отказы — пропуск айтема == отказ (частичный ответ не может ничего «протащить»).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchDecision {
    /// Явные решения по `action_id`. Отсутствие ключа ⇒ Reject (см. [`decision_for`](Self::decision_for)).
    pub per_item: HashMap<i64, ItemDecision>,
}

impl BatchDecision {
    /// Пустое решение — отклоняет ВСЁ (fail-closed дефолт: ни один айтем не одобрен).
    pub fn reject_all() -> Self {
        Self::default()
    }

    /// Собрать из явных пар (action_id, решение). Не перечисленные айтемы → Reject при чтении.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (i64, ItemDecision)>) -> Self {
        Self {
            per_item: pairs.into_iter().collect(),
        }
    }

    /// Решение для конкретного `action_id`. **Отсутствующий ключ → [`ItemDecision::Reject`]** (fail-closed
    /// рубеж 2: даже если источник вернул неполную карту, не-перечисленный айтем НЕ применяется).
    pub fn decision_for(&self, action_id: i64) -> ItemDecision {
        self.per_item
            .get(&action_id)
            .copied()
            .unwrap_or(ItemDecision::Reject)
    }

    /// Одобрен ли айтем (сахар над [`decision_for`](Self::decision_for)).
    pub fn is_approved(&self, action_id: i64) -> bool {
        matches!(self.decision_for(action_id), ItemDecision::Approve)
    }
}

/// Источник решений по предложенному changeset'у — ОДИН путь, МНОГО входов (политика/тест/UI/agentd).
///
/// Object-safe (живёт за `Arc<dyn DecisionSource>` в гейте), async через `#[async_trait]`. Реализация
/// ОБЯЗАНА быть fail-closed: при сомнении — Reject (apply случится ТОЛЬКО по явному Approve).
#[async_trait]
pub trait DecisionSource: Send + Sync {
    /// Решить судьбу батча. Возвращает пер-айтемное [`BatchDecision`] (отсутствующий айтем = Reject).
    async fn decide(&self, batch: &ProposalBatch) -> BatchDecision;
}

/// Fail-closed дефолт unattended-agentd: **Reject для ВСЕХ** айтемов. Пока нет реального контрол-
/// плейна/UI, кормящего решения, headless-агент НИКОГДА не само-одобряет Confirm. Это НЕ «нет решения» —
/// это явное «отклонить всё»: confirm-run под [`PolicyDefault`] не применит НИ ОДНОГО Confirm-действия.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyDefault;

#[async_trait]
impl DecisionSource for PolicyDefault {
    async fn decide(&self, _batch: &ProposalBatch) -> BatchDecision {
        // Пустая карта ⇒ decision_for вернёт Reject для каждого айтема (fail-closed).
        BatchDecision::reject_all()
    }
}

/// Источник, принимающий решения через канал: «решатель» (тест / будущий UI / контрол-плейн agentd)
/// шлёт [`BatchDecision`] на каждый батч по очереди. ОДИН путь (`decide`), вход — внешний канал.
///
/// Реализован как очередь предзаготовленных ответов (FIFO) за `Mutex<tokio::sync::mpsc::Receiver>`:
/// `decide` берёт СЛЕДУЮЩИЙ присланный `BatchDecision`. Если канал закрыт/пуст (решатель ушёл, не
/// ответив) — **fail-closed: reject_all** (отсутствие решения НЕ применяет changeset). Для теста
/// удобно: засеять ожидаемые ответы заранее; для будущего agentd — `sender` отдаётся контрол-плейну.
pub struct ChannelDecision {
    rx: Mutex<tokio::sync::mpsc::Receiver<BatchDecision>>,
}

impl ChannelDecision {
    /// Создать источник + sender для решений. Решатель шлёт `BatchDecision` через sender; `decide`
    /// принимает их по очереди. Буфер `capacity` (≥1). Дроп sender'а ⇒ канал закрыт ⇒ `decide`
    /// fail-closed reject_all.
    pub fn new(capacity: usize) -> (Self, tokio::sync::mpsc::Sender<BatchDecision>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity.max(1));
        (Self { rx: Mutex::new(rx) }, tx)
    }
}

#[async_trait]
impl DecisionSource for ChannelDecision {
    async fn decide(&self, _batch: &ProposalBatch) -> BatchDecision {
        // Берём следующий присланный ответ. None (канал закрыт и пуст) ⇒ fail-closed reject_all:
        // решатель не дал решения → ничего не применяем.
        let mut rx = self.rx.lock().await;
        rx.recv().await.unwrap_or_else(BatchDecision::reject_all)
    }
}

/// Источник, ОДОБРЯЮЩИЙ ВСЕ айтемы батча — для **operator-initiated** действий с ЯВНЫМ согласием
/// (напр. `nexus-agentd --sandbox-undo … --approve`: оператор сам инициировал откат и подтвердил флагом).
/// НЕ для unattended-агента (тот под [`PolicyDefault`] = DENY). Confirm-классификация СОХРАНЯЕТСЯ — это НЕ
/// `Auto` (действие всё равно прошло classify→Confirm); просто решение по нему = Approve. Скомпрометированный
/// in-sandbox агент НЕ может им воспользоваться: его выдаёт ТОЛЬКО host-композиционный корень под `--approve`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproveAll;

#[async_trait]
impl DecisionSource for ApproveAll {
    async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
        BatchDecision::from_pairs(
            batch
                .items
                .iter()
                .map(|i| (i.action_id, ItemDecision::Approve)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::classify::ConfirmReason;

    fn batch() -> ProposalBatch {
        ProposalBatch {
            run_id: 1,
            items: vec![
                ProposalItem {
                    action_id: 10,
                    target_rel: "A.md".into(),
                    tier: RiskTier::Confirm(ConfirmReason::LargeOverwrite),
                    add: 2,
                    del: 1,
                },
                ProposalItem {
                    action_id: 20,
                    target_rel: "B.md".into(),
                    tier: RiskTier::Auto,
                    add: 1,
                    del: 0,
                },
            ],
        }
    }

    /// BatchDecision fail-closed: отсутствующий ключ → Reject; пустое решение отклоняет всё.
    #[test]
    fn missing_item_defaults_reject() {
        let d = BatchDecision::reject_all();
        assert_eq!(d.decision_for(10), ItemDecision::Reject);
        assert!(!d.is_approved(10));

        // Частичная карта: одобрен только 10; 20 (не перечислен) → Reject (fail-closed).
        let d = BatchDecision::from_pairs([(10, ItemDecision::Approve)]);
        assert!(d.is_approved(10));
        assert_eq!(d.decision_for(20), ItemDecision::Reject, "пропуск == отказ");
    }

    /// PolicyDefault → Reject для ВСЕХ айтемов (unattended fail-closed).
    #[tokio::test]
    async fn policy_default_rejects_all() {
        let src = PolicyDefault;
        let d = src.decide(&batch()).await;
        assert_eq!(d.decision_for(10), ItemDecision::Reject);
        assert_eq!(d.decision_for(20), ItemDecision::Reject);
    }

    /// ChannelDecision доставляет присланное решение (по очереди).
    #[tokio::test]
    async fn channel_delivers_sent_decision() {
        let (src, tx) = ChannelDecision::new(2);
        tx.send(BatchDecision::from_pairs([(10, ItemDecision::Approve)]))
            .await
            .unwrap();
        let d = src.decide(&batch()).await;
        assert!(d.is_approved(10), "одобрен присланным решением");
        assert_eq!(
            d.decision_for(20),
            ItemDecision::Reject,
            "не прислан → reject"
        );
    }

    /// ChannelDecision fail-closed: закрытый/пустой канал (решатель ушёл) → reject_all.
    #[tokio::test]
    async fn channel_closed_is_reject_all() {
        let (src, tx) = ChannelDecision::new(1);
        drop(tx); // решатель ушёл, не ответив.
        let d = src.decide(&batch()).await;
        assert_eq!(
            d.decision_for(10),
            ItemDecision::Reject,
            "нет решения ⇒ ничего не применяем"
        );
    }
}
