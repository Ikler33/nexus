import { useDeferredValue, useMemo, useState } from 'react';
import { ChevronRight, List } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { extractHeadings } from '../../lib/editor/outline';
import styles from './OutlineBar.module.css';

/**
 * EDIT-7: оглавление активной заметки — ATX-заголовки (`#`..`######`, вне код-блоков) списком с
 * отступом по уровню; клик ведёт к секции (`onJump` с 1-based номером исходной строки). Сворачивается
 * шапкой-твистом (как BacklinksBar). Нет заголовков → бар скрыт (не шумит на коротких заметках).
 * Уровни нормализованы к минимальному (заметка из h2/h3 не уезжает вправо). Стоит над backlinks-баром.
 *
 * Hermes-8 S6 (scroll-spy): `activeLine` — исходная строка активного (видимого вверху вьюпорта)
 * заголовка, считается в GroupPane при скролле документа. Пункт, чей `h.line === activeLine`, получает
 * `.active` (ember-маркер по README §6) + `aria-current="location"`. Не задан (нет родителя со скроллом —
 * вложенные контексты) → подсветки нет, поведение прежнее.
 */
export function OutlineBar({
  doc,
  onJump,
  activeLine,
}: {
  doc: string;
  onJump: (line: number) => void;
  activeLine?: number | null;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  // Парсинг заголовков откладываем (audit B11): при быстром наборе React не пересчитывает оглавление
  // на каждый символ — useDeferredValue паузит до затишья ввода. Поведение то же, лишь ~кадр задержки.
  const deferredDoc = useDeferredValue(doc);
  const headings = useMemo(() => extractHeadings(deferredDoc), [deferredDoc]);
  if (headings.length === 0) return null;

  const minLevel = headings.reduce((m, h) => Math.min(m, h.level), 6);

  return (
    <nav className={styles.bar} aria-label={t('outline.title')}>
      <button
        type="button"
        className={styles.header}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <ChevronRight size={13} className={styles.twist} data-open={open || undefined} aria-hidden />
        <List size={13} aria-hidden />
        <span>{t('outline.count', { count: headings.length })}</span>
      </button>
      {open && (
        <ul className={styles.list}>
          {headings.map((h, i) => {
            const isActive = activeLine != null && h.line === activeLine;
            return (
              <li key={`${h.line}:${i}`}>
                <button
                  type="button"
                  className={isActive ? `${styles.item} ${styles.active}` : styles.item}
                  aria-current={isActive ? 'location' : undefined}
                  style={{ paddingLeft: `calc(var(--space-2) + ${(h.level - minLevel) * 12}px)` }}
                  onClick={() => onJump(h.line)}
                  title={h.text}
                >
                  {h.text}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </nav>
  );
}
