import { describe, expect, it } from 'vitest';

import type { StaleTask, TaskCard } from '../../lib/tauri-api';
import {
  applyOrder,
  basename,
  DEFAULT_COLUMN_IDS,
  filterStuck,
  groupIntoColumns,
  isOverdue,
  knownPriority,
  normalizeStatus,
  OTHER_COLUMN_ID,
  stripFrontmatter,
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

  it('stripFrontmatter убирает ведущий блок --- (для превью тела)', () => {
    expect(stripFrontmatter('---\nstatus: todo\n---\n# Тело\nтекст')).toBe('# Тело\nтекст');
    expect(stripFrontmatter('# Без frontmatter\nтекст')).toBe('# Без frontmatter\nтекст');
    expect(stripFrontmatter('---\nx: 1\nбез закрытия\n')).toBe('---\nx: 1\nбез закрытия\n'); // незакрытый → как есть
    expect(stripFrontmatter('---\r\nstatus: todo\r\n---\r\nтело\r\n')).toBe('тело\r\n'); // CRLF
  });

  it('todayIsoLocal форматирует YYYY-MM-DD по локальной дате', () => {
    expect(todayIsoLocal(new Date(2026, 0, 5))).toBe('2026-01-05'); // месяц 0-based → январь
  });

  it('knownPriority нормализует набор, прочее → null', () => {
    expect(knownPriority('High')).toBe('high');
    expect(knownPriority('срочно')).toBeNull();
    expect(knownPriority(null)).toBeNull();
  });

  it('applyOrder: в-order первыми по индексу, новые — после стабильно по пути', () => {
    const cs = [card('a.md', 'todo'), card('b.md', 'todo'), card('c.md', 'todo'), card('d.md', 'todo')];
    const out = applyOrder(cs, ['c.md', 'a.md']).map((c) => c.path);
    expect(out).toEqual(['c.md', 'a.md', 'b.md', 'd.md']); // c,a из order; b,d — по пути
    expect(applyOrder(cs, undefined).map((c) => c.path)).toEqual(['a.md', 'b.md', 'c.md', 'd.md']); // нет order → как есть
    expect(applyOrder(cs, []).map((c) => c.path)).toEqual(['a.md', 'b.md', 'c.md', 'd.md']);
  });
});

describe('board-model: filterStuck (AI-2a)', () => {
  const stale = (path: string, status: string): StaleTask => ({
    path,
    title: null,
    status,
    lastEdit: 0,
    daysStale: 30,
  });
  const cols = [
    { id: 'todo', doneLike: false },
    { id: 'doing', doneLike: false },
    { id: 'Done', doneLike: true }, // регистр id — сверка через normalizeStatus
  ];

  it('убирает задачи в done-like колонках (сверка по normalizeStatus), оставляет в работе', () => {
    const out = filterStuck(
      [stale('a.md', 'todo'), stale('b.md', 'done'), stale('c.md', 'DONE'), stale('d.md', 'doing')],
      cols,
    ).map((s) => s.path);
    expect(out).toEqual(['a.md', 'd.md']); // done/DONE отсеяны
  });

  it('статус вне колонок (виртуальная «Прочее») считается застрявшим — он в работе', () => {
    const out = filterStuck([stale('x.md', 'ожидание')], cols).map((s) => s.path);
    expect(out).toEqual(['x.md']);
  });

  it('нет done-like колонок → ничего не отсеивается', () => {
    const out = filterStuck([stale('a.md', 'done')], [{ id: 'todo', doneLike: false }]);
    expect(out).toHaveLength(1);
  });
});
