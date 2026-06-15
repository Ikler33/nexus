import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
} from 'd3-force';
import { describe, expect, it } from 'vitest';

import {
  chargeStrength,
  clampNodePosition,
  CORE_MAX_FACTOR,
  endpointId,
  gravityStrength,
  kinSet,
  neighborSet,
  nodeColor,
  nodeRadius,
  tagHue,
  topTags,
  type EdgeIds,
  type GraphLink,
  type GraphNodeDatum,
} from './graph-sim';

const chain: EdgeIds[] = [
  { source: 'a', target: 'b' },
  { source: 'b', target: 'c' },
  { source: 'c', target: 'd' },
];

describe('graph-sim (помощники подсветки/размера)', () => {
  it('neighborSet: фокус + прямые соседи (или null)', () => {
    expect(neighborSet(chain, null)).toBeNull();
    expect(neighborSet(chain, 'b')).toEqual(new Set(['b', 'a', 'c']));
    expect(neighborSet(chain, 'a')).toEqual(new Set(['a', 'b']));
  });

  it('kinSet: соседи активной ноты без неё самой', () => {
    expect(kinSet(chain, 'b')).toEqual(new Set(['a', 'c']));
    expect(kinSet(chain, 'a')).toEqual(new Set(['b']));
    expect(kinSet(chain, null)).toEqual(new Set());
  });

  it('nodeRadius: формула макета — сирота-точка 3.5, дальше 5.5..15', () => {
    expect(nodeRadius(0)).toBe(3.5); // сирота — точка гало
    expect(nodeRadius(1)).toBeCloseTo(6.6, 5); // 5 + 1·1.6
    expect(nodeRadius(100)).toBe(15); // клампится в 15 (хаб)
  });

  it('endpointId: id из строки или из узла (до/после d3-init)', () => {
    const node = { id: 'x', title: 'X', path: 'x.md', deg: 0, tags: [] } as GraphNodeDatum;
    expect(endpointId('y')).toBe('y');
    expect(endpointId(node)).toBe('x');
  });

  it('tagHue: детерминирован, в 0..359, разные теги — разные оттенки', () => {
    expect(tagHue('demo')).toBe(tagHue('demo'));
    for (const t of ['demo', 'docs', 'planning', 'идеи']) {
      const h = tagHue(t);
      expect(h).toBeGreaterThanOrEqual(0);
      expect(h).toBeLessThan(360);
    }
    expect(tagHue('demo')).not.toBe(tagHue('docs'));
  });

  it('nodeColor: oklch по первому тегу; без тегов — null (фолбэк CSS)', () => {
    expect(nodeColor([])).toBeNull();
    expect(nodeColor(['demo', 'docs'])).toBe(
      `oklch(var(--g-tag-l, 0.55) var(--g-tag-c, 0.12) ${tagHue('demo')})`,
    );
  });

  it('topTags: по частоте, ничья по алфавиту, обрезка по лимиту', () => {
    const nodes = [
      { tags: ['b', 'a'] },
      { tags: ['b'] },
      { tags: ['c'] },
      { tags: [] },
    ];
    expect(topTags(nodes, 8)).toEqual(['b', 'a', 'c']);
    expect(topTags(nodes, 2)).toEqual(['b', 'a']);
    expect(topTags([], 8)).toEqual([]);
  });
});

