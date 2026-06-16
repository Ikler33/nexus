// Чистая модель доски (BOARD-4): группировка карточек по колонкам + утилиты карточек. Без React/IO —
// юнит-тестируемо. Колонкование делает фронт (бэкенд `list_board` отдаёт плоский список, §5 спеки).

import type { StaleTask, TaskCard } from '../../lib/tauri-api';

/** Дефолтные колонки доски (BOARD-4; конфигурируемые доски/переименование — BOARD-3). id = значение `status`. */
export const DEFAULT_COLUMN_IDS = ['todo', 'doing', 'done'] as const;

/** Виртуальная колонка для статусов вне набора — чтобы НЕ терять задачи (§12). */
export const OTHER_COLUMN_ID = '__other__';

/** Колонка доски: id (для дефолтных — сам статус; иначе `OTHER_COLUMN_ID`) + её карточки. */
export interface BoardColumn {
  id: string;
  cards: TaskCard[];
}

/** Нормализация статуса для матчинга колонок: trim + lowercase (§1 — сравнение колонок case-insensitive). */
export function normalizeStatus(status: string): string {
  return status.trim().toLowerCase();
}

/**
 * AI-2a: из «застрявших» задач (бэкенд отдаёт ВСЕ старше порога) убираем терминальные — «застряло» только
 * то, что ещё в работе. Done-like-статусы берём из конфига доски (id done-like-колонки = raw-значение
 * `status`), сверяем по `normalizeStatus`. Чистая фильтрация — без мутации входа.
 */
export function filterStuck(
  stale: StaleTask[],
  columns: { id: string; doneLike: boolean }[],
): StaleTask[] {
  const doneLike = new Set(
    columns.filter((c) => c.doneLike).map((c) => normalizeStatus(c.id)),
  );
  return stale.filter((s) => !doneLike.has(normalizeStatus(s.status)));
}

/**
 * Группирует карточки в колонки: каждая из `columnIds` в порядке + виртуальная «Прочее» В КОНЦЕ для
 * статусов вне набора (включается только если непуста). Колонки набора сохраняются даже пустыми (видна
 * структура доски). Порядок карточек внутри — как пришли (бэкенд сортирует по пути; ручной — BOARD-3).
 */
export function groupIntoColumns(cards: TaskCard[], columnIds: readonly string[]): BoardColumn[] {
  const cols: BoardColumn[] = columnIds.map((id) => ({ id, cards: [] }));
  const byId = new Map(cols.map((c) => [c.id.toLowerCase(), c]));
  const other: TaskCard[] = [];
  for (const card of cards) {
    const col = byId.get(normalizeStatus(card.status));
    if (col) col.cards.push(card);
    else other.push(card);
  }
  if (other.length) cols.push({ id: OTHER_COLUMN_ID, cards: other });
  return cols;
}

/**
 * Применяет ручной порядок колонки (BOARD-3): карточки из `orderPaths` идут первыми в их порядке;
 * остальные (новые, ещё не в order) — после, стабильно по пути. Чистая сортировка копии (не мутирует).
 */
export function applyOrder(cards: TaskCard[], orderPaths: string[] | undefined): TaskCard[] {
  if (!orderPaths || orderPaths.length === 0) return cards;
  const idx = new Map(orderPaths.map((p, i) => [p, i]));
  return [...cards].sort((a, b) => {
    const ia = idx.get(a.path) ?? Number.POSITIVE_INFINITY;
    const ib = idx.get(b.path) ?? Number.POSITIVE_INFINITY;
    if (ia !== ib) return ia - ib;
    return a.path.localeCompare(b.path); // стабильный тай-брейк для не-в-order
  });
}

/** Дедлайн просрочен? Сравнение ISO-дат `YYYY-MM-DD` (лексикографически верно); сегодня — НЕ просрочено;
 *  невалидная дата → false (бейдж не рисуем как overdue). */
export function isOverdue(due: string | null, todayIso: string): boolean {
  if (!due) return false;
  const d = due.trim();
  if (!/^\d{4}-\d{2}-\d{2}$/.test(d)) return false;
  return d < todayIso;
}

/** Убирает ведущий frontmatter-блок (`---\n…\n---`) — для превью ТЕЛА заметки (BOARD-6). Незакрытый блок
 *  или его отсутствие → контент как есть. Ведущие пустые строки тела срезаются. */
