import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { useSyncStore } from '../../stores/sync';
import { useUIStore } from '../../stores/ui';
import { StatusBar } from './StatusBar';

afterEach(() => {
  vi.restoreAllMocks();
  useJobsStore.setState({ counts: { pending: 0, running: 0, dead: 0 } });
  useSyncStore.setState({ mergeRequired: false, conflictFiles: null });
  useUIStore.setState({ conflictOpen: false });
});

describe('StatusBar — индикатор задач (ADR-007 срез 5 / DP-4)', () => {
  it('занятый планировщик → прогресс «N задач»; ошибки — отдельным бейджем', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 1, pending: 2, dead: 1 });
    render(<StatusBar />);
    await waitFor(() => expect(screen.getByText(/3 задач|3 tasks/)).toBeInTheDocument());
    expect(screen.getByText(/⚠ 1/)).toBeInTheDocument();
  });

  it('пустая очередь → индикатора нет; right-блок Local/UTF-8/Markdown на месте', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    render(<StatusBar />);
    await waitFor(() => expect(tauriApi.scheduler.counts).toHaveBeenCalled());
    expect(screen.queryByText(/задач|tasks|⚠/)).toBeNull();
    expect(screen.getByText(/локально|local/i)).toBeInTheDocument();
    expect(screen.getByText('UTF-8')).toBeInTheDocument();
    expect(screen.getByText('Markdown')).toBeInTheDocument();
  });

  // ── DP-14 (макет app.jsx StatusBar): synced/изменения слева, «Проиндексировано · N», пилюля ──

  it('чистое дерево → «Синхронизировано»; пустая очередь → «Проиндексировано · N»', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    vi.spyOn(tauriApi.git, 'status').mockResolvedValue([]);
    vi.spyOn(tauriApi.vault, 'notesCount').mockResolvedValue(42);
    render(<StatusBar />);
    await waitFor(() =>
      expect(screen.getByText(/Синхронизировано|Synced/)).toBeInTheDocument(),
    );
    expect(screen.getByText(/Проиндексировано|Indexed/)).toBeInTheDocument();
    expect(screen.getByText(/· 42/)).toBeInTheDocument();
  });

  it('правки в дереве → «Изменения · N»; merge-required → конфликт-пилюля открывает резолвер', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    vi.spyOn(tauriApi.git, 'status').mockResolvedValue([
      { path: 'a.md', kind: 'modified' },
      { path: 'b.md', kind: 'new' },
    ]);
    vi.spyOn(tauriApi.vault, 'notesCount').mockResolvedValue(7);
    useSyncStore.setState({ mergeRequired: true, conflictFiles: 2 });
    render(<StatusBar />);
    await waitFor(() => expect(screen.getByText(/Изменения · 2|Changes · 2/)).toBeInTheDocument());
    const pill = screen.getByRole('button', { name: /2 конфликта|2 conflicts/ });
    fireEvent.click(pill);
    expect(useUIStore.getState().conflictOpen).toBe(true);
  });
});
