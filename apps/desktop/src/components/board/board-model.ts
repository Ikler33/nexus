// Чистая модель доски (BOARD-4): группировка карточек по колонкам + утилиты карточек. Без React/IO —
// юнит-тестируемо. Колонкование делает фронт (бэкенд `list_board` отдаёт плоский список, §5 спеки).

import type { TaskCard } from '../../lib/tauri-api';

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
