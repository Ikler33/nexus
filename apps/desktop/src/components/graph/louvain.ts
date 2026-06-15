// louvain.ts — детерминированная детекция сообществ (Louvain, модулярность) для графа vault.
// GRAPH-6: чистая функция от топологии (без React/DOM/d3), как graph-sim.ts. Без npm-зависимостей
// (graphology был бы слепой зоной supply-chain-CI) и БЕЗ генератора случайных чисел — детерминизм
// держится на фикс-порядке обхода узлов (сортировка id), целочисленных весах и явных tie-break.
// Вызывается раз на смену данных графа (useMemo по [graph]), результат красит/группирует узлы.

export interface CommunityResult {
  /** id заметки → канонический id сообщества (0 = крупнейшее). Покрывает КАЖДЫЙ узел. */
  community: Map<string, number>;
  /** число сообществ. */
  count: number;
  /** модулярность разбиения на исходном (плоском) графе, текстбук-конвенция. ≥0 при кластерах. */
  modularity: number;
}

/** Уровень иерархии Louvain: плоский взвешенный граф без петель в `adj` (петли — в `self`). */
interface Level {
  n: number;
  /** Внешние соседи (индексы) каждого узла, в порядке возрастания. */
  adj: number[][];
  /** Веса рёбер, параллельно `adj` (целые). */
  w: number[][];
  /** Вес петли узла (внутренние рёбра агрегированного сообщества); в степень входит ×2. */
  self: number[];
  /** Степень узла = Σ внешних весов + 2·self (целая). */
  deg: number[];
  /** Удвоенный суммарный вес рёбер (Σ deg). `m = m2 / 2`. */
  m2: number;
}

/**
 * Детекция сообществ методом Louvain. Детерминированно для одинаковой топологии независимо от
 * порядка входных `nodes`/`edges` (узлы сортируются по id, агрегация — по фикс-индексам).
 */
export function louvainCommunities(
  nodes: readonly { id: string }[],
  edges: readonly { source: string; target: string }[],
): CommunityResult {
  // Пустой граф — ничего считать.
  if (nodes.length === 0) return { community: new Map(), count: 0, modularity: 0 };

  // Фикс-порядок: сортируем id (стабильная нумерация без зависимости от insertion-order Map/Set).
  const ids = nodes.map((n) => n.id).sort((a, b) => a.localeCompare(b));
  const n = ids.length;
  const idx = new Map<string, number>();
  for (let i = 0; i < n; i++) idx.set(ids[i], i);

  // Плоская смежность ПРОСТОГО неориентированного графа: каждая уникальная пара = ОДНО ребро веса 1.
  // Параллельные И реципрокные рёбра (A→B и B→A — типичные взаимные вики-ссылки vault) схлопываются в
  // одно, иначе взаимная пара тянулась бы к слиянию вдвое сильнее однонаправленной, а возвращаемая
  // модулярность (canonicalize считает unit-вес) рассинхронизировалась бы с оптимизируемым графом
  // (adversarial-ревью GRAPH-6, MAJOR). Петли и неизвестные id отбрасываем.
  const pair = new Map<string, number>(); // "min,max" → вес (всегда 1: простой граф)
  for (const e of edges) {
    const a = idx.get(e.source);
    const b = idx.get(e.target);
    if (a == null || b == null || a === b) continue; // неизвестный id или петля — игнор
    const key = a < b ? `${a},${b}` : `${b},${a}`;
    pair.set(key, 1);
  }

  // Базовый уровень. Если рёбер нет — каждый узел своё сообщество (без gain-цикла, без деления на 0).
  if (pair.size === 0) {
    return canonicalize(
      ids,
      ids.map((_, i) => i),
      n,
      edges,
      idx,
    );
  }

  const base = buildLevelFromPairs(n, pair);

  // node2comm[i] — сообщество исходного узла i (индекс) на текущем уровне иерархии.
  const node2comm = ids.map((_, i) => i);
  let level = base;

  // Многоуровневая агрегация. Жёсткий кап итераций (≤ n уровней) — страховка от зацикливания.
  for (let pass = 0; pass < n; pass++) {
    const comm = localMoving(level);
    // Если ни один узел не сменил сообщество — дальнейшая агрегация бессмысленна.
    let moved = false;
    for (let i = 0; i < level.n; i++) {
      if (comm[i] !== i) {
        moved = true;
        break;
      }
    }
    if (!moved) break;

    // Плотная перенумерация сообществ уровня по минимальному индексу-члену (детерминированно).
    const dense = densify(comm, level.n);
    // Протягиваем метку на исходные узлы (Map → .get, не bracket-индекс).
    for (let i = 0; i < n; i++) node2comm[i] = dense.map.get(comm[node2comm[i]])!;
    // Агрегируем супер-граф; если уровень схлопнулся в один узел — стоп.
    if (dense.count <= 1) break;
    level = aggregate(level, comm, dense);
  }

  return canonicalize(ids, node2comm, n, edges, idx);
}

