// Мок канбан-доски (BOARD-2/4): зеркалит контракт Rust `board::list_board` — список заметок-задач
// (frontmatter `status`) с полями для карточек. Сидовый набор покрывает дефолтные колонки
// (todo/doing/done), off-set статус («ожидание» → виртуальная «Прочее»), проекты, приоритеты,
// дедлайны (в т.ч. просроченный) и теги — чтобы превью BoardView было содержательным.

import type { TaskCard } from '../tauri-api';

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
