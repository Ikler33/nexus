import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { collectTasks } from '../../lib/tasks/collect';
import { toggleTaskInPlace } from '../../lib/tasks/toggle';
import type { TaskItem } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { TasksPanel } from './TasksPanel';

vi.mock('../../lib/tasks/collect', () => ({ collectTasks: vi.fn() }));
vi.mock('../../lib/tasks/toggle', () => ({ toggleTaskInPlace: vi.fn() }));
// Навигация ставит курсор отложенным setTimeout через активный CM6-view. В юнит-тесте редактора нет —
// мокаем getActiveEditorView в null, чтобы отложенный колбэк был гарантированным no-op (без обращения
// к возможному stale-view из соседнего теста — иначе флейк под --coverage, класс NAV-4).
vi.mock('../../lib/editor/activeView', () => ({ getActiveEditorView: () => null }));

const mockCollect = vi.mocked(collectTasks);
const mockToggle = vi.mocked(toggleTaskInPlace);

const TASKS: TaskItem[] = [
  { path: 'Inbox.md', line: 1, checked: false, text: 'позвонить', title: 'Inbox' },
  { path: 'Inbox.md', line: 2, checked: true, text: 'оплатить', title: 'Inbox' },
  { path: 'Work.md', line: 5, checked: false, text: 'отчёт', title: 'Work' },
];

beforeEach(async () => {
  await useVaultStore.getState().openVault(''); // мок-vault → info != null (дашборд грузится)
  useUIStore.setState({ tasksOpen: true });
  mockToggle.mockResolvedValue(true);
});
afterEach(() => {
  vi.clearAllMocks();
  useUIStore.setState({ tasksOpen: false });
});

describe('TasksPanel (TASK-1)', () => {
  it('группирует по файлу; фильтр «Открытые» прячет выполненные', async () => {
    mockCollect.mockResolvedValue(TASKS);
    render(<TasksPanel />);
    expect(await screen.findByText('позвонить')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'По файлу' })); // явно режим группировки по файлу
    expect(screen.getByText('отчёт')).toBeInTheDocument();
    expect(screen.queryByText('оплатить')).not.toBeInTheDocument(); // выполненная скрыта в «Открытые»
    expect(screen.getByText('Inbox')).toBeInTheDocument(); // группа-файл (заголовок)
    expect(screen.getByText('Work')).toBeInTheDocument();
  });

  it('фильтр «Все» показывает выполненные', async () => {
    mockCollect.mockResolvedValue(TASKS);
    render(<TasksPanel />);
    await screen.findByText('позвонить');
    fireEvent.click(screen.getByRole('button', { name: 'Все' }));
    expect(screen.getByText('оплатить')).toBeInTheDocument();
  });

  it('клик по чекбоксу зовёт toggleTaskInPlace и прячет выполненную (оптимистично)', async () => {
    mockCollect.mockResolvedValue(TASKS);
    render(<TasksPanel />);
    await screen.findByText('позвонить');
    fireEvent.click(screen.getAllByRole('checkbox')[0]); // «позвонить» (Inbox:1)
    await waitFor(() => expect(mockToggle).toHaveBeenCalledWith('Inbox.md', 1));
    await waitFor(() => expect(screen.queryByText('позвонить')).not.toBeInTheDocument());
  });

  it('клик по тексту задачи открывает заметку (навигация)', async () => {
    mockCollect.mockResolvedValue(TASKS);
    const openFile = vi
      .spyOn(useWorkspaceStore.getState(), 'openFile')
      .mockResolvedValue(undefined);
    render(<TasksPanel />);
    fireEvent.click(await screen.findByText('отчёт'));
    expect(openFile).toHaveBeenCalledWith('Work.md');
  });

  it('пустое состояние, когда открытых задач нет', async () => {
    mockCollect.mockResolvedValue([]);
    render(<TasksPanel />);
    expect(await screen.findByText(/Открытых задач нет/)).toBeInTheDocument();
  });

  // TASK-2: даты относительно РЕАЛЬНОГО сегодня (компонент берёт dateStamp(new Date())) — без fake-timers.
  it('TASK-2: группирует по временным бакетам + бейджи даты и приоритета', async () => {
    const isoOffset = (days: number): string => {
      const d = new Date();
      d.setDate(d.getDate() + days);
      return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    };
    const overdue = isoOffset(-4);
    const dated: TaskItem[] = [
      { path: 'P.md', line: 1, checked: false, text: `просрочено due:${overdue}`, title: 'P' },
      { path: 'P.md', line: 2, checked: false, text: `сегодня 📅 ${isoOffset(0)} ⏫`, title: 'P' },
      { path: 'P.md', line: 3, checked: false, text: 'без даты', title: 'P' },
    ];
    mockCollect.mockResolvedValue(dated);
    render(<TasksPanel />);
    expect(await screen.findByText('Просрочено')).toBeInTheDocument(); // бакет (режим «По дате» по умолчанию)
    expect(screen.getByText('Сегодня')).toBeInTheDocument();
    expect(screen.getByText('Без даты')).toBeInTheDocument();
    expect(screen.getByText(overdue)).toBeInTheDocument(); // бейдж даты просроченной
    expect(screen.getByText('P1')).toBeInTheDocument(); // приоритет ⏫ → P1
  });
});
