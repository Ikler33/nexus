//! Мелкие переиспользуемые утилиты ядра (R-13g).
//!
//! Канон для примитивов, которые исторически копировались байт-в-байт по модулям. Держим их здесь
//! одним источником истины, чтобы правки семантики (например, символ-эллипсис) не расходились.

/// Обрезает строку по СИМВОЛАМ (UTF-8-безопасно, не по байтам) с «…», если длиннее `max`.
///
/// Канон (R-13g): раньше три байт-идентичные копии жили в `ai::chat`, `episode`, `skills`.
/// ⚠️ НЕ путать с `agent::research::worker::truncate_chars` — та БЕЗ эллипсиса (иная семантика,
/// намеренно).
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Короткая строка не трогается; длинная обрезается по символам и получает «…» (UTF-8-safe).
    #[test]
    fn truncate_chars_caps_long_utf8() {
        assert_eq!(truncate_chars("коротко", 100), "коротко");
        let long = "я".repeat(330);
        let t = truncate_chars(&long, 280);
        assert_eq!(t.chars().count(), 281, "max символов + «…»");
        assert!(t.ends_with('…'));
        // Обрезка по СИМВОЛАМ, не по байтам: 280 кириллических символов остались целыми.
        assert!(t.starts_with(&"я".repeat(280)));
    }
}
