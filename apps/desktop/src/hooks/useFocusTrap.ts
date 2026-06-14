import { type RefObject, useEffect, useRef } from 'react';

const FOCUSABLE =
  'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';

/**
 * Focus-trap для модального оверлея (P9, a11y). Возвращает ref для корневого `role="dialog"`-элемента
 * (повесить `tabIndex={-1}`). При монтировании переводит фокус внутрь (первый фокусируемый/контейнер);
 * Tab/Shift+Tab циклят по фокусируемым ВНУТРИ (не утекают за пределы); Esc вызывает `onClose`
 * (+stopPropagation — не доходит до глобального Esc, напр. reading-mode). Восстановление фокуса на
 * триггер НЕ делаем намеренно: эти панели по клику навигируют в редактор, и restore воровал бы у него
 * фокус (см. TasksPanel/InboxPanel/GoalsPanel openTaskLocation). Видимость не фильтруем (jsdom-совместимо;
 * у панелей нет скрытых фокусируемых).
 */
export function useFocusTrap<T extends HTMLElement>(onClose: () => void): RefObject<T | null> {
  const ref = useRef<T>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const container = ref.current;
    if (!container) return;
    const items = () => Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE));

    (items()[0] ?? container).focus();

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (e.key !== 'Tab') return;
      const list = items();
      if (list.length === 0) {
        e.preventDefault();
        return;
      }
      const first = list[0];
      const last = list[list.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };

    container.addEventListener('keydown', onKeyDown);
    return () => container.removeEventListener('keydown', onKeyDown);
  }, []);

  return ref;
}
