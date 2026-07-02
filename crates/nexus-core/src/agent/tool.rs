//! Реэкспорт-шим (R-1, развязка слоёв): типы границы инструментов ([`Tool`]/[`ToolSpec`]/[`ToolCall`]/
//! [`ToolError`]) живут в НЕЙТРАЛЬНОМ [`crate::tool_types`] — их используют агент, актуатор
//! (`crate::actuator::tools`) и AI-слой (`crate::ai::tools`) БЕЗ рёбер actuator→agent / ai→agent.
//! Старый путь `crate::agent::tool::*` сохранён этим реэкспортом (потребители не правятся).

pub use crate::tool_types::*;
