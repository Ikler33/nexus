//! Абстракция эмиссии событий хода актуатора ([`EventSink`]) и её реализации.
//!
//! Самодостаточная единица, вынесенная из `orchestrate.rs` (R-5b, чистый перенос без изменения
//! логики): трейт [`EventSink`] + синки [`CollectingSink`] (тест/диагностика, копит события) и
//! [`TracingEventSink`] (headless agentd — `tracing`-логирует Proposal/Diff/Exec). Публичные имена
//! реэкспортируются `orchestrate` без изменения путей (`orchestrate::EventSink` и т.д.).

use crate::event::AgentEvent;

/// Приёмник [`AgentEvent`] для гейта (эмиссия Proposal/Diff). Object-safe (`&self` + interior mutability
/// у реализаций) — гейт держит `&dyn EventSink` и шлёт события синхронно. 3e свяжет его с `on_event`
/// цикла (адаптер-обёртка над `FnMut`); тесты используют [`CollectingSink`] (копит события в `Vec`).
///
// FIXME(UI-1): связать EventSink.emit → on_event цикла / control-plane-стрим для real-time ревью
// предложений. Сегодня единственная живая реализация на проводке — [`TracingEventSink`] (headless
// agentd): предложения только ЛОГИРУЮТСЯ, не стримятся в UI; под [`PolicyDefault`] они тут же
// auto-DENY-отклоняются (нет интерактивного одобрения). UI-1 добавит человеко-в-петле поверхность
// (стрим Proposal/Diff пользователю + ответ Approve/Reject через DecisionSource).
//
// FIXME(UI-1, AGENT-6 §3): LIVE-EDITOR DIRTY-BUFFER → ФОРС-CONFIRM. Когда агент правит заметку,
// открытую в десктоп-редакторе с НЕсохранёнными изменениями (dirty-буфер), действие обязано
// ПРЕДЛОЖИТЬ (а не auto-apply) — иначе apply затрёт несохранённый буфер пользователя. Это desktop/UI-1
// скоуп: headless agentd НЕ имеет живого редактора, а решение требует состояния dirty-буфера редактора
// (его нет ни в ядре, ни в agentd). Здесь кода нет — граница зафиксирована (CHANGELOG AGENT-6).
pub trait EventSink: Send + Sync {
    /// Принять событие хода (Proposal/Diff и т.п.).
    fn emit(&self, event: AgentEvent);
}

/// Тестовый/диагностический сборщик событий — копит эмитированные [`AgentEvent`] в `Vec` за `Mutex`
/// (interior mutability: `emit(&self, …)`). Снять накопленное — [`CollectingSink::events`].
#[derive(Default)]
pub struct CollectingSink {
    events: std::sync::Mutex<Vec<AgentEvent>>,
}

impl CollectingSink {
    /// Новый пустой сборщик.
    pub fn new() -> Self {
        Self::default()
    }

    /// Снимок накопленных событий (в порядке эмиссии).
    pub fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("event mutex").clone()
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: AgentEvent) {
        self.events.lock().expect("event mutex").push(event);
    }
}

/// EventSink-мост для HEADLESS agentd (AGENT-3e §4): `tracing`-логирует Proposal/Diff. Долговечная
/// запись changeset'а — это ledger (`agent_actions`); UI-стриминг предложений в `on_event`/AgentEvent
/// поток — это UI-1 (нет UI у headless). Здесь — наблюдаемость: оператор видит в логе, ЧТО гейт
/// предложил. Под [`PolicyDefault`] предложения короткоживущи (тут же auto-DENY-отклоняются), но лог
/// предложения остаётся для аудита. Прочие события игнорируются (цикл шлёт свои через `on_event`).
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingEventSink;

impl TracingEventSink {
    /// Новый sink (бесстейтовый).
    pub fn new() -> Self {
        Self
    }
}

impl EventSink for TracingEventSink {
    fn emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::Proposal { run_id, files } => {
                tracing::info!(
                    run_id,
                    files = files.len(),
                    paths = ?files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
                    "actuator: предложение changeset'а (headless — решает DecisionSource)"
                );
            }
            AgentEvent::Diff {
                path,
                add,
                del,
                status,
            } => {
                tracing::info!(%path, add, del, ?status, "actuator: дифф предложенного файла");
            }
            // Exec-наблюдаемость (6c-2g): оператор headless видит exec-намерение/исход в логе. summary —
            // редакция-безопасный силуэт (без сырых argv/значений); ExecResult — exit+finalized без вывода.
            AgentEvent::ExecProposal {
                run_id,
                action_id,
                summary,
            } => {
                tracing::info!(
                    run_id,
                    action_id,
                    %summary,
                    "actuator: exec-предложение (headless — решает DecisionSource)"
                );
            }
            AgentEvent::ExecResult {
                run_id,
                action_id,
                exit_code,
                finalized,
            } => {
                tracing::info!(
                    run_id,
                    action_id,
                    exit_code,
                    finalized,
                    "actuator: exec завершён"
                );
            }
            // Прочие события цикла идут через on_event — здесь не наша забота.
            _ => {}
        }
    }
}
