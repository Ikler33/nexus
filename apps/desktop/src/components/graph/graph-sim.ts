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

/** Радиус узла по степени связности: sqrt-шкала (чёткая градация хабов), клампы 5..28. */
export function nodeRadius(deg: number): number {
  return Math.max(5, Math.min(28, 5 + Math.sqrt(deg) * 4.2));
}

/** id концов ребра (после d3-init `source/target` — объекты, до — строки). */
export function endpointId(end: string | GraphNodeDatum): string {
  return typeof end === 'string' ? end : end.id;
}
