import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SyncPanel } from './SyncPanel';
import { tauriApi } from '../../lib/tauri-api';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SyncPanel commit — ошибка не глотается (audit B13)', () => {
  it('сбой git.commit показывает error-исход, а не тихо проглатывается', async () => {
    vi.spyOn(tauriApi.git, 'commit').mockRejectedValue(new Error('detached HEAD'));
    render(<SyncPanel />);

    // Кнопка коммита доступна после загрузки статуса (мок отдаёт грязные файлы).
    const btn = await screen.findByRole('button', { name: /закоммитить|^commit$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn);

    expect(await screen.findByText(/detached HEAD/)).toBeInTheDocument();
  });

  // audit B16: сбой setRemote/setToken в saveRemote больше не глотается пустым catch.
  it('сбой setRemote показывает ошибку, а не тихо проглатывается', async () => {
    vi.spyOn(tauriApi.git, 'setRemote').mockRejectedValue(new Error('remote refused'));
    render(<SyncPanel />);
    const urlInput = await screen.findByPlaceholderText(/github\.com\/you\/vault/i);
    fireEvent.change(urlInput, { target: { value: 'https://example.com/repo.git' } });
    fireEvent.click(screen.getByRole('button', { name: /подключить|^connect$/i }));
    expect(await screen.findByText(/remote refused/)).toBeInTheDocument();
  });
});