/** Строит базовый уровень из схлопнутых рёбер (без петель: простой граф). */
function buildLevelFromPairs(n: number, pair: Map<string, number>): Level {
  const adj: number[][] = Array.from({ length: n }, () => []);
  const w: number[][] = Array.from({ length: n }, () => []);
  const deg = new Array<number>(n).fill(0);
  // Сортируем ключи, чтобы adj строился в детерминированном порядке (возрастание соседа).
  const keys = [...pair.keys()].sort((ka, kb) => {
    const [a1, b1] = ka.split(',').map(Number);
    const [a2, b2] = kb.split(',').map(Number);
    return a1 - a2 || b1 - b2;
  });
  for (const key of keys) {
    const [a, b] = key.split(',').map(Number);
    const weight = pair.get(key)!;
    adj[a].push(b);
    w[a].push(weight);
    adj[b].push(a);
    w[b].push(weight);
    deg[a] += weight;
    deg[b] += weight;
  }
  let m2 = 0;
  for (let i = 0; i < n; i++) m2 += deg[i];
  return { n, adj, w, self: new Array<number>(n).fill(0), deg, m2 };
}

/**
 * Фаза локального движения: каждый узел в порядке индекса пробует перейти в соседнее сообщество
 * с макс. приростом модулярности. Возвращает массив `comm[i]` (id сообщества = индекс представителя).
 * Tie-break: строгий прирост > best+EPS, при равенстве — меньший id сообщества.
 */
function localMoving(level: Level): number[] {
  const { n, adj, w, deg, m2 } = level;
  const EPS = 1e-12;
  const comm = new Array<number>(n);
  for (let i = 0; i < n; i++) comm[i] = i;
  // Σtot[c] — суммарная степень узлов сообщества c.
  const sigmaTot = deg.slice();
  const m = m2 / 2;

  let improved = true;
  let guard = 0;
  while (improved && guard < n + 8) {
    improved = false;
    guard++;
    for (let u = 0; u < n; u++) {
      const cu = comm[u];
      const ku = deg[u];
      // Снимаем u со своего сообщества.
      sigmaTot[cu] -= ku;
      comm[u] = -1;

      // Вес от u к каждому соседнему сообществу (петли не считаем — adj без них).
      const toComm = new Map<number, number>();
      const au = adj[u];
      const wu = w[u];
      for (let k = 0; k < au.length; k++) {
        const c = comm[au[k]];
        if (c === -1) continue; // сосед — это сам u (не бывает) или временно снятый: пропуск
        toComm.set(c, (toComm.get(c) ?? 0) + wu[k]);
      }

      // Кандидаты в детерминированном порядке (возрастание id сообщества). Базис — вернуться в cu.
      let best = cu;
      let bestGain = (toComm.get(cu) ?? 0) - (sigmaTot[cu] * ku) / (2 * m);
      const cands = [...toComm.keys()].sort((a, b) => a - b);
      for (const c of cands) {
        const gain = toComm.get(c)! - (sigmaTot[c] * ku) / (2 * m);
        if (gain > bestGain + EPS || (Math.abs(gain - bestGain) <= EPS && c < best)) {
          bestGain = gain;
          best = c;
        }
      }

      comm[u] = best;
      sigmaTot[best] += ku;
      if (best !== cu) improved = true;
    }
  }
  return comm;
}

/** Перенумеровывает метки сообществ в плотные индексы 0..k-1 по минимальному индексу-члену. */
function densify(comm: number[], n: number): { map: Map<number, number>; count: number } {
  // Для каждого сообщества — минимальный индекс его члена (детерминированный представитель).
  const minMember = new Map<number, number>();
  for (let i = 0; i < n; i++) {
    const c = comm[i];
    const cur = minMember.get(c);
    if (cur == null || i < cur) minMember.set(c, i);
  }
  // Сортируем сообщества по их минимальному члену → плотная нумерация.
  const order = [...minMember.entries()].sort((a, b) => a[1] - b[1]);
  const map = new Map<number, number>();
  order.forEach(([c], i) => map.set(c, i));
  return { map, count: order.length };
}

