import { useEffect } from 'react';
import { commands, eventToCombo } from '../lib/commands';

/**
 * Глобальный обработчик хоткеев: combo события → команда через реестр (приоритет
 * пользователь>плагин>ядро). Срабатывает только на комбинации с модификатором, чтобы не
 * перехватывать обычный ввод текста. Хоткеи без модификатора (Esc/стрелки) — у компонентов.
 */
export function useKeymap(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // Уже обработано ближе к фокусу (CM6-редактор, инпут панели и т.п. сделали preventDefault) —
      // глобальную команду НЕ дублируем. Иначе, напр., ⌘G «найти дальше» в редакторе (searchKeymap не
      // ставит stopPropagation) ещё и тоглил бы граф (`view.graph` = mod+g). Находка ревью.
      if (e.defaultPrevented) return;
      // Команды требуют модификатор (иначе перехватывали бы обычный ввод текста). Исключение —
      // функциональные клавиши F1–F12: они никогда не текст, поэтому безопасно диспетчеризуются
      // голыми (F2 = переименование, OS-стандарт). Незарегистрированные F-клавиши (F5-reload и т.п.)
      // проходят насквозь: preventDefault ниже срабатывает только при совпавшей команде.
      const isFnKey = /^F\d{1,2}$/.test(e.key);
      if (!isFnKey && !(e.ctrlKey || e.metaKey || e.altKey)) return;
      // Голую F-клавишу НЕ крадём из формового поля (window-листенер ловит её даже при stopPropagation
      // инпута — нативное всплытие). Иначе F2 в самом rename-input/поиске пере-сидил бы введённый текст.
      // contentEditable (CM6-редактор) пропускаем намеренно: F2 должна переименовывать открытый файл.
      const tgt = e.target as HTMLElement | null;
      const inFormField =
        !!tgt && (tgt.tagName === 'INPUT' || tgt.tagName === 'TEXTAREA' || tgt.tagName === 'SELECT');
      if (isFnKey && inFormField) return;
      const id = commands.resolve(eventToCombo(e));
      if (id) {
        e.preventDefault();
        void commands.run(id);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
}
