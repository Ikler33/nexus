import { useEffect, useLayoutEffect, useRef, useState } from 'react';

import styles from './Popcard.module.css';

/** Контент поповера. Только заполненные слоты рендерятся (пустые опускаются) — анти-фейк (S7):
 *  ничего не выдумываем, лучше показать меньше. `meta` — ТОЛЬКО реальные данные (статус из
 *  frontmatter / счётчик беклинков из `graph.getBacklinks`), иначе undefined → слот не рисуется. */
export interface PopcardContent {
  /** `--font-mono` ember-надстрочник: тип заметки (uppercase) или «Сноска». Опц. */
  eyebrow?: string;
  /** Cormorant-заголовок (имя/заголовок заметки). Для сноски опускается. Опц. */
  title?: string;
  /** Тело: эксцерпт заметки / текст сноски / честное «не найдено». */
  body: string;
  /** `--font-mono` faint-мета (статус · N ссылок). ТОЛЬКО реальные данные. Опц. */
  meta?: string;
  /** Тело приглушено (muted) — честное состояние «заметка не найдена». */
  muted?: boolean;
}

const CARD_WIDTH = 300;
const FNOTE_WIDTH = 280;
const GAP = 8;
const MARGIN = 8;

/** Считает позицию карточки от rect триггера: под элементом (`bottom+GAP`), с клампом по правому/левому
 *  краю и флипом вверх, если по высоте не влезает. `cardHeight` — реальная высота (замер после монтирования)
 *  или 0 на первом кадре (тогда флип не сработает, скорректируется в useLayoutEffect). */
function place(
  rect: { top: number; bottom: number; left: number },
  cardWidth: number,
  cardHeight: number,
  vw: number,
  vh: number,
): { top: number; left: number } {
  const left = Math.max(MARGIN, Math.min(rect.left, vw - cardWidth - MARGIN));
  let top = rect.bottom + GAP;
  // Флип вверх, если низ карточки выходит за вьюпорт (и сверху места хватает).
  if (cardHeight > 0 && top + cardHeight > vh && rect.top - cardHeight - GAP >= MARGIN) {
    top = rect.top - cardHeight - GAP;
  }
  // Финальный кламп по высоте: если флип невозможен (карточка выше доступного места и сверху, и снизу),
  // прижимаем к вьюпорту, чтобы низ не уезжал за экран (ревью: длинная карточка переполняла низ).
  if (cardHeight > 0) {
    top = Math.max(MARGIN, Math.min(top, vh - cardHeight - MARGIN));
  }
  return { top, left };
}

/**
 * Hermes-8 S7: ховер-превью `.popcard` (preview-only, НЕ интерактивный — `pointer-events:none`, чтобы
 * не ловить ховер-трапы и не перехватывать клик по ссылке). Вики — `width:300`, сноска (`fnote`) — `280`.
 * Появление: примонтировали → следующий кадр класс `.show` (opacity 0→1, translateY 4px→0, .15s);
 * `prefers-reduced-motion` обнуляет анимацию (CSS). Позиционирование `fixed` от rect триггера.
 */
export function Popcard({
  rect,
  content,
  variant,
}: {
  /** Прямоугольник триггера (`getBoundingClientRect`) — точка привязки. */
  rect: { top: number; bottom: number; left: number };
  content: PopcardContent;
  variant: 'wiki' | 'fnote';
}) {
  const cardWidth = variant === 'fnote' ? FNOTE_WIDTH : CARD_WIDTH;
  const cardRef = useRef<HTMLDivElement>(null);
  const [show, setShow] = useState(false);
  const [pos, setPos] = useState(() =>
    place(
      rect,
      cardWidth,
      0,
      typeof window !== 'undefined' ? window.innerWidth : 1280,
      typeof window !== 'undefined' ? window.innerHeight : 800,
    ),
  );

  // После монтирования замеряем реальную высоту → пере-позиционируем (флип вверх, если нужно).
  useLayoutEffect(() => {
    const h = cardRef.current?.offsetHeight ?? 0;
    setPos(place(rect, cardWidth, h, window.innerWidth, window.innerHeight));
    // rect — стабильный объект из контроллера на время жизни карточки; пересоздаётся при новом ховере
    // (новый mount). content в deps на случай асинхронной дозагрузки тела (высота меняется).
  }, [rect, cardWidth, content]);

  // Следующий кадр после монтирования → класс `.show` (запуск transition). rAF, чтобы стиль 0→1
  // действительно проигрался (синхронная установка склеила бы кадры → без анимации).
  useEffect(() => {
    const id = requestAnimationFrame(() => setShow(true));
    return () => cancelAnimationFrame(id);
  }, []);

  const cls = [styles.popcard, variant === 'fnote' ? styles.fnote : '', show ? styles.show : '']
    .filter(Boolean)
    .join(' ');

  return (
    <div
      ref={cardRef}
      className={cls}
      role="tooltip"
      style={{ top: pos.top, left: pos.left }}
      data-popcard={variant}
    >
      {content.eyebrow && <div className={styles.eyebrow}>{content.eyebrow}</div>}
      {content.title && <div className={styles.title}>{content.title}</div>}
      <div className={content.muted ? `${styles.body} ${styles.muted}` : styles.body}>{content.body}</div>
      {content.meta && <div className={styles.meta}>{content.meta}</div>}
    </div>
  );
}
