// Мок канбан-доски (BOARD-2/4): зеркалит контракт Rust `board::list_board` — список заметок-задач
// (frontmatter `status`) с полями для карточек. Сидовый набор покрывает дефолтные колонки
// (todo/doing/done), off-set статус («ожидание» → виртуальная «Прочее»), проекты, приоритеты,
// дедлайны (в т.ч. просроченный) и теги — чтобы превью BoardView было содержательным.

import type { BoardConfig, BoardData, BoardSummary, StaleTask, TaskCard } from '../tauri-api';

const SEED: TaskCard[] = [
  {
    path: 'Tasks/Дизайн доски.md',
    title: 'Дизайн канбан-доски',
    status: 'doing',
    project: 'Nexus',
    priority: 'high',
    due: '2026-06-20',
    tags: ['design', 'task'],
  },
  {
    path: 'Tasks/Перенести заметки.md',
    title: 'Перенести старые заметки',
    status: 'todo',
    project: 'Nexus',
    priority: 'medium',
    due: null,
    tags: ['migration'],
  },
  {
    path: 'Tasks/Оплатить счёт.md',
    title: 'Оплатить счёт за интернет',
    status: 'todo',
    project: 'Дом',
    priority: 'urgent',
    due: '2026-06-14', // просрочено относительно 2026-06-16
    tags: ['быт'],
  },
  {
    path: 'Tasks/Прочитать статью.md',
    title: 'Прочитать статью про RAG',
    status: 'done',
    project: 'Учёба',
    priority: 'low',
    due: null,
    tags: ['reading', 'ai'],
  },
  {
    path: 'Tasks/Согласовать смету.md',
    title: 'Согласовать смету с подрядчиком',
    status: 'ожидание', // вне дефолтного набора → виртуальная колонка «Прочее»
    project: 'Дом',
    priority: 'medium',
    due: '2026-06-25',
    tags: [],
  },
  {
    path: 'Tasks/Релиз 0.9.md',
    title: 'Подготовить релиз 0.9',
    status: 'doing',
    project: 'Nexus',
    priority: 'high',
    due: '2026-06-30',
    tags: ['release', 'task'],
  },
];

/** Все заметки-задачи с полями для доски (сид под дефолтный ключ `status`). */
export async function listBoard(): Promise<TaskCard[]> {
  // Возвращаем копию, чтобы потребитель не мутировал сид.
  return SEED.map((c) => ({ ...c, tags: [...c.tags] }));
}

// BOARD-3: персист-конфиг доски — мутабельный мок (saveBoard переживает в рамках сессии, как бэкенд-файл).
let CONFIG: BoardConfig = {
  id: 'personal',
  title: '',
  statusKey: 'status',
  columns: [
    { id: 'todo', label: '', wip: null, color: null, doneLike: false },
    { id: 'doing', label: '', wip: null, color: null, doneLike: false },
    { id: 'done', label: '', wip: null, color: null, doneLike: true },
  ],
  scope: { folder: null, project: null, tags: [] },
  order: { doing: ['Tasks/Релиз 0.9.md', 'Tasks/Дизайн доски.md'] }, // показываем ручной порядок в превью
  sort: 'manual',
  cardFields: ['due', 'priority', 'tags'],
};

/** Доска целиком (зеркало `get_board`): конфиг + карточки в scope + corrupt. Мок — scope пустой → все. */
export async function getBoard(): Promise<BoardData> {
  return {
    config: structuredClone(CONFIG),
    cards: SEED.map((c) => ({ ...c, tags: [...c.tags] })),
    corrupt: false,
  };
}

/** Персист конфига (зеркало `save_board`) — мутирует мок-стейт. */
export async function saveBoard(config: BoardConfig): Promise<void> {
  CONFIG = structuredClone(config);
}

/** Список досок (зеркало `list_boards`) — всегда ≥1. */
export async function listBoards(): Promise<BoardSummary[]> {
  return [{ id: CONFIG.id, title: CONFIG.title }];
}

/** AI-2a: «застрявшие» задачи (зеркало `stale_tasks`). Сид включает done-задачу — фронт ОБЯЗАН её
 *  отсеять (done-like колонка), демонстрируя контракт фильтрации; статусы зеркалят SEED. */
export async function staleTasks(): Promise<StaleTask[]> {
  return [
    {
      path: 'Tasks/Согласовать смету.md',
      title: 'Согласовать смету с подрядчиком',
      status: 'ожидание',
      lastEdit: 1_747_000_000,
      daysStale: 42,
    },
    {
      path: 'Tasks/Перенести заметки.md',
      title: 'Перенести старые заметки',
      status: 'todo',
      lastEdit: 1_748_000_000,
      daysStale: 21,
    },
    {
      // done-задача застряла по времени, но фронт её НЕ покажет (done-like).
      path: 'Tasks/Прочитать статью.md',
      title: 'Прочитать статью про RAG',
      status: 'done',
      lastEdit: 1_746_000_000,
      daysStale: 50,
    },
  ];
}
