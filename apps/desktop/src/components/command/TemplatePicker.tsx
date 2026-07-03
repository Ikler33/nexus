import { useEffect, useRef, useState } from 'react';
import { FileText, LayoutTemplate } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { listTemplates, newNoteFromTemplate, templateTitle } from '../../lib/templates';
import { useUIStore } from '../../stores/ui';
import { useToastStore } from '../../stores/toast';
import styles from './TemplatePicker.module.css';

/**
 * Выбор шаблона (CAP-3, ⌘⇧T): список заметок из Templates/ → создать новую с подстановкой
 * плейсхолдеров и открыть. Стрелки/Enter/клик; Esc — отмена. Пустая папка — подсказка-онбординг.
 */
export function TemplatePicker() {
  const open = useUIStore((s) => s.templatesOpen);
  const close = useUIStore((s) => s.closeTemplates);
  const { t } = useTranslation();
  const [items, setItems] = useState<string[] | null>(null); // null = ещё грузим
  const [active, setActive] = useState(0);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    setItems(null);
    setActive(0);
    boxRef.current?.focus(); // фокус на модалку → ловит стрелки/Enter/Esc
    let cancelled = false;
    void listTemplates().then((list) => {
      if (!cancelled) setItems(list);
    });
    return () => {
      cancelled = true;
    };
  }, [open]);

  if (!open) return null;

  const pick = (templatePath: string) => {
    close();
    void newNoteFromTemplate(templatePath)
      .then(() => {
        useToastStore.getState().addToast(
          t('templates.created', { name: templateTitle(templatePath) }),
          { kind: 'success' },
        );
      })
      .catch(() => {
        // Ошибка создания видима (мандат 3): шаблон удалён / сбой записи — не молчим.
        useToastStore.getState().addToast(t('templates.error'), { kind: 'error' });
      });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!items || items.length === 0) {
      if (e.key === 'Escape') {
        // preventDefault — как в основной ветке: сигнал «обработано» для reading-Esc-гейта App.tsx.
        e.preventDefault();
        close();
      }
      return;
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActive((a) => Math.min(items.length - 1, a + 1));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setActive((a) => Math.max(0, a - 1));
        break;
      case 'Enter':
        e.preventDefault();
        pick(items[active]);
        break;
      case 'Escape':
        e.preventDefault();
        close();
        break;
    }
  };

  return (
    <div className={styles.overlay} onClick={close} role="presentation">
      <div
        ref={boxRef}
        className={styles.box}
        role="dialog"
        aria-modal="true"
        aria-label={t('templates.title')}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
        tabIndex={-1}
      >
        <div className={styles.head}>
          <LayoutTemplate size={15} aria-hidden className={styles.headIco} />
          <span className={styles.title}>{t('templates.title')}</span>
          <kbd className={styles.kbd}>Esc</kbd>
        </div>
        {items === null ? (
          <div className={styles.empty}>{t('templates.loading')}</div>
        ) : items.length === 0 ? (
          <div className={styles.empty}>{t('templates.empty')}</div>
        ) : (
          <ul className={styles.list} role="listbox" aria-label={t('templates.title')}>
            {items.map((path, i) => (
              <li
                key={path}
                role="option"
                aria-selected={i === active}
                data-active={i === active || undefined}
                className={styles.item}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(path)}
              >
                <FileText size={15} className={styles.itemIco} aria-hidden />
                <span className={styles.itemName}>{templateTitle(path)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