/** Агрегирует уровень в супер-граф: супер-узел = плотное сообщество. Веса целые → порядок-независимо. */
function aggregate(level: Level, comm: number[], dense: { map: Map<number, number>; count: number }): Level {
  const k = dense.count;
  const sc = (i: number) => dense.map.get(comm[i])!; // исходный индекс → супер-индекс
  const self = new Array<number>(k).fill(0);
  // Внешние супер-рёбра: ключ "min,max" супер-индексов → суммарный вес.
  const pair = new Map<string, number>();

  // Наследуем петли членов.
  for (let i = 0; i < level.n; i++) self[sc(i)] += level.self[i];

  // Каждое ориг-ребро (i<j) учитываем один раз: смотрим adj[i] для j>i.
  for (let i = 0; i < level.n; i++) {
    const ai = level.adj[i];
    const wi = level.w[i];
    for (let t = 0; t < ai.length; t++) {
      const j = ai[t];
      if (j <= i) continue; // ребро учтём со стороны меньшего индекса
      const weight = wi[t];
      const si = sc(i);
      const sj = sc(j);
      if (si === sj) {
        self[si] += weight; // внутреннее ребро → петля супер-узла
      } else {
        const key = si < sj ? `${si},${sj}` : `${sj},${si}`;
        pair.set(key, (pair.get(key) ?? 0) + weight);
      }
    }
  }

  // Собираем супер-уровень.
  const adj: number[][] = Array.from({ length: k }, () => []);
  const w: number[][] = Array.from({ length: k }, () => []);
  const deg = new Array<number>(k).fill(0);
  for (let s = 0; s < k; s++) deg[s] = 2 * self[s];
  const keys = [...pair.keys()].sort((ka, kb) => {
    const [a1, b1] = ka.split(',').map(Number);
    const [a2, b2] = kb.split(',').map(Number);
    return a1 - a2 || b1 - b2;
  });
  for (const key of keys) {
    const [a, b] = key.split(',').map(Number);
    const weight = pair.get(key)!;
    adj[a].push(b);
    w[a].push(weight);
    adj[b].push(a);
    w[b].push(weight);
    deg[a] += weight;
    deg[b] += weight;
  }
  let m2 = 0;
  for (let s = 0; s < k; s++) m2 += deg[s];
  return { n: k, adj, w, self, deg, m2 };
}

/**
 * Финальная канонизация: сообщества → плотные id по убыванию размера (0 = крупнейшее),
 * ничья — по лексикографически минимальному id-члену. Считает модулярность на ПЛОСКОМ графе
 * (текстбук: Q = Σ_c [ L_c/m − (deg_c/2m)² ]) — конвенция зафиксирована тестом (две треугольника ≈0.357).
 */
function canonicalize(
  ids: string[],
  node2comm: number[],
  n: number,
  edges: readonly { source: string; target: string }[],
  idx: Map<string, number>,
): CommunityResult {
  // Размер и минимальный id-член каждого сырого сообщества.
  const size = new Map<number, number>();
  const minId = new Map<number, string>();
  for (let i = 0; i < n; i++) {
    const c = node2comm[i];
    size.set(c, (size.get(c) ?? 0) + 1);
    const cur = minId.get(c);
    if (cur == null || ids[i].localeCompare(cur) < 0) minId.set(c, ids[i]);
  }
  const order = [...size.keys()].sort(
    (a, b) => size.get(b)! - size.get(a)! || minId.get(a)!.localeCompare(minId.get(b)!),
  );
  const rank = new Map<number, number>();
  order.forEach((c, i) => rank.set(c, i));

  const community = new Map<string, number>();
  for (let i = 0; i < n; i++) community.set(ids[i], rank.get(node2comm[i])!);

  // Модулярность на исходном (простом, unit-вес) графе.
  let m = 0;
  const Lc = new Map<number, number>(); // внутренние рёбра сообщества (вес 1 каждое)
  const degC = new Map<number, number>(); // сумма степеней
  const deg = new Array<number>(n).fill(0);
  const counted = new Set<string>();
  for (const e of edges) {
    const a = idx.get(e.source);
    const b = idx.get(e.target);
    if (a == null || b == null || a === b) continue;
    const key = a < b ? `${a},${b}` : `${b},${a}`;
    if (counted.has(key)) continue; // параллельные/реципрокные рёбра — одно (как и buildLevelFromPairs)
    counted.add(key);
    m++;
    deg[a]++;
    deg[b]++;
    const ca = community.get(ids[a])!;
    const cb = community.get(ids[b])!;
    if (ca === cb) Lc.set(ca, (Lc.get(ca) ?? 0) + 1);
  }
  for (let i = 0; i < n; i++) {
    const c = community.get(ids[i])!;
    degC.set(c, (degC.get(c) ?? 0) + deg[i]);
  }
  let modularity = 0;
  if (m > 0) {
    for (const c of order) {
      const l = Lc.get(c) ?? 0;
      const d = degC.get(c) ?? 0;
      modularity += l / m - (d / (2 * m)) ** 2;
    }
  }

  return { community, count: order.length, modularity };
}
