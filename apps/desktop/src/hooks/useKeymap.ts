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
      if (!(e.ctrlKey || e.metaKey || e.altKey)) return;
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
