import { describe, expect, it } from 'vitest';

import type { StaleTask, TaskCard } from '../tauri-api';
import {
  applyOrder,
  basename,
  DEFAULT_COLUMN_IDS,
  filterStuck,
  filterTasks,
  groupIntoColumns,
  isOverdue,
  knownPriority,
  normalizeStatus,
  OTHER_COLUMN_ID,
  planDay,
  sortTasks,
  stripFrontmatter,
  todayIsoLocal,
} from './board-model';

function card(path: string, status: string): TaskCard {
  return { path, title: null, status, project: null, priority: null, due: null, tags: [] };
}

/** Гибкий конструктор карточки для VIEW-1 тестов. */
function tcard(path: string, o: Partial<TaskCard> = {}): TaskCard {
  return {
    path,
    title: null,
    status: 'todo',
    project: null,
    priority: null,
    due: null,
    tags: [],
    ...o,
  };
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

describe('board-model: planDay (AI-2b)', () => {
  const cols = [
    { id: 'todo', doneLike: false },
    { id: 'doing', doneLike: false },
    { id: 'done', doneLike: true },
  ];
  const task = (
    path: string,
    o: { status?: string; due?: string | null; priority?: string | null } = {},
  ): TaskCard => ({
    path,
    title: null,
    status: o.status ?? 'todo',
    project: null,
    priority: o.priority ?? null,
    due: o.due ?? null,
    tags: [],
  });
  const today = '2026-06-16';

  it('корзины overdue → today → priority; внутри overdue раньше-дата выше', () => {
    const out = planDay(
      [
        task('prio.md', { priority: 'high' }), // priority (нет дедлайна)
        task('due-today.md', { due: today }), // today
        task('overdue-late.md', { due: '2026-06-15' }), // overdue (вчера)
        task('overdue-early.md', { due: '2026-06-01' }), // overdue (раньше → выше)
      ],
      cols,
      today,
    );
    expect(out.map((i) => i.card.path)).toEqual([
      'overdue-early.md',
      'overdue-late.md',
      'due-today.md',
      'prio.md',
    ]);
    expect(out.map((i) => i.bucket)).toEqual(['overdue', 'overdue', 'today', 'priority']);
  });

  it('задачи без причины (нет дедлайна, низкий/нет приоритета) НЕ попадают в план', () => {
    const out = planDay(
      [task('a.md', { priority: 'low' }), task('b.md', { priority: null }), task('c.md', {})],
      cols,
      today,
    );
    expect(out).toHaveLength(0);
  });

  it('done-like задача исключается даже если просрочена', () => {
    const out = planDay([task('d.md', { status: 'done', due: '2026-06-01' })], cols, today);
    expect(out).toHaveLength(0);
  });

  it('priority-корзина: urgent выше high; обрезка по limit', () => {
    const out = planDay(
      [task('h.md', { priority: 'high' }), task('u.md', { priority: 'urgent' })],
      cols,
      today,
      1,
    );
    expect(out.map((i) => i.card.path)).toEqual(['u.md']); // urgent важнее, limit=1
  });
});

describe('board-model: sortTasks (VIEW-1)', () => {
  it('пустой вход → пустой массив', () => {
    expect(sortTasks([], 'due', 'asc')).toEqual([]);
  });

  it('due asc: даты по возрастанию, null-due В КОНЦЕ', () => {
    const out = sortTasks(
      [tcard('a.md'), tcard('b.md', { due: '2026-06-20' }), tcard('c.md', { due: '2026-06-10' })],
      'due',
      'asc',
    );
    expect(out.map((c) => c.path)).toEqual(['c.md', 'b.md', 'a.md']);
  });

  it('due desc: даты по убыванию, но null-due ВСЁ РАВНО в конце (не всплывает)', () => {
    const out = sortTasks(
      [tcard('a.md'), tcard('b.md', { due: '2026-06-20' }), tcard('c.md', { due: '2026-06-10' })],
      'due',
      'desc',
    );
    expect(out.map((c) => c.path)).toEqual(['b.md', 'c.md', 'a.md']); // a.md (null) последний
  });

  it('невалидная дата трактуется как «нет значения» (тонет в конец)', () => {
    const out = sortTasks(
      [tcard('a.md', { due: 'скоро' }), tcard('b.md', { due: '2026-06-10' })],
      'due',
      'asc',
    );
    expect(out.map((c) => c.path)).toEqual(['b.md', 'a.md']);
  });

  it('priority asc: urgent→high→medium→low→неизвестный(в конце)', () => {
    const out = sortTasks(
      [
        tcard('low.md', { priority: 'low' }),
        tcard('x.md', { priority: 'wat' }),
        tcard('urg.md', { priority: 'urgent' }),
        tcard('med.md', { priority: 'medium' }),
        tcard('hi.md', { priority: 'high' }),
      ],
      'priority',
      'asc',
    );
    expect(out.map((c) => c.path)).toEqual(['urg.md', 'hi.md', 'med.md', 'low.md', 'x.md']);
  });

  it('priority desc: low→…→urgent, но неизвестный ВСЁ РАВНО в конце', () => {
    const out = sortTasks(
      [
        tcard('low.md', { priority: 'low' }),
        tcard('x.md', { priority: 'wat' }),
        tcard('urg.md', { priority: 'urgent' }),
        tcard('hi.md', { priority: 'high' }),
      ],
      'priority',
      'desc',
    );
    expect(out.map((c) => c.path)).toEqual(['low.md', 'hi.md', 'urg.md', 'x.md']);
  });

  it('status: регистронезависимая алфавитная сортировка', () => {
    const out = sortTasks(
      [
        tcard('t.md', { status: 'Todo' }),
        tcard('d.md', { status: 'doing' }),
        tcard('n.md', { status: 'DONE' }),
      ],
      'status',
      'asc',
    );
    expect(out.map((c) => c.path)).toEqual(['d.md', 'n.md', 't.md']); // doing < done < todo
  });

  it('title: null → фолбэк basename; алфавит', () => {
    const out = sortTasks([tcard('Zebra.md'), tcard('x.md', { title: 'Apple' })], 'title', 'asc');
    expect(out.map((c) => c.path)).toEqual(['x.md', 'Zebra.md']); // Apple < Zebra
  });

  it('стабильный тай-брейк по пути при равном ключе (в т.ч. оба null-due)', () => {
    const out = sortTasks([tcard('b.md'), tcard('a.md')], 'due', 'asc');
    expect(out.map((c) => c.path)).toEqual(['a.md', 'b.md']);
    // desc не должен переворачивать тай-брейк (он не зависит от dir)
    const outD = sortTasks([tcard('b.md'), tcard('a.md')], 'due', 'desc');
    expect(outD.map((c) => c.path)).toEqual(['a.md', 'b.md']);
  });

  it('НЕ мутирует вход (порядок и идентичность элементов сохранены)', () => {
    const input = [tcard('b.md', { due: '2026-01-02' }), tcard('a.md', { due: '2026-01-01' })];
    const first = input[0];
    const out = sortTasks(input, 'due', 'asc');
    expect(input.map((c) => c.path)).toEqual(['b.md', 'a.md']); // вход не тронут
    expect(input[0]).toBe(first); // та же ссылка
    expect(out.map((c) => c.path)).toEqual(['a.md', 'b.md']); // отсортирована копия
  });
});

describe('board-model: filterTasks (VIEW-1)', () => {
  const cards: TaskCard[] = [
    tcard('a.md', {
      title: 'Design board',
      status: 'todo',
      priority: 'high',
      project: 'Nexus',
      tags: ['design', 'task'],
    }),
    tcard('b.md', {
      title: 'Pay bill',
      status: 'Done',
      priority: 'low',
      project: 'Home',
      tags: ['быт'],
    }),
    tcard('c.md', { title: null, status: 'doing', priority: null, project: 'Nexus', tags: [] }),
  ];

  it('пустой фильтр — no-op (все карточки)', () => {
    expect(filterTasks(cards, {}).map((c) => c.path)).toEqual(['a.md', 'b.md', 'c.md']);
    expect(filterTasks(cards, { status: '', text: '  ' }).map((c) => c.path)).toEqual([
      'a.md',
      'b.md',
      'c.md',
    ]);
  });

  it('status — регистронезависимое равенство', () => {
    expect(filterTasks(cards, { status: 'TODO' }).map((c) => c.path)).toEqual(['a.md']);
  });

  it('priority — равенство; карточка без приоритета не совпадает', () => {
    expect(filterTasks(cards, { priority: 'High' }).map((c) => c.path)).toEqual(['a.md']);
    expect(filterTasks(cards, { priority: 'low' }).map((c) => c.path)).toEqual(['b.md']);
  });

  it('project — равенство (CI), сужает до проекта', () => {
    expect(filterTasks(cards, { project: 'nexus' }).map((c) => c.path)).toEqual(['a.md', 'c.md']);
    expect(filterTasks(cards, { project: 'Home' }).map((c) => c.path)).toEqual(['b.md']);
  });

  it('tag — членство (CI)', () => {
    expect(filterTasks(cards, { tag: 'TASK' }).map((c) => c.path)).toEqual(['a.md']);
    expect(filterTasks(cards, { tag: 'быт' }).map((c) => c.path)).toEqual(['b.md']);
  });

  it('text — подстрока (CI) по заголовку/проекту/тегам', () => {
    expect(filterTasks(cards, { text: 'design' }).map((c) => c.path)).toEqual(['a.md']); // title+tag
    expect(filterTasks(cards, { text: 'HOME' }).map((c) => c.path)).toEqual(['b.md']); // project
    expect(filterTasks(cards, { text: 'быт' }).map((c) => c.path)).toEqual(['b.md']); // tag
  });

  it('несколько полей комбинируются по И', () => {
    expect(filterTasks(cards, { status: 'todo', project: 'Nexus' }).map((c) => c.path)).toEqual([
      'a.md',
    ]);
    expect(filterTasks(cards, { project: 'Nexus', priority: 'high' }).map((c) => c.path)).toEqual([
      'a.md',
    ]);
  });

  it('несовпадающий фильтр → пусто; вход не мутируется', () => {
    expect(filterTasks(cards, { status: 'нет-такого' })).toEqual([]);
    const out = filterTasks(cards, { project: 'Nexus' });
    expect(out).not.toBe(cards); // новый массив
    expect(cards).toHaveLength(3); // вход цел
  });
});
