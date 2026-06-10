import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { StatusBar } from './StatusBar';

afterEach(() => {
  vi.restoreAllMocks();
  useJobsStore.setState({ counts: { pending: 0, running: 0, dead: 0 } });
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
});
