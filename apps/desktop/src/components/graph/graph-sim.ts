// graph-sim.ts — чистые помощники графа (без React/DOM): подсветка соседей/kin + радиус узла + типы.
// Раскладку и drag-физику делает d3-force (в GraphView) — индустриальный force-движок (как у
// Obsidian-класс графов): forceManyBody (разлёт) + forceLink (пружины) + forceCollide (без наложений)
// + drag через fx/fy (тянем ноду — пиннится, связанные подтягиваются с естественным сопротивлением).
// Здесь — только детерминированно-тестируемая логика; раскладку d3 тестировать не нужно.

import type { SimulationLinkDatum, SimulationNodeDatum } from 'd3-force';

/** Узел графа для d3-force (d3 домутирует `x/y/vx/vy/fx/fy` на месте). */
export interface GraphNodeDatum extends SimulationNodeDatum {
  id: string;
  title: string;
  path: string;
  deg: number;
  /** Теги заметки (без `#`) — цвет узла и фильтр-чипы. */
  tags: string[];
  /** Радиус кольца-гало для сироты в глобальном графе (макет: ring-притяжение + кламп). */
  ring?: number;
}

/** Стабильный оттенок тега: FNV-1a-хеш имени → hue 0..359 (один тег — один цвет везде). */
export function tagHue(tag: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < tag.length; i++) {
    h ^= tag.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0) % 360;
}

/**
 * Цвет узла по первому тегу — как `nodeColor` макета `graph.jsx`, но вместо хардкод-словаря
 * под демо-теги — хеш-палитра oklch (любой vault). Светлота/хрома — CSS-переменные
 * `--g-tag-l/--g-tag-c` (пер-тема, graph.css); без тегов — `null` (фолбэк из CSS).
 */
export function nodeColor(tags: string[]): string | null {
  if (tags.length === 0) return null;
  return `oklch(var(--g-tag-l, 0.55) var(--g-tag-c, 0.12) ${tagHue(tags[0])})`;
}

/**
 * Цвет узла по id сообщества (GRAPH-6, режим «Цвет: Сообщества»). Hue — золотой угол
 * (137.508°), даёт максимальный визуальный разнос соседних кластеров; та же oklch-семья и
 * CSS-переменные светлоты/хромы, что у `nodeColor` (пер-тема, без нового CSS). id<0 («нет
 * сообщества») → `null` (фолбэк из CSS), чтобы не путать с кластером 0 (крупнейшим).
 */
export function clusterColor(id: number): string | null {
  if (id < 0) return null;
  const hue = Math.round((id * 137.508) % 360);
  return `oklch(var(--g-tag-l, 0.55) var(--g-tag-c, 0.12) ${hue})`;
}

/** Топ-N тегов узлов графа по частоте (ничья — по алфавиту) — фильтр-чипы бара. */
export function topTags(nodes: readonly { tags: string[] }[], limit: number): string[] {
  const counts = new Map<string, number>();
  for (const n of nodes) for (const t of n.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, Math.max(0, limit))
    .map(([t]) => t);
}

/** Ребро d3-force: до init `source/target` — id-строки, после — объекты узлов. */
export interface GraphLink extends SimulationLinkDatum<GraphNodeDatum> {
  source: string | GraphNodeDatum;
  target: string | GraphNodeDatum;
}

/** Лёгкое id-ребро для логики подсветки (без d3-мутаций). */
export interface EdgeIds {
  source: string;
  target: string;
}

/** «Фокус + прямые соседи» — подсветка на hover/drag; `null`, если фокуса нет. */
export function neighborSet(edges: EdgeIds[], focus: string | null): Set<string> | null {
  if (!focus) return null;
  const s = new Set<string>([focus]);
  for (const e of edges) {
    if (e.source === focus) s.add(e.target);
    if (e.target === focus) s.add(e.source);
  }
  return s;
}

/** Прямые соседи активной ноты (для kin-колец), без неё самой. */
export function kinSet(edges: EdgeIds[], activeId: string | null): Set<string> {
  const s = new Set<string>();
  if (!activeId) return s;
  for (const e of edges) {
    if (e.source === activeId) s.add(e.target);
    if (e.target === activeId) s.add(e.source);
  }
  s.delete(activeId);
  return s;
}

/** Радиус узла по степени (формула макета graph.jsx): сирота — точка 3.5, дальше 5.5..15. */
export function nodeRadius(deg: number): number {
  if (deg === 0) return 3.5;
  return Math.max(5.5, Math.min(15, 5 + deg * 1.6));
}

/** id концов ребра (после d3-init `source/target` — объекты, до — строки). */
export function endpointId(end: string | GraphNodeDatum): string {
  return typeof end === 'string' ? end : end.id;
}

