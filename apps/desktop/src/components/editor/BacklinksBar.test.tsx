import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import { BacklinksBar } from './BacklinksBar';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('BacklinksBar (Ф0-6/Ф0-9)', () => {
  it('показывает входящие ссылки переданного файла', async () => {
    render(<BacklinksBar path="Inbox.md" />); // на Inbox ссылается README (mock)
    expect(await screen.findByText('README')).toBeInTheDocument(); // DP-15: title без .md
  });

  // S6b рескин: карта бэклинка несёт title + context и кликабельна (открывает источник).
  it('карта рендерит title + context и по клику открывает источник', async () => {
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([
      { sourcePath: 'Notes/Src.md', sourceTitle: 'Источник', context: 'строка контекста', lineNumber: 3 },
    ]);
    const openFile = vi.fn();
    useWorkspaceStore.setState({ openFile });
    render(<BacklinksBar path="A.md" />);
    const card = await screen.findByRole('button', { name: /Источник/ });
    expect(screen.getByText('строка контекста')).toBeInTheDocument();
    fireEvent.click(card);
    expect(openFile).toHaveBeenCalledWith('Notes/Src.md');
  });

  it('показывает пустое состояние, когда обратных ссылок нет', async () => {
    render(<BacklinksBar path="Notes/Idea.md" />); // на Idea никто не ссылается
    expect(await screen.findByText(/Нет обратных ссылок/)).toBeInTheDocument();
  });

  // REFRESH: подписка на vault:changed → дебаунс-ре-запрос → новая обратная ссылка без смены файла.
  it('REFRESH: vault:changed пере-запрашивает и показывает новую ссылку', async () => {
    vi.useFakeTimers();
    let fire = () => {};
    vi.spyOn(tauriApi.events, 'onVaultChanged').mockImplementation(async (cb) => {
      fire = cb;
      return () => {};
    });
    const spy = vi
      .spyOn(tauriApi.graph, 'getBacklinks')
      .mockResolvedValueOnce([]) // первичный: пусто
      .mockResolvedValue([
        { sourcePath: 'Notes/Linker.md', sourceTitle: 'Linker', context: null, lineNumber: null },
      ]);
    render(<BacklinksBar path="Pipeline.md" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(spy).toHaveBeenCalledTimes(1);
    await act(async () => {
      fire();
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(spy).toHaveBeenCalledTimes(2);
    expect(screen.getByText('Linker')).toBeInTheDocument();
  });

  it('REFRESH: ошибка тихого рефреша НЕ обнуляет показанные ссылки (#296)', async () => {
    vi.useFakeTimers();
    let fire = () => {};
    vi.spyOn(tauriApi.events, 'onVaultChanged').mockImplementation(async (cb) => {
      fire = cb;
      return () => {};
    });
    vi.spyOn(tauriApi.graph, 'getBacklinks')
      .mockResolvedValueOnce([
        { sourcePath: 'Notes/A.md', sourceTitle: 'Keeper', context: null, lineNumber: null },
      ])
      .mockRejectedValue(new Error('transient'));
    render(<BacklinksBar path="Pipeline.md" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText('Keeper')).toBeInTheDocument();
    await act(async () => {
      fire();
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(screen.getByText('Keeper')).toBeInTheDocument(); // транзиентная ошибка не стёрла список
  });
});
