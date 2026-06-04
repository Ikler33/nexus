// graph-sim.ts — чистая логика force-directed графа (без React/DOM), вынесена для unit-тестов.
// Визуал/интерактив (drag, анимации) живут в GraphView.tsx и проверяются человеком; математика
// раскладки/подсветки — здесь и тестируется детерминированно. Портировано из дизайн-прототипа
// (handoff `graph.jsx`): отталкивание + пружины рёбер + гравитация к центру, alpha-«остывание».

export interface SimNode {
  id: string;
  title: string;
  path: string;
  deg: number;
}
export interface SimEdge {
  a: string;
  b: string;
}
export interface Pos {
  x: number;
  y: number;
  vx: number;
  vy: number;
}
export type Positions = Record<string, Pos>;

/** Логический размер сцены (SVG viewBox). Узлы клампятся внутрь с полями. */
export const STAGE_W = 900;
export const STAGE_H = 620;

/** Неориентированный список смежности по рёбрам. */
export function buildAdjacency(edges: SimEdge[]): Record<string, string[]> {
  const adj: Record<string, string[]> = {};
  for (const e of edges) {
    (adj[e.a] ??= []).push(e.b);
    (adj[e.b] ??= []).push(e.a);
  }
  return adj;
}

/** BFS от `start` до глубины `depth` включительно — множество достижимых id (для локального N-hop). */
export function bfsReachable(
  adj: Record<string, string[]>,
  start: string,
  depth: number,
): Set<string> {
  const dist: Record<string, number> = { [start]: 0 };
  const q = [start];
  while (q.length) {
    const cur = q.shift() as string;
    if (dist[cur] >= depth) continue;
    for (const nb of adj[cur] ?? []) {
      if (!(nb in dist)) {
        dist[nb] = dist[cur] + 1;
        q.push(nb);
      }
    }
  }
  return new Set(Object.keys(dist));
}

/** «Фокус + прямые соседи» для подсветки на hover/drag; `null`, если фокуса нет. */
export function neighborSet(edges: SimEdge[], focus: string | null): Set<string> | null {
  if (!focus) return null;
  const s = new Set<string>([focus]);
  for (const e of edges) {
    if (e.a === focus) s.add(e.b);
    if (e.b === focus) s.add(e.a);
  }
  return s;
}

/** Прямые соседи активной ноты (для kin-колец), без самой ноты. */
export function kinSet(edges: SimEdge[], activeId: string | null): Set<string> {
  const s = new Set<string>();
  if (!activeId) return s;
  for (const e of edges) {
    if (e.a === activeId) s.add(e.b);
    if (e.b === activeId) s.add(e.a);
  }
  s.delete(activeId);
  return s;
}

/** Радиус узла по степени связности (дизайн: 6 + deg·1.7, клампы 6..15). */
export function nodeRadius(deg: number): number {
  return Math.max(6, Math.min(15, 6 + deg * 1.7));
}

/**
 * Один шаг симуляции: отталкивание (O(n²)) + пружины рёбер (rest 96) + гравитация к центру.
 * Мутирует `pos` на месте. `dragging` пиннится (скорость обнуляется, позицию двигает курсор).
 * `alpha` — «температура»; возвращает следующую (остывшую) alpha. При drag держим пол 0.45,
 * чтобы соседи продолжали подтягиваться пружинами.
 */
export function forceStep(
  pos: Positions,
  nodeIds: string[],
  edges: SimEdge[],
  alpha: number,
  dragging: string | null,
  w = STAGE_W,
  h = STAGE_H,
): number {
  const cx = w / 2;
  const cy = h / 2;
  const a = dragging ? Math.max(alpha, 0.45) : alpha;

  for (let i = 0; i < nodeIds.length; i++) {
    for (let j = i + 1; j < nodeIds.length; j++) {
      const A = pos[nodeIds[i]];
      const B = pos[nodeIds[j]];
      if (!A || !B) continue;
      const dx = A.x - B.x;
      const dy = A.y - B.y;
      const d2 = dx * dx + dy * dy || 0.01;
      const f = (6800 / d2) * a;
      const d = Math.sqrt(d2);
      A.vx += (dx / d) * f;
      A.vy += (dy / d) * f;
      B.vx -= (dx / d) * f;
      B.vy -= (dy / d) * f;
    }
  }

  for (const e of edges) {
    const A = pos[e.a];
    const B = pos[e.b];
    if (!A || !B) continue;
    const dx = B.x - A.x;
    const dy = B.y - A.y;
    const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
    const f = (d - 96) * 0.05 * a;
    A.vx += (dx / d) * f;
    A.vy += (dy / d) * f;
    B.vx -= (dx / d) * f;
    B.vy -= (dy / d) * f;
  }

  for (const id of nodeIds) {
    const N = pos[id];
    if (!N) continue;
    if (id === dragging) {
      N.vx = 0;
      N.vy = 0;
      continue;
    }
    N.vx += (cx - N.x) * 0.012 * a;
    N.vy += (cy - N.y) * 0.012 * a;
    N.vx *= 0.85;
    N.vy *= 0.85;
    N.x += N.vx;
    N.y += N.vy;
    N.x = Math.max(40, Math.min(w - 40, N.x));
    N.y = Math.max(36, Math.min(h - 36, N.y));
  }

  return a * 0.94;
}

/**
 * Сеет начальные позиции для новых id по кругу (детерминированный угол + лёгкий джиттер по индексу,
 * без `Math.random` — чтобы раскладка была воспроизводимой и тестируемой). Существующие не трогает.
 */
export function seedPositions(pos: Positions, nodeIds: string[], w = STAGE_W, h = STAGE_H): void {
  const cx = w / 2;
  const cy = h / 2;
  const n = Math.max(1, nodeIds.length);
  nodeIds.forEach((id, i) => {
    if (pos[id]) return;
    const ang = (i / n) * Math.PI * 2;
    const jitter = Math.sin(i * 12.9898) * 18; // детерминированный «шум»
    pos[id] = {
      x: cx + Math.cos(ang) * 160 + jitter,
      y: cy + Math.sin(ang) * 160 + jitter,
      vx: 0,
      vy: 0,
    };
  });
}
