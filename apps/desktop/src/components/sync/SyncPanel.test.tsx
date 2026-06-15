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
});
