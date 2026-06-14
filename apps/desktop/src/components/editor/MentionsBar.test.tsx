import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import { MentionsBar } from './MentionsBar';

afterEach(() => vi.restoreAllMocks());

describe('MentionsBar (UNLINK-1)', () => {
  it('показывает упоминания без ссылки; клик ведёт к источнику', async () => {
    vi.spyOn(tauriApi.graph, 'unlinkedMentions').mockResolvedValue([
      { sourcePath: 'Notes/Idea.md', sourceTitle: 'Idea', snippet: '…про RAG Pipeline тут…' },
    ]);
    const openFile = vi
      .spyOn(useWorkspaceStore.getState(), 'openFile')
      .mockResolvedValue(undefined);
    render(<MentionsBar path="Pipeline.md" />);
    const btn = await screen.findByRole('button', { name: /Idea/ });
    fireEvent.click(btn);
    expect(openFile).toHaveBeenCalledWith('Notes/Idea.md');
  });

  it('нет упоминаний → бар скрыт (не шумит на типичной заметке)', async () => {
    const spy = vi.spyOn(tauriApi.graph, 'unlinkedMentions').mockResolvedValue([]);
    const { container } = render(<MentionsBar path="Pipeline.md" />);
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});