// ── GRAPH-1: физика (детерминированный snapshot-тест когезии — доказываем, что ретюн убрал разлёт) ──
describe('graph-sim физика (GRAPH-1)', () => {
  it('chargeStrength: степенной кап min(deg,8) и слабый фактор сироты', () => {
    // deg-40 заряжен как deg-8 (кап) — мега-хаб не «бомба».
    expect(chargeStrength({ deg: 40 }, 300)).toBe(chargeStrength({ deg: 8 }, 300));
    expect(chargeStrength({ deg: 8 }, 300)).toBe(-(300 + 8 * 30));
    expect(chargeStrength({ deg: 0 }, 300)).toBe(-300);
    // сирота расталкивается слабо (×0.12), без степенного члена.
    expect(chargeStrength({ deg: 0, ring: 200 }, 300)).toBeCloseTo(-(300 * 0.12), 6);
  });

  it('gravityStrength: сирота 0, локальный 0.6× глобального', () => {
    expect(gravityStrength({ ring: 200 }, 0.085, true)).toBe(0);
    expect(gravityStrength({}, 0.085, true)).toBe(0.085);
    expect(gravityStrength({}, 0.085, false)).toBeCloseTo(0.085 * 0.6, 6);
  });

  it('clampNodePosition: сирота в полосе кольца; связный ≤ coreMax; границы сцены', () => {
    const W = 1500;
    const H = 1300;
    const cx = W / 2;
    const cy = H / 2;
    const coreMax = Math.min(W, H) * CORE_MAX_FACTOR;
    // Сирота за пределами полосы [0.78R,1.18R] → снап внутрь.
    const orphan: GraphNodeDatum = mk('o', 0, cx + 400, cy, 100);
    clampNodePosition(orphan, cx, cy, W, H, true);
    const od = Math.hypot((orphan.x ?? 0) - cx, (orphan.y ?? 0) - cy);
    expect(od).toBeGreaterThanOrEqual(100 * 0.78 - 0.01);
    expect(od).toBeLessThanOrEqual(100 * 1.18 + 0.01);
    // Связный глобального дальше coreMax → снап на coreMax.
    const far: GraphNodeDatum = mk('f', 3, cx + 1000, cy);
    clampNodePosition(far, cx, cy, W, H, true);
    expect(Math.hypot((far.x ?? 0) - cx, (far.y ?? 0) - cy)).toBeCloseTo(coreMax, 3);
    // Границы сцены (margin 20).
    const oob: GraphNodeDatum = mk('b', 1, -500, 5000);
    clampNodePosition(oob, cx, cy, W, H, false);
    expect(oob.x).toBe(20);
    expect(oob.y).toBe(H - 20);
  });

  it('ретюн сжимает раскладку: новые силы держат узлы плотнее старых (фикс разлёта), без NaN', () => {
    const W = 1500;
    const H = 1300;
    const cx = W / 2;
    const cy = H / 2;
    const { seed, links } = buildSeedGraph(cx, cy);

    const oldNodes = settle(seed, links, {
      cx,
      cy,
      charge: (d) => -(360 * (d.ring ? 0.12 : 1) + d.deg * 30), // старая формула (без капа)
      gravity: (d) => (d.ring ? 0 : 0.022), // старая глобальная гравитация (max(0.012,0.022))
      linkDist: 62,
      distMax: 950,
      distMin: 1,
    });
    const newNodes = settle(seed, links, {
      cx,
      cy,
      charge: (d) => chargeStrength(d, 300),
      gravity: (d) => gravityStrength(d, 0.085, true),
      linkDist: 46,
      distMax: 340,
      distMin: 14,
    });

    const meanR = (ns: GraphNodeDatum[]) => {
      const conn = ns.filter((n) => !n.ring);
      return conn.reduce((s, n) => s + Math.hypot((n.x ?? 0) - cx, (n.y ?? 0) - cy), 0) / conn.length;
    };
    // Новая физика держит связные узлы ЗАМЕТНО ближе к центру — разлёт ушёл.
    expect(meanR(newNodes)).toBeLessThan(meanR(oldNodes) * 0.8);
    // Никаких NaN после укладки.
    for (const n of newNodes) {
      expect(Number.isFinite(n.x)).toBe(true);
      expect(Number.isFinite(n.y)).toBe(true);
    }
  });
});

/** Узел для тестов. */
function mk(id: string, deg: number, x: number, y: number, ring?: number): GraphNodeDatum {
  return { id, title: id, path: `${id}.md`, deg, tags: [], x, y, vx: 0, vy: 0, ring };
}

/** Детерминированный seed-граф: 2 хаба со спицами (связаны), + 2 сироты. Фикс-позиции (разлёт по сцене). */
function buildSeedGraph(cx: number, cy: number): { seed: GraphNodeDatum[]; links: GraphLink[] } {
  const seed: GraphNodeDatum[] = [];
  const links: GraphLink[] = [];
  const hubs = ['h0', 'h1'];
  for (let h = 0; h < hubs.length; h++) {
    // Хабы разнесены далеко от центра — проверяем, что гравитация их стянет.
    seed.push(mk(hubs[h], 5, cx + (h === 0 ? -600 : 600), cy - 300));
    for (let s = 0; s < 5; s++) {
      const id = `s${h}_${s}`;
      seed.push(mk(id, 1, cx + (h === 0 ? -600 : 600) + s * 40 - 80, cy - 300 + (s - 2) * 40));
      links.push({ source: hubs[h], target: id });
    }
  }
  links.push({ source: 'h0', target: 'h1' }); // два кластера связаны
  // 2 сироты на кольце.
  seed.push(mk('o0', 0, cx + 500, cy + 400, 480));
  seed.push(mk('o1', 0, cx - 500, cy + 400, 480));
  return { seed, links };
}

/** Прогоняет d3-force до укладки (без клампов — чистый баланс сил), возвращает узлы. Детерминирован
 *  (фикс seed-позиции, без рандома). */
function settle(
  seed: GraphNodeDatum[],
  links: GraphLink[],
  opts: {
    cx: number;
    cy: number;
    charge: (d: GraphNodeDatum) => number;
    gravity: (d: GraphNodeDatum) => number;
    linkDist: number;
    distMax: number;
    distMin: number;
  },
): GraphNodeDatum[] {
  const nodes = seed.map((n) => ({ ...n })); // не мутируем seed между прогонами
  const linkObjs: GraphLink[] = links.map((l) => ({ source: l.source, target: l.target }));
  const sim = forceSimulation<GraphNodeDatum, GraphLink>(nodes)
    .velocityDecay(0.45)
    .force(
      'charge',
      forceManyBody<GraphNodeDatum>().strength(opts.charge).distanceMin(opts.distMin).distanceMax(opts.distMax),
    )
    .force(
      'link',
      forceLink<GraphNodeDatum, GraphLink>(linkObjs)
        .id((d) => d.id)
        .distance(opts.linkDist),
    )
    .force('x', forceX<GraphNodeDatum>(opts.cx).strength(opts.gravity))
    .force('y', forceY<GraphNodeDatum>(opts.cy).strength(opts.gravity))
    .force('collide', forceCollide<GraphNodeDatum>().radius((d) => nodeRadius(d.deg) + 6))
    .stop();
  for (let i = 0; i < 250; i++) sim.tick();
  return nodes;
}
