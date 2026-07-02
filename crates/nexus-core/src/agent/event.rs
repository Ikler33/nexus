//! Реэкспорт-шим (R-1, развязка слоёв): контракт событий хода агента живёт в НЕЙТРАЛЬНОМ
//! [`crate::event`] — его используют и агент (эмиттер цикла), и актуатор (`Proposal`/`Diff` в
//! `crate::actuator::orchestrate`) БЕЗ ребра actuator→agent. Старый путь `crate::agent::event::*`
//! сохранён этим реэкспортом (потребители — desktop/agentd/cli/sandbox — не правятся).

pub use crate::event::*;
