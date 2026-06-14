import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { discard, loadInbox, toNote, toTask } from '../../lib/inbox/actions';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { InboxPanel } from './InboxPanel';

vi.mock('../../lib/inbox/actions', () => ({
  loadInbox: vi.fn(),
  toTask: vi.fn(),
  toNote: vi.fn(),
  discard: vi.fn(),
}));

const mockLoad = vi.mocked(loadInbox);
const mockToTask = vi.mocked(toTask);
const mockToNote = vi.mocked(toNote);
const mockDiscard = vi.mocked(discard);

beforeEach(async () => {
  await useVaultStore.getState().openVault(''); // info != null → панель грузится
  useUIStore.setState({ inboxOpen: true });
  mockToTask.mockResolvedValue(true);
  mockToNote.mockResolvedValue(true);
  mockDiscard.mockResolvedValue(true);
});
afterEach(() => {
  vi.clearAllMocks();
  useUIStore.setState({ inboxOpen: false });
});

describe('InboxPanel (INBOX-1)', () => {
  it('рендерит захваты (время + текст) и действия; клик «В задачу» зовёт toTask', async () => {
    mockLoad.mockResolvedValue([{ line: 2, time: '09:00', text: 'позвонить' }]);
    render(<InboxPanel />);
    expect(await screen.findByText('позвонить')).toBeInTheDocument();
    expect(screen.getByText('09:00')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'В задачу (в дневник)' }));
    await waitFor(() =>
      expect(mockToTask).toHaveBeenCalledWith({ line: 2, time: '09:00', text: 'позвонить' }),
    );
  });

  it('клик «Удалить» зовёт discard', async () => {
    mockLoad.mockResolvedValue([{ line: 2, time: '09:00', text: 'мусор' }]);
    render(<InboxPanel />);
    await screen.findByText('мусор');
    fireEvent.click(screen.getByRole('button', { name: 'Удалить' }));
    await waitFor(() => expect(mockDiscard).toHaveBeenCalled());
  });

  it('клик «В заметку» зовёт toNote и закрывает панель (навигация)', async () => {
    mockLoad.mockResolvedValue([{ line: 2, time: '09:00', text: 'идея' }]);
    render(<InboxPanel />);
    await screen.findByText('идея');
    fireEvent.click(screen.getByRole('button', { name: 'В новую заметку' }));
    await waitFor(() => expect(mockToNote).toHaveBeenCalled());
    await waitFor(() => expect(useUIStore.getState().inboxOpen).toBe(false));
  });

  it('пустое состояние', async () => {
    mockLoad.mockResolvedValue([]);
    render(<InboxPanel />);
    expect(await screen.findByText(/Входящие пусты/)).toBeInTheDocument();
  });
});