// ── GRAPH-1: параметры/формулы физики (общий источник правды для GraphView и тестов) ──
// Ресёрч-ретюн (Obsidian-когезия): сильное центрирование побеждает заряд, изоляты — аккуратное гало.
/** Кап степенного члена заряда: мега-хаб не становится «бомбой отталкивания». */
export const CHARGE_DEG_CAP = 8;
export const CHARGE_DEG_TERM = 30;
/** Сирота расталкивается слабо — рыхлое кольцо-гало, а не разлёт. */
export const CHARGE_ORPHAN_FACTOR = 0.12;
/** Конечная отсечка отталкивания (локализует раскладку — главный рычаг против разлёта). */
export const CHARGE_DISTANCE_MAX = 340;
/** Нижняя отсечка: совпавшие на старте узлы не получают почти-бесконечную силу (без fling-out). */
export const CHARGE_DISTANCE_MIN = 14;
/** Чуть больше трения дефолтных 0.4 — гасит овершут усиленных сил. */
export const VELOCITY_DECAY = 0.45;
/** Radial-сила кольца сирот (мягче прежних 0.08: сильная общая гравитация держит ядро). */
export const RADIAL_STRENGTH = 0.06;
/** Локальный режим мягче глобального (узлов мало). */
export const LOCAL_GRAVITY_FACTOR = 0.6;
/** Кап радиуса связных узлов в глобале (сейфнет: при сильной гравитации почти не срабатывает). */
export const CORE_MAX_FACTOR = 0.27;
/** Радиус кольца-гало сирот (GRAPH-1): держим чуть СНАРУЖИ ядра (~CORE_MAX_FACTOR), а не у края сцены —
 *  иначе при немногих сиротах они «разлетаются по углам» (жалоба владельца). 0.30 = плотный нимб у ядра. */
export const ORPHAN_RING_FACTOR = 0.3;
/** GRAPH-2: сколько тиков прогнать ГОЛОВЛЕСС (без отрисовки) до первого кадра — граф открывается уже
 *  СОБРАННЫМ, без видимого «прыжка» раскладки. После — короткое живое дотыхание до полной остановки. */
export const WARMUP_TICKS = 120;
const ORPHAN_BAND_LO = 0.78;
const ORPHAN_BAND_HI = 1.18;
const STAGE_MARGIN = 20;

/** Заряд отталкивания узла (отрицательный — d3: <0 = отталкивание). Степенной член капнут `min(deg,8)`;
 *  сирота — слабый фактор `0.12`. `repel` — пользовательский слайдер базы. */
export function chargeStrength(d: { deg: number; ring?: number }, repel: number): number {
  return -(
    repel * (d.ring ? CHARGE_ORPHAN_FACTOR : 1) +
    Math.min(d.deg, CHARGE_DEG_CAP) * CHARGE_DEG_TERM
  );
}

/** Сила центр-гравитации (forceX/Y): сироты — 0 (их держит кольцо); локальный режim — 0.6× глобального. */
export function gravityStrength(d: { ring?: number }, gravity: number, isFull: boolean): number {
  return d.ring ? 0 : isFull ? gravity : gravity * LOCAL_GRAVITY_FACTOR;
}

/** Клампы позиции узла (мутирует `x/y`): сирота — в полосе кольца `[0.78R, 1.18R]`; связный глобального —
 *  не дальше `coreMax` от центра (никогда не в углы); общие границы сцены. Сейфнет — основную работу
 *  делают силы. Перетаскиваемую (fx≠null) вызывающий пропускает сам. */
export function clampNodePosition(
  d: GraphNodeDatum,
  cx: number,
  cy: number,
  w: number,
  h: number,
  isFull: boolean,
): void {
  const dx = (d.x ?? cx) - cx;
  const dy = (d.y ?? cy) - cy;
  const dist = Math.hypot(dx, dy) || 0.01;
  if (d.ring) {
    const lo = d.ring * ORPHAN_BAND_LO;
    const hi = d.ring * ORPHAN_BAND_HI;
    if (dist < lo || dist > hi) {
      const k = (dist < lo ? lo : hi) / dist;
      d.x = cx + dx * k;
      d.y = cy + dy * k;
    }
  } else if (isFull) {
    const coreMax = Math.min(w, h) * CORE_MAX_FACTOR;
    if (dist > coreMax) {
      const k = coreMax / dist;
      d.x = cx + dx * k;
      d.y = cy + dy * k;
    }
  }
  d.x = Math.max(STAGE_MARGIN, Math.min(w - STAGE_MARGIN, d.x ?? cx));
  d.y = Math.max(STAGE_MARGIN, Math.min(h - STAGE_MARGIN, d.y ?? cy));
}
