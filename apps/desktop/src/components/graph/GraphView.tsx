import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import type { FullGraph } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import {
  forceStep,
  kinSet,
  neighborSet,
  nodeRadius,
  seedPositions,
  STAGE_H,
  STAGE_W,
  type Positions,
  type SimEdge,
  type SimNode,
} from './graph-sim';
import './graph.css';

type Mode = 'local' | 'full';

/** Топ-N по связности для единого графа (SVG-сим тянет с запасом; о неполноте — баннер-warning). */
const FULL_LIMIT = 600;

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

interface GraphState {
  nodes: SimNode[];
  edges: SimEdge[];
  activeId: string | null;
  total: number;
  truncated: boolean;
}

/**
 * Граф ссылок (ADR-004) — кастомный SVG force-directed (по дизайну `handoff/graph.jsx`): drag
 * (соседи подтягиваются пружинами), hover-подсветка связанных, активная нота с пульсом/кольцом,
 * kin-кольца соседей, «текущие» рёбра с анимацией. Режимы: локальный N-hop (глубина 1–3, считает
 * Rust) и единый (топ-N по связности). Раскладка — лёгкая физика на main-thread (узлов мало: N-hop
 * либо топ-600). Чистая математика — в `graph-sim.ts` (юнит-тесты); визуал/drag — проверка человеком.
 */
