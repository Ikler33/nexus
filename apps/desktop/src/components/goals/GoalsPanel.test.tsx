import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { GoalsPanel } from './GoalsPanel';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';

afterEach(() => {
  vi.restoreAllMocks();
  useUIStore.setState({ goalsOpen: false });
});

describe('GoalsPanel (#35)', () => {
  it('рендерит цели (бар + бейдж «нет прогресса» D7); клик открывает заметку и закрывает панель', async () => {
    useUIStore.setState({ goalsOpen: true });
    const openFile = vi
      .spyOn(useWorkspaceStore.getState(), 'openFile')
      .mockResolvedValue(undefined);
    render(<GoalsPanel />);

    // Мок отдаёт 3 цели: 2 с прогрессом, 1 без.
    expect(await screen.findByText('Дописать книгу')).toBeInTheDocument();
    expect(screen.getByText('65%')).toBeInTheDocument(); // прогресс-бар %
    expect(screen.getByText(/нет прогресса|no progress/i)).toBeInTheDocument(); // D7

    fireEvent.click(screen.getByText('Дописать книгу'));
    expect(openFile).toHaveBeenCalledWith('Цели/Книга.md');
    expect(useUIStore.getState().goalsOpen).toBe(false);
  });
});
