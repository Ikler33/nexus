import { useContext, type ReactNode } from 'react';

import { SectionContext } from '../../lib/markdown/section-context';
import styles from './MarkdownPreview.module.css';

/**
 * Обёртка H2-секции (Hermes-8 S3 «Редакция»). `rehypeSections` сгруппировал h2 + тело в
 * `<section class="sec" data-sec-id>` (h2 первым ребёнком, тело в `.sec-body`). Здесь — только класс
 * `.collapsed` по состоянию сворачивания (из контекста), чтобы CSS-анимация max-height/opacity спрятала
 * `.sec-body`. Интерактив (клик/шеврон/a11y) висит на самом h2 (`components.h2`), т.к. react-markdown не
 * форвардит обработчики во вложенный заголовок через clone — оба читают общий `SectionContext` по secId.
 *
 * Тело НЕ размонтируется при сворачивании (прячется классом) — чтобы scroll-spy/оглавление могли раскрыть
 * секцию и доскроллить к содержимому, а не потерять его.
 */
export function Section({ secId, children }: { secId: string; children?: ReactNode }) {
  const { isCollapsed } = useContext(SectionContext);
  const collapsed = isCollapsed(secId);
  return (
    <section
      className={collapsed ? `${styles.sec} ${styles.collapsed}` : styles.sec}
      data-sec-id={secId}
    >
      {children}
    </section>
  );
}
