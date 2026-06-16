import { describe, expect, it } from 'vitest';

import type { TaskCard } from '../../lib/tauri-api';
import {
  basename,
  DEFAULT_COLUMN_IDS,
  groupIntoColumns,
  isOverdue,
  knownPriority,
  normalizeStatus,
  OTHER_COLUMN_ID,
  todayIsoLocal,
} from './board-model';

function card(path: string, status: string): TaskCard {
  return { path, title: null, status, project: null, priority: null, due: null, tags: [] };
}

describe('board-model: groupIntoColumns', () => {
  it('раскладывает по дефолтным колонкам, статусы case-insensitive', () => {
    const cols = groupIntoColumns(
      [card('a.md', 'todo'), card('b.md', 'Doing'), card('c.md', 'DONE'), card('d.md', 'todo')],
      DEFAULT_COLUMN_IDS,
    );
    expect(cols.map((c) => c.id)).toEqual(['todo', 'doing', 'done']);
    expect(cols[0].cards.map((c) => c.path)).toEqual(['a.md', 'd.md']);
    expect(cols[1].cards.map((c) => c.path)).toEqual(['b.md']);
    expect(cols[2].cards.map((c) => c.path)).toEqual(['c.md']);
  });

  it('статус вне набора → виртуальная «Прочее» в конце (задачи не теряются)', () => {
    const cols = groupIntoColumns(
      [card('a.md', 'todo'), card('x.md', 'ожидание'), card('y.md', 'backlog')],
      DEFAULT_COLUMN_IDS,
    );
    expect(cols.at(-1)?.id).toBe(OTHER_COLUMN_ID);
    expect(cols.at(-1)?.cards.map((c) => c.path)).toEqual(['x.md', 'y.md']);
  });

  it('пустые колонки набора сохраняются; «Прочее» добавляется только когда непуста', () => {
    const cols = groupIntoColumns([card('a.md', 'todo')], DEFAULT_COLUMN_IDS);
    expect(cols.map((c) => c.id)).toEqual(['todo', 'doing', 'done']); // нет «Прочее»
    expect(cols[1].cards).toEqual([]);
    expect(cols[2].cards).toEqual([]);
  });
});

describe('board-model: утилиты', () => {
  it('normalizeStatus тримит и понижает регистр', () => {
    expect(normalizeStatus('  Doing ')).toBe('doing');
  });

  it('isOverdue: вчера → true, сегодня/завтра → false, невалидная дата → false', () => {
    expect(isOverdue('2026-06-15', '2026-06-16')).toBe(true);
    expect(isOverdue('2026-06-16', '2026-06-16')).toBe(false); // сегодня не просрочено
    expect(isOverdue('2026-06-20', '2026-06-16')).toBe(false);
    expect(isOverdue('скоро', '2026-06-16')).toBe(false);
    expect(isOverdue(null, '2026-06-16')).toBe(false);
  });

  it('basename убирает путь и .md', () => {
    expect(basename('Tasks/Sub/Заметка.md')).toBe('Заметка');
    expect(basename('a.MD')).toBe('a');
  });

  it('todayIsoLocal форматирует YYYY-MM-DD по локальной дате', () => {
    expect(todayIsoLocal(new Date(2026, 0, 5))).toBe('2026-01-05'); // месяц 0-based → январь
  });

  it('knownPriority нормализует набор, прочее → null', () => {
    expect(knownPriority('High')).toBe('high');
    expect(knownPriority('срочно')).toBeNull();
    expect(knownPriority(null)).toBeNull();
  });
});