export default function GraphView() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeGraph);
  const center = useWorkspaceStore(activePath);
  const openFile = useWorkspaceStore((s) => s.openFile);

  const [mode, setMode] = useState<Mode>('local');
  const [depth, setDepth] = useState(2);
  const [graph, setGraph] = useState<GraphState | null>(null);
  const [loading, setLoading] = useState(true);
  const [hover, setHover] = useState<string | null>(null);
  const [dragId, setDragId] = useState<string | null>(null);
  const [, tick] = useState(0); // ре-рендер из rAF-петли (позиции живут в posRef, вне React-стейта)

  const posRef = useRef<Positions>({});
  const dragRef = useRef<string | null>(null);
  const rafRef = useRef<number | null>(null);
  const alphaRef = useRef(0);
  const runningRef = useRef(false);
  const movedRef = useRef(false);
  const svgRef = useRef<SVGSVGElement>(null);
  const graphRef = useRef<GraphState | null>(null);
  graphRef.current = graph;

  // ── петля симуляции (persistent, re-heatable) ──
  const step = useCallback(() => {
    const g = graphRef.current;
    if (!g) return;
    const ids = g.nodes.map((n) => n.id);
    alphaRef.current = forceStep(posRef.current, ids, g.edges, alphaRef.current, dragRef.current);
    tick((v) => v + 1);
  }, []);
  const loop = useCallback(() => {
    step();
    if (alphaRef.current > 0.04 || dragRef.current) {
      rafRef.current = requestAnimationFrame(loop);
    } else {
      runningRef.current = false;
    }
  }, [step]);
  const kick = useCallback(
    (a: number) => {
      alphaRef.current = Math.max(alphaRef.current, a);
      if (!runningRef.current) {
        runningRef.current = true;
        rafRef.current = requestAnimationFrame(loop);
      }
    },
    [loop],
  );

  useEffect(
    () => () => {
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
      runningRef.current = false;
    },
    [],
  );

  // ── загрузка данных: локальный N-hop считает Rust (глубина = hops); единый — топ-N ──
  useEffect(() => {
    if (mode === 'local' && !center) {
      setGraph(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void (async () => {
      const data =
        mode === 'full'
          ? await tauriApi.graph.getFullGraph(FULL_LIMIT)
          : await tauriApi.graph.getLocalGraph(center as string, depth);
      if (cancelled) return;
      const deg: Record<string, number> = {};
      for (const e of data.edges) {
        deg[String(e.source)] = (deg[String(e.source)] ?? 0) + 1;
        deg[String(e.target)] = (deg[String(e.target)] ?? 0) + 1;
      }
      const nodes: SimNode[] = data.nodes.map((n) => ({
        id: String(n.id),
        title: n.title ?? basename(n.path),
        path: n.path,
        deg: deg[String(n.id)] ?? 0,
      }));
      const edges: SimEdge[] = data.edges.map((e) => ({
        a: String(e.source),
        b: String(e.target),
      }));
      const activeId = nodes.find((n) => n.path === center)?.id ?? null;
      const full = mode === 'full' ? (data as FullGraph) : null;
      const total = full ? full.totalFiles : nodes.length;
      const truncated = full ? full.truncated : false;
      setGraph({ nodes, edges, activeId, total, truncated });
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, depth, center]);

  // ── re-seed позиций + разогрев симуляции на смену данных ──
  useEffect(() => {
    if (!graph) return;
    const ids = new Set(graph.nodes.map((n) => n.id));
    for (const id of Object.keys(posRef.current)) {
      if (!ids.has(id)) delete posRef.current[id];
    }
    seedPositions(posRef.current, graph.nodes.map((n) => n.id));
    setLoading(true);
    kick(1);
    const timer = setTimeout(() => setLoading(false), 700);
    return () => clearTimeout(timer);
  }, [graph, kick]);

  // ── drag: пиннуем узел к курсору, соседи подтягиваются пружинами (сим держим «тёплым») ──
  const toLocal = (e: { clientX: number; clientY: number }) => {
    const r = svgRef.current?.getBoundingClientRect();
    if (!r) return { x: 0, y: 0 };
    return {
      x: ((e.clientX - r.left) / r.width) * STAGE_W,
      y: ((e.clientY - r.top) / r.height) * STAGE_H,
    };
  };
  const onDown = useCallback(
    (id: string) => (e: React.MouseEvent) => {
      e.preventDefault();
      dragRef.current = id;
      setDragId(id);
      movedRef.current = false;
      const N0 = posRef.current[id];
      const start = toLocal(e);
      const off = N0 ? { x: N0.x - start.x, y: N0.y - start.y } : { x: 0, y: 0 };
      kick(0.7);
      const move = (ev: MouseEvent) => {
        movedRef.current = true;
        const p = toLocal(ev);
        const N = posRef.current[id];
        if (N) {
          N.x = Math.max(40, Math.min(STAGE_W - 40, p.x + off.x));
          N.y = Math.max(36, Math.min(STAGE_H - 36, p.y + off.y));
          N.vx = 0;
          N.vy = 0;
        }
        kick(0.5);
      };
      const up = () => {
        dragRef.current = null;
        setDragId(null);
        kick(0.35);
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', up);
      };
      window.addEventListener('mousemove', move);
      window.addEventListener('mouseup', up);
    },
    [kick],
  );

  const focus = dragId ?? hover;
  const nbrs = useMemo(() => (graph ? neighborSet(graph.edges, focus) : null), [graph, focus]);
  const kin = useMemo(
    () => (graph ? kinSet(graph.edges, graph.activeId) : new Set<string>()),
    [graph],
  );

  const showCanvas = mode === 'full' || !!center;
  const P = posRef.current;

  return (
    <div className="graph-view">
      <div className="graph-bar">
        <div className="seg">
          <button
            className={'seg-btn' + (mode === 'local' ? ' on' : '')}
            onClick={() => setMode('local')}
          >
            {t('graph.modeLocal')}
          </button>
          <button
            className={'seg-btn' + (mode === 'full' ? ' on' : '')}
            onClick={() => setMode('full')}
          >
            {t('graph.modeFull')}
          </button>
        </div>
        {mode === 'local' && (
          <label className="graph-depth">
            {t('graph.depth')}
            <input
              type="range"
              min={1}
              max={3}
              value={depth}
              onChange={(e) => setDepth(+e.target.value)}
            />
            <span className="graph-mono">{depth}</span>
          </label>
        )}
        <div className="graph-spacer" />
        {graph && (
          <span className="graph-stat graph-mono">
            {t('graph.stat', { nodes: graph.nodes.length, edges: graph.edges.length })}
          </span>
        )}
        <button className="graph-close" onClick={close} title={t('graph.close')} aria-label={t('graph.close')}>
          <X size={16} />
        </button>
      </div>

      {mode === 'full' && graph?.truncated && (
        <div className="graph-warn">
          {t('graph.truncated', { shown: graph.nodes.length, total: graph.total })}
        </div>
      )}

      <div className="graph-stage">
        {!showCanvas && <div className="graph-loading">{t('graph.empty')}</div>}
        {showCanvas && loading && <div className="graph-loading">{t('graph.loading')}</div>}
        {showCanvas && graph && (
          <svg
            ref={svgRef}
            className="graph-svg"
            viewBox={`0 0 ${STAGE_W} ${STAGE_H}`}
            preserveAspectRatio="xMidYMid meet"
          >
            {(() => {
              let flowN = 0;
              return graph.edges.map((e, i) => {
                const A = P[e.a];
                const B = P[e.b];
                if (!A || !B) return null;
                const active = nbrs ? nbrs.has(e.a) && nbrs.has(e.b) : true;
                const lit = dragId != null && (e.a === dragId || e.b === dragId);
                const flow =
                  !dragId && graph.activeId != null && (e.a === graph.activeId || e.b === graph.activeId);
                const fcls = flow ? ' flow f' + ((flowN++ % 4) + 1) : '';
                return (
                  <line
                    key={i}
                    x1={A.x}
                    y1={A.y}
                    x2={B.x}
                    y2={B.y}
                    className={'g-edge' + (active ? '' : ' dim') + (lit ? ' lit' : '') + fcls}
                  />
                );
              });
            })()}
            {graph.nodes.map((n) => {
              const N = P[n.id];
              if (!N) return null;
              const isActive = n.id === graph.activeId;
              const r = nodeRadius(n.deg);
              const faded = nbrs != null && !nbrs.has(n.id);
              const isKin = !isActive && kin.has(n.id);
              return (
                <g
                  key={n.id}
                  className={
                    'g-node' +
                    (faded ? ' faded' : '') +
                    (isActive ? ' active' : '') +
                    (isKin ? ' kin' : '') +
                    (dragId === n.id ? ' grabbing' : '')
                  }
                  transform={`translate(${N.x},${N.y})`}
                  onMouseEnter={() => setHover(n.id)}
                  onMouseLeave={() => setHover(null)}
                  onMouseDown={onDown(n.id)}
                  onClick={() => {
                    if (movedRef.current) {
                      movedRef.current = false;
                      return;
                    }
                    close();
                    void openFile(n.path);
                  }}
                >
                  {isActive && <circle r={r + 6} className="g-pulse" />}
                  {isActive && <circle r={r + 6} className="g-ripple" />}
                  <circle r={r} className="g-dot" />
                  {isActive && <circle r={r + 5} className="g-ring" />}
                  {isKin && <circle r={r + 3.5} className="g-kinring" />}
                  <text y={r + 14} className="g-label" textAnchor="middle">
                    {n.title}
                  </text>
                </g>
              );
            })}
          </svg>
        )}
      </div>
    </div>
  );
}
