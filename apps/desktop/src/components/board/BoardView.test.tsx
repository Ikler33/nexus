import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { BoardView } from './BoardView';
import i18n from '../../i18n/setup';
import { tauriApi, type BoardData } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';

const CARD_MIME = 'application/x-nexus-board-card';

function boardData(): BoardData {
  return {
    config: {
      id: 'personal',
      title: '',
      statusKey: 'status',
      columns: [
        { id: 'todo', label: '', wip: null, color: null, doneLike: false },
        { id: 'doing', label: '', wip: null, color: null, doneLike: false },
        { id: 'done', label: '', wip: null, color: null, doneLike: true },
      ],
      scope: { folder: null, project: null, tags: [] },
      order: {},
      sort: 'manual',
      cardFields: [],
    },
    cards: [
      { path: 't.md', title: 'Task T', status: 'todo', project: null, priority: null, due: null, tags: [] },
    ],
    corrupt: false,
  };
}

/** Мок dataTransfer (jsdom не даёт нативного DnD): несёт наш MIME + дропэффект. */
function dt() {
  return { types: [CARD_MIME], dropEffect: '', effectAllowed: '', setData: vi.fn(), getData: () => 't.md' };
}

/** Колонка по локализованной метке (section role=region с aria-label). */
function column(name: RegExp) {
  return screen.getByRole('region', { name });
}

describe('BoardView DnD (BOARD-5 — optimistic + rollback, §14.6)', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
    vi.restoreAllMocks();
    useWorkspaceStore.setState({ buffers: {} });
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(boardData());
    vi.spyOn(tauriApi.board, 'save').mockResolvedValue(undefined);
  });

  it('перенос в другую колонку: set_frontmatter_field(status) + карточка переезжает', async () => {
    const setFm = vi
      .spyOn(tauriApi.vault, 'setFrontmatterField')
      .mockResolvedValue({ content: '---\nstatus: doing\n---\n', hash: 'h2' });
    render(<BoardView />);
    const card = (await screen.findByText('Task T')).closest('button')!;

    fireEvent.dragStart(card, { dataTransfer: dt() });
    fireEvent.drop(column(/In progress/i), { dataTransfer: dt() });

    await waitFor(() => expect(setFm).toHaveBeenCalledWith('t.md', 'status', 'doing'));
    // Карточка теперь в колонке «In progress» (optimistic), не в «To do».
    await waitFor(() => expect(within(column(/In progress/i)).getByText('Task T')).toBeInTheDocument());
    expect(within(column(/To do/i)).queryByText('Task T')).toBeNull();
    expect(tauriApi.board.save).toHaveBeenCalled();
  });

  it('ошибка записи статуса (битый frontmatter) → ОТКАТ: карточка возвращается в исходную колонку', async () => {
    vi.spyOn(tauriApi.vault, 'setFrontmatterField').mockRejectedValue(new Error('MalformedFrontmatter'));
    const save = vi.spyOn(tauriApi.board, 'save');
    render(<BoardView />);
    const card = (await screen.findByText('Task T')).closest('button')!;

    fireEvent.dragStart(card, { dataTransfer: dt() });
    fireEvent.drop(column(/In progress/i), { dataTransfer: dt() });

    // После провала статуса карточка снова в «To do», порядок НЕ сохранялся (save не вызван).
    await waitFor(() => expect(within(column(/To do/i)).getByText('Task T')).toBeInTheDocument());
    expect(within(column(/In progress/i)).queryByText('Task T')).toBeNull();
    expect(save).not.toHaveBeenCalled();
  });

  it('R1: флаш грязного буфера не удался → frontmatter НЕ тронут, ход отменён (правки тела целы)', async () => {
    const setFm = vi.spyOn(tauriApi.vault, 'setFrontmatterField');
    // Открытый ГРЯЗНЫЙ буфер заметки; saveBuffer «не сохраняет» (остаётся dirty — имитация ошибки записи).
    useWorkspaceStore.setState({
      buffers: { 't.md': { path: 't.md', doc: 'мои правки тела', dirty: true, baseHash: 'h0' } },
    });
    vi.spyOn(useWorkspaceStore.getState(), 'saveBuffer').mockResolvedValue(undefined); // dirty не снят
    render(<BoardView />);
    const card = (await screen.findByText('Task T')).closest('button')!;

    fireEvent.dragStart(card, { dataTransfer: dt() });
    fireEvent.drop(column(/In progress/i), { dataTransfer: dt() });

    // frontmatter НЕ записан (иначе потеряли бы правки тела) → ход откатан, буфер цел.
    await waitFor(() => expect(within(column(/To do/i)).getByText('Task T')).toBeInTheDocument());
    expect(setFm).not.toHaveBeenCalled();
    expect(useWorkspaceStore.getState().buffers['t.md'].doc).toBe('мои правки тела');
  });

  it('R3: фокус во время хода НЕ рефетчит доску (busy-гард против гонки)', async () => {
    let resolveFm: (v: { content: string; hash: string }) => void = () => {};
    vi.spyOn(tauriApi.vault, 'setFrontmatterField').mockReturnValue(
      new Promise((r) => {
        resolveFm = r;
      }),
    );
    const get = vi.spyOn(tauriApi.board, 'get').mockResolvedValue(boardData());
    render(<BoardView />);
    const card = (await screen.findByText('Task T')).closest('button')!;
    const callsAfterMount = get.mock.calls.length;

    fireEvent.dragStart(card, { dataTransfer: dt() });
    fireEvent.drop(column(/In progress/i), { dataTransfer: dt() }); // ход в полёте (Fm не зарезолвлен)
    window.dispatchEvent(new Event('focus')); // фокус во время хода
    await new Promise((r) => setTimeout(r, 0));

    expect(get.mock.calls.length).toBe(callsAfterMount); // load НЕ вызван во время busy
    resolveFm({ content: '---\nstatus: doing\n---\n', hash: 'h2' });
  });
});
