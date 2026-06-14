import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { RelatedNotesSection } from './RelatedNotesSection';
import { tauriApi } from '../../lib/tauri-api';
import { useToastStore } from '../../stores/toast';
import { useWorkspaceStore } from '../../stores/workspace';

afterEach(() => vi.restoreAllMocks());

describe('RelatedNotesSection (FLOW: новость → заметки vault)', () => {
  it('рендерит связанные заметки; клик открывает заметку в редакторе', async () => {
    const openFile = vi.fn().mockResolvedValue(undefined);
    useWorkspaceStore.setState({ openFile });
    vi.spyOn(tauriApi.news, 'related').mockResolvedValue([
      { path: 'Заметки/RAG.md', title: 'RAG на заметках', score: 0.031, reason: 'про retrieval' },
    ]);

    render(<RelatedNotesSection itemId={1} />);

    const card = await screen.findByRole('button', { name: /RAG на заметках/ });
    expect(screen.getByText('про retrieval')).toBeInTheDocument();
    fireEvent.click(card);
    expect(openFile).toHaveBeenCalledWith('Заметки/RAG.md');
  });

  it('stale-ссылка (openFile реджектится) → тост-ошибка, без тихого провала', async () => {
    useWorkspaceStore.setState({ openFile: vi.fn().mockRejectedValue(new Error('перемещена')) });
    const addToast = vi.spyOn(useToastStore.getState(), 'addToast');
    vi.spyOn(tauriApi.news, 'related').mockResolvedValue([
      { path: 'Notes/Gone.md', title: 'Удалённая', score: 0.02, reason: 'x' },
    ]);

    render(<RelatedNotesSection itemId={1} />);
    fireEvent.click(await screen.findByRole('button', { name: /Удалённая/ }));

    await waitFor(() => expect(addToast).toHaveBeenCalled());
    expect(addToast.mock.calls[0][1]).toMatchObject({ kind: 'error' });
  });

  it('переход success→empty при смене itemId реально схлопывает секцию (не вакуумный грин)', async () => {
    vi.spyOn(tauriApi.news, 'related').mockImplementation((id) =>
      Promise.resolve(
        id === 1
          ? [{ path: 'Notes/Idea.md', title: 'Idea', score: 0.03, reason: 'идея' }]
          : [],
      ),
    );

    const { rerender } = render(<RelatedNotesSection itemId={1} />);
    // Сначала секция РЕНДЕРИТСЯ (доказываем ненулевой DOM до перехода).
    expect(await screen.findByText('Idea')).toBeInTheDocument();

    rerender(<RelatedNotesSection itemId={2} />);
    // Пустой результат для id=2 → секция исчезает (empty-ветка реально отработала).
    await waitFor(() => expect(screen.queryByText('Idea')).not.toBeInTheDocument());
    expect(screen.queryByText(/связанные заметки|related notes/i)).not.toBeInTheDocument();
  });

  it('ошибка бэкенда → секция тихо не появляется (без падения)', async () => {
    vi.spyOn(tauriApi.news, 'related').mockRejectedValue(new Error('no RAG'));
    const { container } = render(<RelatedNotesSection itemId={3} />);
    await waitFor(() => expect(tauriApi.news.related).toHaveBeenCalled());
    // Дожидаемся флаша catch-микротаска: контейнер остаётся пустым и после отклонения.
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });
});
