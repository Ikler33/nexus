import { describe, expect, it } from 'vitest';
import type { GraphData } from '../../lib/tauri-api';
import { tauriApi } from '../../lib/tauri-api';
import { computeLayout } from './layout';

describe('computeLayout (Ф0-11)', () => {
  it('назначает конечные координаты всем узлам и разводит их', () => {
    const data: GraphData = {
      nodes: [
        { id: 1, path: 'A.md', title: null },
        { id: 2, path: 'B.md', title: null },
        { id: 3, path: 'C.md', title: null },
      ],
      edges: [
        { source: 1, target: 2 },
        { source: 2, target: 3 },
      ],
    };
    const pos = computeLayout(data);
    expect(Object.keys(pos).sort()).toEqual(['1', '2', '3']);
    for (const p of Object.values(pos)) {
      expect(Number.isFinite(p.x)).toBe(true);
      expect(Number.isFinite(p.y)).toBe(true);
    }
    const uniq = new Set(Object.values(pos).map((p) => `${p.x.toFixed(2)},${p.y.toFixed(2)}`));
    expect(uniq.size).toBeGreaterThan(1);
  });

  it('одиночный узел и пустой граф не падают', () => {
    expect(computeLayout({ nodes: [{ id: 1, path: 'A.md', title: null }], edges: [] })['1']).toBeDefined();
    expect(computeLayout({ nodes: [], edges: [] })).toEqual({});
  });
});

describe('getLocalGraph mock (Ф0-11)', () => {
  it('возвращает центр и соседей (2-hop)', async () => {
    const g = await tauriApi.graph.getLocalGraph('README.md', 2);
    const paths = g.nodes.map((n) => n.path);
    expect(paths).toContain('README.md');
    expect(paths).toContain('Inbox.md'); // README -> [[Inbox]]
    expect(g.edges.length).toBeGreaterThan(0);
  });

  it('несуществующий центр → пустой граф', async () => {
    expect(await tauriApi.graph.getLocalGraph('Zzz.md', 2)).toEqual({ nodes: [], edges: [] });
  });
});
