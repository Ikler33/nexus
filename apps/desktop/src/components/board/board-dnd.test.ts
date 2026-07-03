import { describe, expect, it } from 'vitest';

import { OTHER_COLUMN_ID } from '../../lib/board/board-model';
import { planMove } from './board-dnd';

const displayed = {
  todo: ['a.md', 'b.md'],
  doing: ['c.md'],
};

describe('board-dnd: planMove', () => {
  it('кросс-колоночный перенос: статус меняется, обе колонки переписаны', () => {
    const plan = planMove(displayed, { path: 'a.md', fromCol: 'todo' }, 'doing', 0);
    expect(plan).not.toBeNull();
    expect(plan!.statusChange).toEqual({ path: 'a.md', status: 'doing' });
    expect(plan!.order.doing).toEqual(['a.md', 'c.md']); // вставлен на индекс 0
    expect(plan!.order.todo).toEqual(['b.md']); // убран из источника
  });

  it('реордер внутри колонки: статус НЕ меняется, переписана одна колонка', () => {
    const plan = planMove(displayed, { path: 'a.md', fromCol: 'todo' }, 'todo', 2);
    expect(plan).not.toBeNull();
    expect(plan!.statusChange).toBeNull();
    expect(plan!.order.todo).toEqual(['b.md', 'a.md']); // a ушёл в конец
    expect(plan!.order).not.toHaveProperty('doing');
  });

  it('индекс клампится в границы колонки', () => {
    const plan = planMove(displayed, { path: 'c.md', fromCol: 'doing' }, 'todo', 99);
    expect(plan!.order.todo).toEqual(['a.md', 'b.md', 'c.md']); // в конец
  });

  it('реордер ВНИЗ внутри колонки: индекс корректируется (R2 off-by-one)', () => {
    const d = { todo: ['a.md', 'b.md', 'c.md'] };
    // Тащим a (idx 0) НА c (idx 2) = «перед c» → [b, a, c], НЕ [b, c, a].
    expect(planMove(d, { path: 'a.md', fromCol: 'todo' }, 'todo', 2)!.order.todo).toEqual([
      'b.md',
      'a.md',
      'c.md',
    ]);
    // Тащим a (idx 0) на b (idx 1) → no-op (a уже перед b).
    expect(planMove(d, { path: 'a.md', fromCol: 'todo' }, 'todo', 1)).toBeNull();
    // Реордер ВВЕРХ: c (idx 2) на a (idx 0) → [c, a, b].
    expect(planMove(d, { path: 'c.md', fromCol: 'todo' }, 'todo', 0)!.order.todo).toEqual([
      'c.md',
      'a.md',
      'b.md',
    ]);
  });

  it('перенос в «Прочее» запрещён → null', () => {
    expect(planMove(displayed, { path: 'a.md', fromCol: 'todo' }, OTHER_COLUMN_ID, 0)).toBeNull();
  });

  it('no-op: та же колонка и та же позиция → null', () => {
    expect(planMove(displayed, { path: 'a.md', fromCol: 'todo' }, 'todo', 0)).toBeNull();
    expect(planMove(displayed, { path: 'b.md', fromCol: 'todo' }, 'todo', 1)).toBeNull();
  });

  it('перенос ИЗ «Прочее»: статус меняется, persist-порядок «Прочее» НЕ пишется (виртуальная)', () => {
    const d = { todo: ['a.md'], [OTHER_COLUMN_ID]: ['x.md'] };
    const plan = planMove(d, { path: 'x.md', fromCol: OTHER_COLUMN_ID }, 'todo', 0);
    expect(plan!.statusChange).toEqual({ path: 'x.md', status: 'todo' });
    expect(plan!.order.todo).toEqual(['x.md', 'a.md']);
    expect(plan!.order).not.toHaveProperty(OTHER_COLUMN_ID); // «Прочее» не персистим
  });
});
