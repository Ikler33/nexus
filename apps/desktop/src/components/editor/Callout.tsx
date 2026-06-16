import {
  Bug,
  Check,
  ChevronDown,
  ChevronRight,
  CircleCheck,
  CircleHelp,
  CircleX,
  ClipboardList,
  Flame,
  Info,
  List,
  Pencil,
  Quote,
  TriangleAlert,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import { Children, useState, type ReactNode } from 'react';

import styles from './MarkdownPreview.module.css';

/**
 * Obsidian-callout (admonition) в режиме чтения. Тип, иконка и цвет — по каноническому виду; алиасы
 * (`hint`→tip, `error`→danger, …) сводятся к канону. CSP-безопасно: иконка — инлайновый SVG (lucide),
 * цвет/тинт — классами + `data-callout`-селектором (без inline-style). Сворачивание (`+`/`-`) — на
 * React-state, тело прячется классом. Заголовок и тело приходят как `children` (первый непустой ребёнок
 * — это `nexus-callout-title`, остальное — тело), потому что react-markdown уже отрендерил их.
 */

// Алиасы Obsidian → канонический вид (для иконки и цвета). Неизвестный тип → 'note' (нейтральный).
const ALIASES: Record<string, string> = {
  summary: 'abstract',
  tldr: 'abstract',
  hint: 'tip',
  important: 'tip',
  check: 'success',
  done: 'success',
  help: 'question',
  faq: 'question',
  caution: 'warning',
  attention: 'warning',
  fail: 'failure',
  missing: 'failure',
  error: 'danger',
  cite: 'quote',
};

const ICONS: Record<string, LucideIcon> = {
  note: Pencil,
  abstract: ClipboardList,
  info: Info,
  todo: CircleCheck,
  tip: Flame,
  success: Check,
  question: CircleHelp,
  warning: TriangleAlert,
  failure: CircleX,
  danger: Zap,
  bug: Bug,
  example: List,
  quote: Quote,
};

/** Канонический вид: алиас → канон; неизвестный → 'note'. */
function canonicalCalloutKind(kind: string): string {
  const lower = kind.toLowerCase();
  const canon = ALIASES[lower] ?? lower;
  return canon in ICONS ? canon : 'note';
}

/** Дефолтная подпись, когда заголовок не задан: тип с заглавной (Obsidian показывает ключевое слово). */
function defaultLabel(rawLabel: string): string {
  if (!rawLabel) return 'Note';
  return rawLabel.charAt(0).toUpperCase() + rawLabel.slice(1);
}

export function CalloutTitle({
  kind,
  label,
  children,
}: {
  kind: string;
  label: string;
  children?: ReactNode;
}) {
  const canon = canonicalCalloutKind(kind);
  const Icon = ICONS[canon] ?? Pencil;
  // children пуст (заголовок не задан) → дефолтная подпись по ключевому слову.
  const hasTitle = Children.toArray(children).some((c) => !(typeof c === 'string' && c.trim() === ''));
  return (
    <>
      <span className={styles.calloutIcon} aria-hidden="true">
        <Icon size={18} />
      </span>
      <span className={styles.calloutTitleText}>{hasTitle ? children : defaultLabel(label)}</span>
    </>
  );
}

export function Callout({
  kind,
  fold,
  children,
}: {
  kind: string;
  fold: string;
  children?: ReactNode;
}) {
  const canon = canonicalCalloutKind(kind);
  const foldable = fold === '+' || fold === '-';
  const [open, setOpen] = useState(fold !== '-'); // '-' свёрнут по умолчанию

  // children = [перевод-строки, <CalloutTitle/>, перевод-строки, ...тело]. Отбрасываем whitespace-строки;
  // первый реальный ребёнок — заголовок, остальное — тело.
  const kids = Children.toArray(children).filter((c) => !(typeof c === 'string' && c.trim() === ''));
  const header = kids[0];
  const body = kids.slice(1);

  const toggle = () => foldable && setOpen((o) => !o);

  return (
    <div className={styles.callout} data-callout={canon}>
      <div
        className={styles.calloutHeader}
        role={foldable ? 'button' : undefined}
        tabIndex={foldable ? 0 : undefined}
        aria-expanded={foldable ? open : undefined}
        onClick={toggle}
        onKeyDown={
          foldable
            ? (e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  toggle();
                }
              }
            : undefined
        }
      >
        {header}
        {foldable && (
          <span className={styles.calloutFold} aria-hidden="true">
            {open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
          </span>
        )}
      </div>
      {body.length > 0 && open && <div className={styles.calloutBody}>{body}</div>}
    </div>
  );
}
