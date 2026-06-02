import Graph from 'graphology';
import forceAtlas2 from 'graphology-layout-forceatlas2';
import type { GraphData } from '../../lib/tauri-api';

export type Positions = Record<string, { x: number; y: number }>;

/**
 * Считает раскладку локального графа (graphology + ForceAtlas2). Чистая функция —
 * выполняется в Web Worker (`layout.worker.ts`), не блокируя main-thread (AC-PERF-6).
 * Стартовые позиции — по кругу (детерминированно, без зависимости от случайности).
 */
export function computeLayout(data: GraphData): Positions {
  const graph = new Graph();
  for (const n of data.nodes) {
    if (!graph.hasNode(String(n.id))) graph.addNode(String(n.id));
  }
  const ids = graph.nodes();
  ids.forEach((id, i) => {
    const angle = (2 * Math.PI * i) / Math.max(1, ids.length);
    graph.setNodeAttribute(id, 'x', Math.cos(angle));
    graph.setNodeAttribute(id, 'y', Math.sin(angle));
  });
  for (const e of data.edges) {
    const s = String(e.source);
    const t = String(e.target);
    if (graph.hasNode(s) && graph.hasNode(t) && !graph.hasEdge(s, t)) {
      graph.addEdge(s, t);
    }
  }
  if (graph.order > 1) {
    forceAtlas2.assign(graph, {
      iterations: 100,
      settings: forceAtlas2.inferSettings(graph),
    });
  }
  const positions: Positions = {};
  graph.forEachNode((id, attr) => {
    positions[id] = { x: attr.x as number, y: attr.y as number };
  });
  return positions;
}
