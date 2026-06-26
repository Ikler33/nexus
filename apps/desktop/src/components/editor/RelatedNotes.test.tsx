import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import { RelatedNotes } from './RelatedNotes';

afterEach(() => vi.restoreAllMocks());

describe('RelatedNotes (Inspector «Похожие»)', () => {
  it('рендерит похожие с cosine-score (0–1, toFixed(2)) и открывает по клику', async () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([
      { path: 'Notes/Bravo.md', title: 'Bravo', score: 0.87, reason: 'общий контекст' },
    ]);
    const openFile = vi.fn();
    useWorkspaceStore.setState({ openFile });

    render(<RelatedNotes path="A.md" />);
    const item = await screen.findByRole('button', { name: /Bravo/ });
    // score = 1 − cosine-distance → осмысленная similarity 0–1; формат README §6 (0.87), НЕ проценты.
    expect(screen.getByText('0.87')).toBeInTheDocument();
    expect(screen.queryByText(/%/)).toBeNull(); // фейк-«87%» убран — score честно как 0–1
    expect(screen.getByText(/общий контекст/)).toBeInTheDocument();
    fireEvent.click(item);
    expect(openFile).toHaveBeenCalledWith('Notes/Bravo.md');
  });

  it('пустой результат → заглушка', async () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([]);
    render(<RelatedNotes path="A.md" />);
    expect(await screen.findByText('Нет похожих заметок')).toBeInTheDocument();
  });
});
