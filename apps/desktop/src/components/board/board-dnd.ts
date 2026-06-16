// Чистая модель drag-n-drop доски (BOARD-5): вычисление нового порядка + смены статуса при перетаскивании
// карточки. Без React/IO — юнит-тестируемо. Состояние-машина (optimistic/rollback) — в BoardView.

import { OTHER_COLUMN_ID } from './board-model';

/** Данные перетаскиваемой карточки (кладутся в dataTransfer). */
export interface DragData {
  path: string;
  fromCol: string;
}

/** План перемещения: какие колонки порядка переписать + нужна ли смена статуса. */
export interface MovePlan {
  /** colId → новый полный порядок путей (только затронутые колонки). Мержится в `config.order`. */
  order: Record<string, string[]>;
  /** Смена статуса при кросс-колоночном переносе (`null` — реордер внутри колонки). */
  statusChange: { path: string; status: string } | null;
}

/**
 * Планирует перенос карточки `drag.path` из `drag.fromCol` в `toCol` на позицию `toIndex`.
 * `displayed` — текущая раскладка (colId → пути в порядке показа). Возвращает `null`, если перенос
 * НЕДОПУСТИМ (в виртуальную «Прочее» — у неё нет статуса-колонки; либо no-op на то же место).
 *
 * Затронутые колонки переписываются ПОЛНОСТЬЮ (явный ручной порядок → переживает refresh). Источник
 * (если другой) теряет путь; цель получает его на `toIndex` (после удаления из неё же — для внутри-реордера).
 */
export function planMove(
  displayed: Record<string, string[]>,
  drag: DragData,
  toCol: string,
  toIndex: number,
): MovePlan | null {
  if (toCol === OTHER_COLUMN_ID) return null; // в «Прочее» ронять нельзя (нет целевого статуса)
  const { path, fromCol } = drag;

  // `toIndex` — позиция в ОТОБРАЖАЕМОМ списке (с перетаскиваемой карточкой). После её удаления индексы
  // ниже сдвигаются влево: при движении ВНИЗ внутри той же колонки целевой индекс надо уменьшить на 1
  // (иначе «вставка перед карточкой N» промахивается на одну позицию — adversarial-ревью R2).
  const fromIdx = (displayed[toCol] ?? []).indexOf(path);
  let insertAt = toIndex;
  if (fromCol === toCol && fromIdx !== -1 && fromIdx < toIndex) {
    insertAt -= 1;
  }
  const target = (displayed[toCol] ?? []).filter((p) => p !== path);
  const clamped = Math.max(0, Math.min(insertAt, target.length));
  target.splice(clamped, 0, path);

  // No-op: та же колонка и итоговая позиция не изменилась.
  if (fromCol === toCol) {
    const before = displayed[toCol] ?? [];
    if (before.length === target.length && before.every((p, i) => p === target[i])) {
      return null;
    }
  }

  const order: Record<string, string[]> = { [toCol]: target };
  // Источник переписываем, КРОМЕ виртуальной «Прочее» (у неё нет персист-порядка — пересобирается).
  if (fromCol !== toCol && fromCol !== OTHER_COLUMN_ID) {
    order[fromCol] = (displayed[fromCol] ?? []).filter((p) => p !== path);
  }
  return {
    order,
    statusChange: fromCol !== toCol ? { path, status: toCol } : null,
  };
}
