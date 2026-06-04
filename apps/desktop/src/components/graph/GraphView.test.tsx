import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// d3-force замокан (кросс-план #23): forceSimulation сразу проставляет x/y узлам и НЕ запускает
// реальные тики (d3-timer) — рендер детерминирован, нет утечки таймера в тесте. on('tick') зовём раз.
vi.mock('d3-force', () => {
  const chain: unknown = new Proxy(() => chain, { get: () => () => chain });
  return {
    forceManyBody: () => chain,
    forceLink: () => chain,
    forceX: () => chain,
    forceY: () => chain,
    forceCollide: () => chain,
    forceSimulation: (nodes: Array<{ x?: number; y?: number }> = []) => {
      nodes.forEach((n, i) => {
        n.x = 100 + i * 30;
        n.y = 100;
      });
      const sim: Record<string, (...a: unknown[]) => unknown> = {
        force: () => sim,
        on: (...a: unknown[]) => {
          (a[1] as (() => void) | undefined)?.(); // тик-колбэк зовём один раз
          return sim;
        },
        alpha: () => sim,
        alphaTarget: () => sim,
        restart: () => sim,
        stop: () => undefined,
        nodes: () => nodes,
      };
      return sim;
    },
  };
});

import GraphView from './GraphView';
import { tauriApi } from '../../lib/tauri-api';

describe('GraphView render-smoke (кросс-план #23)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('монтируется и рисует узлы full-графа без краха', async () => {
    vi.spyOn(tauriApi.graph, 'getFullGraph').mockResolvedValue({
      nodes: [
        { id: 1, path: 'A.md', title: 'A' },
        { id: 2, path: 'B.md', title: 'B' },
      ],
      edges: [{ source: 1, target: 2 }],
      totalFiles: 2,
      truncated: false,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    render(<GraphView />);
    // Переключаемся на «весь vault» (full) — там холст рисуется без открытого файла (center).
    fireEvent.click(screen.getByText(/весь vault|whole vault/i));

    await waitFor(() => {
      expect(document.querySelector('.graph-svg')).toBeTruthy();
      expect(document.querySelectorAll('.g-dot').length).toBe(2);
    });
  });
});