export function stripFrontmatter(content: string): string {
  if (!content.startsWith('---\n') && !content.startsWith('---\r\n')) return content;
  const open = content.indexOf('\n') + 1;
  const lines = content.slice(open).split('\n');
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].replace(/\r$/, '') === '---') {
      return lines
        .slice(i + 1)
        .join('\n')
        .replace(/^\s*\n/, '');
    }
  }
  return content; // незакрытый блок — не угадываем, показываем как есть
}

/** Имя файла без пути и расширения `.md` — фолбэк-заголовок карточки. */
export function basename(path: string): string {
  const file = path.slice(path.lastIndexOf('/') + 1);
  return file.replace(/\.md$/i, '');
}

/** Локальная ISO-дата `YYYY-MM-DD` (для сравнения дедлайнов в зоне пользователя). */
export function todayIsoLocal(d = new Date()): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

/** Известные приоритеты (для i18n-метки и цвета); прочее значение показываем raw нейтральным бейджем. */
export const KNOWN_PRIORITIES = ['low', 'medium', 'high', 'urgent'] as const;

/** Нормализованный приоритет из набора или `null` (нестандартное значение — raw, нейтральный стиль). */
export function knownPriority(priority: string | null): (typeof KNOWN_PRIORITIES)[number] | null {
  const p = priority?.trim().toLowerCase();
  return (KNOWN_PRIORITIES as readonly string[]).includes(p ?? '')
    ? (p as (typeof KNOWN_PRIORITIES)[number])
    : null;
}

/** Корзина плана дня: причина попадания задачи в фокус (для бейджа + группировки). */
export type PlanBucket = 'overdue' | 'today' | 'priority';

/** Элемент плана дня (AI-2b): карточка + причина отбора. */
export interface PlanItem {
  card: TaskCard;
  bucket: PlanBucket;
}

/** Ранг приоритета для сортировки (меньше = важнее); нестандартный/нет — в конец. */
function priorityRank(priority: string | null): number {
  switch (knownPriority(priority)) {
    case 'urgent':
      return 0;
    case 'high':
      return 1;
    case 'medium':
      return 2;
    case 'low':
      return 3;
    default:
      return 4;
  }
}

/**
 * AI-2b (A3, спека §10): детерминированный «план на день» — отбор и раскладка активных задач в фокус.
 * Чистая функция (без LLM/IO): из карточек берём НЕ-терминальные (done-like убраны по конфигу) и
 * относим в одну из корзин по приоритету причины: `overdue` (дедлайн в прошлом) → `today` (дедлайн
 * сегодня) → `priority` (urgent/high без срочного дедлайна). Внутри `overdue`/`today` — по дате (раньше
 * выше), затем по приоритету; в `priority` — по приоритету; всюду тай-брейк по пути. Задачи без причины
 * (нет дедлайна и не высокий приоритет) в план НЕ попадают — план сфокусирован. Обрезка до `limit`.
 */
export function planDay(
  cards: TaskCard[],
  columns: { id: string; doneLike: boolean }[],
  todayIso: string,
  limit = 7,
): PlanItem[] {
  const doneLike = new Set(columns.filter((c) => c.doneLike).map((c) => normalizeStatus(c.id)));
  const items: PlanItem[] = [];
  for (const card of cards) {
    if (doneLike.has(normalizeStatus(card.status))) continue; // терминальная — не в план
    let bucket: PlanBucket | null = null;
    if (card.due && isOverdue(card.due, todayIso)) bucket = 'overdue';
    else if (card.due && card.due === todayIso) bucket = 'today';
    else {
      const p = knownPriority(card.priority);
      if (p === 'urgent' || p === 'high') bucket = 'priority';
    }
    if (bucket) items.push({ card, bucket });
  }
  const bucketRank: Record<PlanBucket, number> = { overdue: 0, today: 1, priority: 2 };
  items.sort((a, b) => {
    const br = bucketRank[a.bucket] - bucketRank[b.bucket];
    if (br) return br;
    // overdue/today: сначала по дате (раньше = важнее); priority: дата не различает.
    if (a.bucket !== 'priority') {
      const d = (a.card.due ?? '').localeCompare(b.card.due ?? '');
      if (d) return d;
    }
    const pr = priorityRank(a.card.priority) - priorityRank(b.card.priority);
    if (pr) return pr;
    return a.card.path.localeCompare(b.card.path);
  });
  return items.slice(0, Math.max(0, limit));
}
