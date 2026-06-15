import { describe, expect, it } from 'vitest';

import { louvainCommunities } from './louvain';

type E = { source: string; target: string };
const nodes = (...ids: string[]) => ids.map((id) => ({ id }));
const edge = (a: string, b: string): E => ({ source: a, target: b });

/** Все пары внутри клики Kn. */
function clique(ids: string[]): E[] {
  const es: E[] = [];
  for (let i = 0; i < ids.length; i++)
    for (let j = i + 1; j < ids.length; j++) es.push(edge(ids[i], ids[j]));
  return es;
}

/** «Кто-с-кем» — со-кластерность как множество пар id (инвариант относительно перенумерации меток). */
function coClusterPairs(community: Map<string, number>): Set<string> {
  const ids = [...community.keys()].sort();
  const pairs = new Set<string>();
  for (let i = 0; i < ids.length; i++)
    for (let j = i + 1; j < ids.length; j++)
      if (community.get(ids[i]) === community.get(ids[j])) pairs.add(`${ids[i]}|${ids[j]}`);
  return pairs;
}

const seededShuffle = <T>(arr: readonly T[], seed: number): T[] => {
  // Детерминированная перестановка (LCG) — без генератора случайных чисел рантайма.
  const a = arr.slice();
  let s = seed >>> 0;
  for (let i = a.length - 1; i > 0; i--) {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    const j = s % (i + 1);
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
};

describe('louvainCommunities — детекция сообществ (GRAPH-6)', () => {
  it('два K5-клика + соединяющее ребро → ровно 2 сообщества, разбиение как ожидается', () => {
    const A = ['a1', 'a2', 'a3', 'a4', 'a5'];
    const B = ['b1', 'b2', 'b3', 'b4', 'b5'];
    const edges = [...clique(A), ...clique(B), edge('a1', 'b1')];
    const r = louvainCommunities(nodes(...A, ...B), edges);
    expect(r.count).toBe(2);
    // Внутри каждого клика — одно сообщество; между кликами — разные.
    for (const x of A) expect(r.community.get(x)).toBe(r.community.get('a1'));
    for (const x of B) expect(r.community.get(x)).toBe(r.community.get('b1'));
    expect(r.community.get('a1')).not.toBe(r.community.get('b1'));
  });

  it('ДЕТЕРМИНИЗМ: результат не зависит от порядка входных nodes/edges (перетасовка)', () => {
    const A = ['a1', 'a2', 'a3', 'a4', 'a5'];
    const B = ['b1', 'b2', 'b3', 'b4', 'b5'];
    const C = ['c1', 'c2', 'c3', 'c4', 'c5'];
    // 3 клика, соединённые в цепь — достаточно глубоко, чтобы задействовать ≥2 уровня агрегации.
    const ns = nodes(...A, ...B, ...C);
    const es = [...clique(A), ...clique(B), ...clique(C), edge('a1', 'b1'), edge('b2', 'c1')];
    const base = louvainCommunities(ns, es);
    const basePairs = coClusterPairs(base.community);
    for (let seed = 1; seed <= 4; seed++) {
      const r = louvainCommunities(seededShuffle(ns, seed), seededShuffle(es, seed * 7 + 1));
      expect(r.count).toBe(base.count);
      expect(coClusterPairs(r.community)).toEqual(basePairs);
      // И сами канонические метки идентичны (канонизация по размеру стабильна).
      for (const k of base.community.keys())
        expect(r.community.get(k)).toBe(base.community.get(k));
    }
  });

  it('КАНОНИЗАЦИЯ: сообщество 0 — крупнейшее', () => {
    const big = ['x1', 'x2', 'x3', 'x4', 'x5', 'x6'];
    const small = ['y1', 'y2', 'y3'];
    const edges = [...clique(big), ...clique(small), edge('x1', 'y1')];
    const r = louvainCommunities(nodes(...big, ...small), edges);
    expect(r.count).toBe(2);
    expect(r.community.get('x1')).toBe(0); // крупнейший клик → метка 0
    expect(r.community.get('y1')).toBe(1);
  });

  it('модулярность две треугольника + ребро ≈ 0.357 (пин конвенции)', () => {
    const ids = ['a', 'b', 'c', 'd', 'e', 'f'];
    const edges = [
      edge('a', 'b'),
      edge('b', 'c'),
      edge('c', 'a'),
      edge('d', 'e'),
      edge('e', 'f'),
      edge('f', 'd'),
      edge('c', 'd'),
    ];
    const r = louvainCommunities(nodes(...ids), edges);
    expect(r.count).toBe(2);
    expect(r.modularity).toBeCloseTo(0.3571, 3);
  });

  it('вырожденные: пустой граф → 0; без рёбер → n singleton; клика → 1', () => {
    expect(louvainCommunities([], [])).toEqual({ community: new Map(), count: 0, modularity: 0 });

    const iso = louvainCommunities(nodes('p', 'q', 'r'), []);
    expect(iso.count).toBe(3);
    expect(new Set(iso.community.values()).size).toBe(3);
    expect(iso.modularity).toBe(0);

    const k4 = louvainCommunities(nodes('a', 'b', 'c', 'd'), clique(['a', 'b', 'c', 'd']));
    expect(k4.count).toBe(1);
    expect(k4.community.get('a')).toBe(0);
  });

  it('узел без рёбер получает своё сообщество (не -1), рёбра на неизвестные id игнорируются', () => {
    const r = louvainCommunities(nodes('a', 'b', 'c', 'lonely'), [
      edge('a', 'b'),
      edge('b', 'c'),
      edge('c', 'a'),
      edge('a', 'ghost'), // ghost нет в nodes — игнор без падения
    ]);
    expect(r.community.get('lonely')).not.toBeUndefined();
    expect([...r.community.values()].every((v) => v >= 0)).toBe(true);
    // lonely — отдельное сообщество от треугольника.
    expect(r.community.get('lonely')).not.toBe(r.community.get('a'));
  });

  it('реципрокные/параллельные рёбра = одно (простой граф) — нет смещения и рассинхрона модулярности', () => {
    // adversarial-ревью MAJOR: A→B и B→A (взаимные вики-ссылки) НЕ должны тянуть пару к слиянию вдвое
    // сильнее. Результат обязан совпадать с графом без реверс-дублей.
    const ns = nodes('a', 'b', 'c', 'd');
    const withReciprocal = louvainCommunities(ns, [
      edge('a', 'b'),
      edge('b', 'a'), // реверс-дубль
      edge('a', 'c'),
      edge('c', 'd'),
      edge('d', 'a'),
    ]);
    const simple = louvainCommunities(ns, [
      edge('a', 'b'),
      edge('a', 'c'),
      edge('c', 'd'),
      edge('d', 'a'),
    ]);
    expect(withReciprocal.count).toBe(simple.count);
    expect(coClusterPairs(withReciprocal.community)).toEqual(coClusterPairs(simple.community));
    expect(withReciprocal.modularity).toBeCloseTo(simple.modularity, 6);
    // Параллельные дубли того же ребра тоже схлопываются.
    const dup = louvainCommunities(nodes('x', 'y'), [edge('x', 'y'), edge('x', 'y'), edge('y', 'x')]);
    expect(dup.count).toBe(1);
    expect(dup.modularity).toBeCloseTo(0, 6); // одно ребро, один кластер
  });

  it('строковые id (не числовые) канонизируются лексикографически без сбоя порядка', () => {
    // '10' vs '2' — числовой минус сломал бы; localeCompare держит порядок.
    const A = ['10', '2', '30'];
    const B = ['note-a/index', 'note-b/index', 'note-c/index'];
    const edges = [...clique(A), ...clique(B), edge('10', 'note-a/index')];
    const r1 = louvainCommunities(nodes(...A, ...B), edges);
    const r2 = louvainCommunities(seededShuffle(nodes(...A, ...B), 3), seededShuffle(edges, 9));
    expect(r1.count).toBe(2);
    for (const k of r1.community.keys()) expect(r2.community.get(k)).toBe(r1.community.get(k));
  });
});
