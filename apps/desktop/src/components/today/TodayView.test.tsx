import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { TodayView } from './TodayView';
import i18n from '../../i18n/setup';
import { tauriApi, type EpisodeRow, type TaskCard, type TaskItem } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';

const DAILY = 'Journal/2026-06-18.md';

const boardCards: TaskCard[] = [
  { path: 'Tasks/Future.md', title: 'Future board', status: 'todo', project: null, priority: null, due: '2026-06-25', tags: [] },
  { path: 'Tasks/Overdue.md', title: 'Overdue board', status: 'todo', project: 'P', priority: 'high', due: '2026-06-14', tags: [] },
  { path: 'Tasks/Today.md', title: 'Today board', status: 'todo', project: null, priority: null, due: '2026-06-18', tags: [] },
];
const checklist: TaskItem[] = [
  { path: 'Notes/A.md', line: 1, checked: false, text: 'Overdue check 📅 2026-06-14', title: 'A' },
  { path: 'Notes/B.md', line: 2, checked: false, text: 'Today check 📅 2026-06-18', title: 'B' },
  { path: 'Notes/C.md', line: 3, checked: false, text: 'Future check 📅 2026-06-30', title: 'C' },
  { path: 'Notes/D.md', line: 4, checked: true, text: 'Done today 📅 2026-06-18', title: 'D' },
];
const episodes: EpisodeRow[] = [
  { id: 1, sessionId: 7, sessionTitle: 'Сессия про RAG', summary: 'Обсудили ретривал', topics: [], startedAt: 0, endedAt: 2, generatedAt: 0, dismissed: false },
  { id: 2, sessionId: 8, sessionTitle: 'Скрытый эпизод', summary: '—', topics: [], startedAt: 0, endedAt: 1, generatedAt: 0, dismissed: true },
];
const FILES: Record<string, string> = {
  [DAILY]: '---\ntag: journal\n---\n# 2026-06-18\nПлан: написать тесты',
  'Inbox.md': '# Inbox\n- 09:00 первая мысль\n- 10:00 вторая мысль',
};

function mockAll() {
  vi.spyOn(tauriApi.board, 'list').mockResolvedValue(boardCards);
  vi.spyOn(tauriApi.tasks, 'listTasks').mockResolvedValue(checklist);
  vi.spyOn(tauriApi.vault, 'fileHash').mockImplementation((p) =>
    Promise.resolve(FILES[p] !== undefined ? 'h' : null),
  );
  vi.spyOn(tauriApi.vault, 'readFile').mockImplementation((p) => Promise.resolve(FILES[p] ?? ''));
  vi.spyOn(tauriApi.episode, 'list').mockResolvedValue(episodes);
}

describe('TodayView (TODAY-1)', () => {
  beforeEach(async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date(2026, 5, 18, 9, 0, 0)); // 18 июня 2026 — фиксируем «сегодня»
    await i18n.changeLanguage('en');
    useWorkspaceStore.setState({ buffers: {} });
    mockAll();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('секция доски: просроченные раньше сегодняшних, будущие исключены', async () => {
    render(<TodayView />);
    await screen.findByText('Overdue board');
    expect(screen.getByText('Today board')).toBeInTheDocument();
    expect(screen.queryByText('Future board')).toBeNull(); // будущее — не сегодня/просрочено

    const order = screen
      .getAllByRole('button')
      .map((b) => b.textContent || '')
      .filter((t) => /Overdue board|Today board/.test(t))
      .map((t) => (/Overdue board/.test(t) ? 'overdue' : 'today'));
    expect(order).toEqual(['overdue', 'today']);
  });

  it('секция чек-задач: просроченные+сегодня, будущие и выполненные исключены', async () => {
    render(<TodayView />);
    expect(await screen.findByText('Overdue check 📅 2026-06-14')).toBeInTheDocument();
    expect(screen.getByText('Today check 📅 2026-06-18')).toBeInTheDocument();
    expect(screen.queryByText(/Future check/)).toBeNull();
    expect(screen.queryByText(/Done today/)).toBeNull();
  });

  it('заметка дня: показывает тело (frontmatter снят), если файл есть', async () => {
    render(<TodayView />);
    expect(await screen.findByText(/План: написать тесты/)).toBeInTheDocument();
    expect(screen.queryByText(/tag: journal/)).toBeNull(); // frontmatter снят
  });

  it('заметка дня отсутствует → пусто + кнопка; НЕ авто-создаёт (read-only на рендере)', async () => {
    vi.spyOn(tauriApi.vault, 'fileHash').mockImplementation((p) =>
      Promise.resolve(p === 'Inbox.md' ? 'h' : null),
    );
    const writeSpy = vi.spyOn(tauriApi.vault, 'writeFile');
    render(<TodayView />);
    await screen.findByText('Overdue board');
    expect(screen.getByText(i18n.t('today.dailyCreate'))).toBeInTheDocument();
    expect(writeSpy).not.toHaveBeenCalled(); // ни одной записи в vault на рендере
  });

  it('Входящие: счётчик quick-capture строк', async () => {
    render(<TodayView />);
    expect(await screen.findByText(i18n.t('today.inboxCount', { count: 2 }))).toBeInTheDocument();
  });

  it('эпизоды: показывает не-скрытые, скрытые исключены', async () => {
    render(<TodayView />);
    expect(await screen.findByText('Сессия про RAG')).toBeInTheDocument();
    expect(screen.queryByText('Скрытый эпизод')).toBeNull();
  });

  it('fail-safe: сбой загрузки доски → пустое состояние секции, без краха', async () => {
    vi.spyOn(tauriApi.board, 'list').mockRejectedValue(new Error('boom'));
    render(<TodayView />);
    await waitFor(() => expect(screen.getByText(i18n.t('today.boardEmpty'))).toBeInTheDocument());
    // остальные секции продолжают грузиться
    expect(await screen.findByText('Сессия про RAG')).toBeInTheDocument();
  });

  it('клик по задаче доски открывает заметку и закрывает «Сегодня»', async () => {
    const openFile = vi.fn().mockResolvedValue(undefined);
    useWorkspaceStore.setState({ openFile } as never);
    render(<TodayView />);
    fireEvent.click(await screen.findByText('Overdue board'));
    expect(openFile).toHaveBeenCalledWith('Tasks/Overdue.md');
  });
});
