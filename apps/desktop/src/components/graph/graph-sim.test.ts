import { describe, expect, it } from 'vitest';

import {
  bfsReachable,
  buildAdjacency,
  forceStep,
  kinSet,
  neighborSet,
  nodeRadius,
  seedPositions,
  type Positions,
  type SimEdge,
} from './graph-sim';

const chain: SimEdge[] = [
  { a: 'a', b: 'b' },
  { a: 'b', b: 'c' },
  { a: 'c', b: 'd' },
];

describe('graph-sim (V-граф интерактив)', () => {
  it('bfsReachable: локальный N-hop расширяется по глубине', () => {
    const adj = buildAdjacency(chain);
    expect(bfsReachable(adj, 'a', 1)).toEqual(new Set(['a', 'b']));
    expect(bfsReachable(adj, 'a', 2)).toEqual(new Set(['a', 'b', 'c']));
    expect(bfsReachable(adj, 'a', 3)).toEqual(new Set(['a', 'b', 'c', 'd']));
    // изолированный центр — только он сам
    expect(bfsReachable(buildAdjacency([]), 'x', 2)).toEqual(new Set(['x']));
  });

  it('neighborSet: фокус + прямые соседи (или null)', () => {
    expect(neighborSet(chain, null)).toBeNull();
    expect(neighborSet(chain, 'b')).toEqual(new Set(['b', 'a', 'c']));
  });

  it('kinSet: соседи активной ноты без неё самой', () => {
    expect(kinSet(chain, 'b')).toEqual(new Set(['a', 'c']));
    expect(kinSet(chain, null)).toEqual(new Set());
  });

  it('nodeRadius: растёт со степенью, клампится 6..15', () => {
    expect(nodeRadius(0)).toBe(6);
    expect(nodeRadius(3)).toBeCloseTo(11.1, 5);
    expect(nodeRadius(100)).toBe(15);
  });

  it('forceStep: пружина стягивает связанные далёкие узлы', () => {
    const pos: Positions = {
      a: { x: 100, y: 310, vx: 0, vy: 0 },
      b: { x: 800, y: 310, vx: 0, vy: 0 },
    };
    const before = pos.b.x - pos.a.x;
    forceStep(pos, ['a', 'b'], [{ a: 'a', b: 'b' }], 1, null);
    const after = pos.b.x - pos.a.x;
    expect(pos.a.x).toBeGreaterThan(100); // a поехал вправо к b
    expect(pos.b.x).toBeLessThan(800); // b поехал влево к a
    expect(after).toBeLessThan(before); // дистанция сократилась
  });

  it('forceStep: отталкивание разводит несвязанные близкие узлы', () => {
    const pos: Positions = {
      a: { x: 445, y: 310, vx: 0, vy: 0 },
      b: { x: 455, y: 310, vx: 0, vy: 0 },
    };
    const before = Math.abs(pos.b.x - pos.a.x);
    forceStep(pos, ['a', 'b'], [], 1, null);
    expect(Math.abs(pos.b.x - pos.a.x)).toBeGreaterThan(before);
  });

  it('forceStep: перетаскиваемый узел запиннен (позиция не меняется), alpha остывает', () => {
    const pos: Positions = {
      a: { x: 200, y: 200, vx: 0, vy: 0 },
      b: { x: 600, y: 400, vx: 0, vy: 0 },
    };
    const next = forceStep(pos, ['a', 'b'], [{ a: 'a', b: 'b' }], 1, 'a');
    expect(pos.a).toEqual({ x: 200, y: 200, vx: 0, vy: 0 }); // a не сдвинулся
    expect(pos.b.x).not.toBe(600); // b — свободен, поехал
    expect(next).toBeCloseTo(0.94, 5); // 1 * 0.94
  });

  it('seedPositions: сеет недостающие детерминированно, существующие не трогает', () => {
    const pos: Positions = { keep: { x: 1, y: 2, vx: 3, vy: 4 } };
    seedPositions(pos, ['keep', 'n0', 'n1']);
    expect(pos.keep).toEqual({ x: 1, y: 2, vx: 3, vy: 4 });
    expect(pos.n0).toBeDefined();
    expect(pos.n1).toBeDefined();
    // детерминизм: повтор даёт те же координаты
    const pos2: Positions = {};
    seedPositions(pos2, ['n0', 'n1']);
    const pos3: Positions = {};
    seedPositions(pos3, ['n0', 'n1']);
    expect(pos2).toEqual(pos3);
  });
});
