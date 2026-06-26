import { ChevronDown } from 'lucide-react';
import { useContext, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { SectionContext } from '../../lib/markdown/section-context';
import styles from './MarkdownPreview.module.css';

/**
 * Заголовок сворачиваемой H2-секции (Hermes-8 S3 «Редакция»). Клик по всей строке h2 тогглит секцию
 * (мышиная зона), а доступная с клавиатуры/скринридера кнопка — это вложенный шеврон `.fold`
 * (`<button aria-expanded>`). Так h2 ОСТАЁТСЯ заголовком (role=heading): Outline/scroll-spy/`#heading`-
 * якоря/getByRole('heading') не ломаются (ИНВАРИАНТ HEADANCHOR-1 + a11y), а сворачивание остаётся
 * доступным. Двойного тоггла нет: клик по кнопке-шеврону `stopPropagation` гасит всплытие на h2.
 *
 * `id`(slug) и `data-outline-line` приходят пропсами от `components.h2` и ставятся на host-`<h2>` как есть.
 */
export function SectionHeading({
  secId,
  id,
  outlineLine,
  children,
}: {
  secId: string;
  id: string;
  outlineLine?: number;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const { isCollapsed, toggle } = useContext(SectionContext);
  const collapsed = isCollapsed(secId);
  return (
    <h2 id={id} data-outline-line={outlineLine} onClick={() => toggle(secId)}>
      <button
        type="button"
        className={styles.fold}
        aria-expanded={!collapsed}
        aria-label={collapsed ? t('editor.expandSection') : t('editor.collapseSection')}
        onClick={(e) => {
          // Кнопка-шеврон сама тогглит (клавиатура/фокус); гасим всплытие, иначе клик по ней сработал бы
          // ещё и через onClick самого h2 (двойной тоггл → секция «не реагирует»).
          e.stopPropagation();
          toggle(secId);
        }}
      >
        <ChevronDown size={13} aria-hidden />
      </button>
      {children}
    </h2>
  );
}
