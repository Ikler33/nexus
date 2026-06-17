import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import { MentionsBar } from './MentionsBar';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

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

  // REFRESH: подписка на vault:changed → дебаунс-ре-запрос → новое упоминание появляется без смены файла.
  it('REFRESH: vault:changed пере-запрашивает и показывает новое упоминание', async () => {
    vi.useFakeTimers();
    let fire = () => {};
    vi.spyOn(tauriApi.events, 'onVaultChanged').mockImplementation(async (cb) => {
      fire = cb;
      return () => {};
    });
    const spy = vi
      .spyOn(tauriApi.graph, 'unlinkedMentions')
      .mockResolvedValueOnce([]) // первичный: пусто → бар скрыт
      .mockResolvedValue([{ sourcePath: 'Notes/New.md', sourceTitle: 'NewMention', snippet: 's' }]);
    render(<MentionsBar path="Pipeline.md" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0); // флашим первичный fetch + подписку
    });
    expect(spy).toHaveBeenCalledTimes(1);
    await act(async () => {
      fire(); // индексатор отработал
      await vi.advanceTimersByTimeAsync(1500); // дебаунс → тихий рефреш
    });
    expect(spy).toHaveBeenCalledTimes(2);
    expect(screen.getByText('NewMention')).toBeInTheDocument();
  });

  it('REFRESH: ошибка тихого рефреша НЕ обнуляет уже показанные упоминания (#296)', async () => {
    vi.useFakeTimers();
    let fire = () => {};
    vi.spyOn(tauriApi.events, 'onVaultChanged').mockImplementation(async (cb) => {
      fire = cb;
      return () => {};
    });
    vi.spyOn(tauriApi.graph, 'unlinkedMentions')
      .mockResolvedValueOnce([{ sourcePath: 'Notes/A.md', sourceTitle: 'Keeper', snippet: 's' }])
      .mockRejectedValue(new Error('transient')); // рефреш падает
    render(<MentionsBar path="Pipeline.md" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText('Keeper')).toBeInTheDocument();
    await act(async () => {
      fire();
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(screen.getByText('Keeper')).toBeInTheDocument(); // не обнулилось на транзиентной ошибке
  });
});
