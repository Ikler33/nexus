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
    forceRadial: () => chain,
    forceCollide: () => chain,
    forceSimulation: (nodes: Array<{ x?: number; y?: number }> = []) => {
      nodes.forEach((n, i) => {
        n.x = 100 + i * 30;
        n.y = 100;
      });
      const sim: Record<string, (...a: unknown[]) => unknown> = {
        velocityDecay: () => sim,
        force: () => sim,
        on: (...a: unknown[]) => {
          (a[1] as (() => void) | undefined)?.(); // тик-колбэк зовём один раз
          return sim;
        },
        tick: () => sim, // GRAPH-2: warmup-цикл зовёт sim.tick() (мок — no-op, позиции уже проставлены)
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
import { useWorkspaceStore } from '../../stores/workspace';

describe('GraphView render-smoke (кросс-план #23)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('монтируется и рисует узлы full-графа без краха', async () => {
    vi.spyOn(tauriApi.graph, 'getFullGraph').mockResolvedValue({
      nodes: [
        { id: 1, path: 'A.md', title: 'A', tags: ['demo'] },
        { id: 2, path: 'B.md', title: 'B', tags: [] },
      ],
      edges: [{ source: 1, target: 2 }],
      totalFiles: 2,
      truncated: false,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    // openFile-шпион ставим ДО рендера — компонент захватывает его из стора при маунте
    // (мид-тест setState гонялся бы с ре-рендером onSearchKey).
    const openSpy = vi.fn().mockResolvedValue(undefined);
    useWorkspaceStore.setState({ openFile: openSpy });

    render(<GraphView />);
    // Переключаемся на «весь vault» (full) — там холст рисуется без открытого файла (center).
    fireEvent.click(screen.getByText(/весь vault|whole vault/i));

    await waitFor(() => {
      expect(document.querySelector('.graph-svg')).toBeTruthy();
      expect(document.querySelectorAll('.g-dot').length).toBe(2);
    });

    // Срез «Граф: теги»: чип топ-тега в баре; клик гасит узлы без тега, повторный — сбрасывает.
    const chip = screen.getByRole('button', { name: '#demo' });
    fireEvent.click(chip);
    await waitFor(() => {
      expect(document.querySelectorAll('.g-node.faded').length).toBe(1);
    });
    fireEvent.click(chip);
    await waitFor(() => {
      expect(document.querySelectorAll('.g-node.faded').length).toBe(0);
    });

    // GRAPH-4: поиск — совпадение по заголовку подсвечивается (.hit), прочие гаснут (.faded).
    const searchInput = screen.getByLabelText(/поиск по графу|search the graph/i);
    fireEvent.change(searchInput, { target: { value: 'a' } });
    await waitFor(() => {
      expect(document.querySelectorAll('.g-node.hit').length).toBe(1);
      expect(document.querySelectorAll('.g-node.faded').length).toBe(1); // B погас
    });
    // Счётчик совпадений отражает 1 хит.
    expect(document.querySelector('.graph-search-count')?.textContent).toBe('1');

    // Enter → открывается верхнее совпадение (quick-switcher).
    fireEvent.keyDown(searchInput, { key: 'Enter' });
    await waitFor(() => expect(openSpy).toHaveBeenCalledWith('A.md'));

    // Esc → очищает запрос (подсветка/гашение сбрасываются).
    fireEvent.keyDown(searchInput, { key: 'Escape' });
    await waitFor(() => {
      expect((searchInput as HTMLInputElement).value).toBe('');
      expect(document.querySelectorAll('.g-node.hit').length).toBe(0);
      expect(document.querySelectorAll('.g-node.faded').length).toBe(0);
    });
  });
});
