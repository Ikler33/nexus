//! `Redacted<T>` (AC-SEC-6, ревью H18): обёртка, чьи `Debug`/`Display` НЕ печатают значение —
//! чтобы контент заметок, пути и URL не утекали в логи/трейсы/`{:?}`/crash-отчёты по неосторожности.
//! Значение достаётся ТОЛЬКО явно через [`Redacted::expose`] — имя кричит на ревью «здесь раскрываем».
//!
//! Конвенция: всё, что может содержать пользовательский контент/секрет и потенциально попадёт в лог
//! (поле структуры с `#[derive(Debug)]`, аргумент `tracing::*`, payload ошибки), оборачивать в
//! `Redacted`. Сейчас ядро НЕ логирует контент заметок (проверено), так что это страховка от регрессий
//! и инструмент для будущих фич (web/импорт/память агента), где контент пойдёт через логируемые пути.

use std::fmt;

/// Обёртка над чувствительным значением: `Debug`/`Display` печатают `<redacted>`, не значение.
/// Доступ к значению — только явный, через [`expose`](Self::expose) / [`into_inner`](Self::into_inner).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Оборачивает значение как редактируемое (скрытое в Debug/Display).
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Явный доступ к скрытому значению. Имя намеренно «громкое» — каждое раскрытие видно на ревью.
    pub fn expose(&self) -> &T {
        &self.0
    }

    /// Забирает значение, потребляя обёртку.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> From<T> for Redacted<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_reveal_value() {
        let secret = Redacted::new("содержимое заметки с секретом");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert!(!format!("{secret:?}").contains("секрет"));
    }

    #[test]
    fn value_is_hidden_in_interpolation_but_reachable_via_expose() {
        let path = Redacted::new("/Users/login/vault/private note.md");
        // Как это попало бы в tracing-аргумент или {:?}-лог — путь не виден.
        let line = format!("indexing path={path:?} done");
        assert!(!line.contains("vault"));
        assert!(!line.contains("login"));
        assert!(line.contains("<redacted>"));
        // expose возвращает оригинал явно.
        assert_eq!(*path.expose(), "/Users/login/vault/private note.md");
    }

    #[test]
    fn into_inner_returns_value() {
        assert_eq!(Redacted::new(42u32).into_inner(), 42);
        // From<T> тоже доступен.
        let r: Redacted<&str> = "x".into();
        assert_eq!(r.into_inner(), "x");
    }
}
