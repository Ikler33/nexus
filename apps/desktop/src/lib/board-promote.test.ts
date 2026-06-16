import { beforeEach, describe, expect, it, vi } from 'vitest';

import { promoteToBoard } from './board-promote';
import * as fmEdit from './frontmatter-edit';
import { FlushFailedError } from './frontmatter-edit';
import { tauriApi, type BoardData, type NoteProperty } from './tauri-api';
import { useWorkspaceStore } from '../stores/workspace';

/** Минимальная BoardData с заданными статус-ключом и колонками (остальное — заглушки). */
function board(statusKey: string, colIds: string[]): BoardData {
  return {
    config: {
      id: 'personal',
      title: '',
      statusKey,
      columns: colIds.map((id) => ({ id, label: '', wip: null, color: null, doneLike: false })),
      scope: { folder: null, project: null, tags: [] },
      order: {},
      sort: 'manual',
      cardFields: [],
    },
    cards: [],
    corrupt: false,
  };
}

function prop(key: string, value: string): NoteProperty {
  return { key, value, type: 'text' };
}

describe('promoteToBoard (AI-1 — «На доску», спека §10)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useWorkspaceStore.setState({ buffers: {} });
  });

  it('заметка без status → пишет первую колонку дефолт-доски, kind=promoted', async () => {
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(board('status', ['todo', 'doing', 'done']));
    vi.spyOn(tauriApi.properties, 'forNote').mockResolvedValue([]);
    const write = vi.spyOn(fmEdit, 'writeFrontmatterField').mockResolvedValue();

    const r = await promoteToBoard('Notes/A.md');

    expect(write).toHaveBeenCalledWith('Notes/A.md', 'status', 'todo');
    expect(r).toEqual({ kind: 'promoted', statusKey: 'status', column: 'todo' });
  });

  it('уважает кастомный statusKey и первую колонку конфига', async () => {
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(board('state', ['backlog', 'wip']));
    vi.spyOn(tauriApi.properties, 'forNote').mockResolvedValue([prop('title', 'X')]);
    const write = vi.spyOn(fmEdit, 'writeFrontmatterField').mockResolvedValue();

    const r = await promoteToBoard('Notes/B.md');

    expect(write).toHaveBeenCalledWith('Notes/B.md', 'state', 'backlog');
    expect(r.column).toBe('backlog');
  });

  it('заметка уже с непустым status → kind=already, НЕ перетираем колонку', async () => {
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(board('status', ['todo', 'doing', 'done']));
    vi.spyOn(tauriApi.properties, 'forNote').mockResolvedValue([prop('status', 'doing')]);
    const write = vi.spyOn(fmEdit, 'writeFrontmatterField').mockResolvedValue();

    const r = await promoteToBoard('Notes/C.md');

    expect(write).not.toHaveBeenCalled();
    expect(r).toEqual({ kind: 'already', statusKey: 'status', column: 'doing' });
  });

  it('пустое значение status (есть ключ, но пусто) → промоутим как новую задачу', async () => {
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(board('status', ['todo', 'done']));
    vi.spyOn(tauriApi.properties, 'forNote').mockResolvedValue([prop('status', '   ')]);
    const write = vi.spyOn(fmEdit, 'writeFrontmatterField').mockResolvedValue();

    const r = await promoteToBoard('Notes/D.md');

    expect(write).toHaveBeenCalledWith('Notes/D.md', 'status', 'todo');
    expect(r.kind).toBe('promoted');
  });

  // Ревью AI-1 MAJOR: forNote читает ДИСК. Несохранённый `status` в открытом буфере должен флашиться ДО
  // guard'а, иначе запись откатила бы только что набранную колонку (data-loss класса BOARD-5 R1).
  it('грязный буфер: флашит ДО чтения status → видит сохранённое значение, already, не пишет', async () => {
    useWorkspaceStore.setState({
      buffers: { 'N.md': { path: 'N.md', doc: 'тело', dirty: true, baseHash: 'h0' } },
    });
    const save = vi
      .spyOn(useWorkspaceStore.getState(), 'saveBuffer')
      .mockImplementation(async () => {
        useWorkspaceStore.setState({ buffers: {} }); // флаш снял dirty (status: doing на диске)
      });
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(board('status', ['todo', 'done']));
    vi.spyOn(tauriApi.properties, 'forNote').mockResolvedValue([prop('status', 'doing')]);
    const write = vi.spyOn(fmEdit, 'writeFrontmatterField').mockResolvedValue();

    const r = await promoteToBoard('N.md');

    expect(save).toHaveBeenCalledWith('N.md', true);
    expect(write).not.toHaveBeenCalled();
    expect(r).toEqual({ kind: 'already', statusKey: 'status', column: 'doing' });
  });

  it('грязный буфер, флаш не удался → FlushFailedError, status НЕ читаем и НЕ пишем', async () => {
    useWorkspaceStore.setState({
      buffers: { 'N.md': { path: 'N.md', doc: 'тело', dirty: true, baseHash: 'h0' } },
    });
    vi.spyOn(useWorkspaceStore.getState(), 'saveBuffer').mockResolvedValue(undefined); // dirty не снят
    vi.spyOn(tauriApi.board, 'get').mockResolvedValue(board('status', ['todo']));
    const forNote = vi.spyOn(tauriApi.properties, 'forNote');
    const write = vi.spyOn(fmEdit, 'writeFrontmatterField').mockResolvedValue();

    await expect(promoteToBoard('N.md')).rejects.toBeInstanceOf(FlushFailedError);
    expect(forNote).not.toHaveBeenCalled();
    expect(write).not.toHaveBeenCalled();
  });
});
