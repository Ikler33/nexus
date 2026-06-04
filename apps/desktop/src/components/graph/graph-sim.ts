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
