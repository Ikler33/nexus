import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { StatusBar } from './StatusBar';

afterEach(() => {
  vi.restoreAllMocks();
  useJobsStore.setState({ counts: { pending: 0, running: 0, dead: 0 } });
});

describe('StatusBar — индикатор задач (ADR-007 срез 5)', () => {
  it('показывает running/pending по данным планировщика', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 1, pending: 2, dead: 0 });
    render(<StatusBar />);
    await waitFor(() => expect(screen.getByText(/⚙ 1/)).toBeInTheDocument());
    expect(screen.getByText(/⏳ 2/)).toBeInTheDocument();
  });

  it('пустая очередь → индикатора нет', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    render(<StatusBar />);
    await waitFor(() => expect(tauriApi.scheduler.counts).toHaveBeenCalled());
    expect(screen.queryByText(/⚙|⏳|⚠/)).toBeNull();
  });
});
