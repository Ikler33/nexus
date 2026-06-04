import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  type Simulation,
} from 'd3-force';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import type { FullGraph } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import {
  endpointId,
  kinSet,
  neighborSet,
  nodeRadius,
  type EdgeIds,
  type GraphLink,
  type GraphNodeDatum,
} from './graph-sim';
import './graph.css';

type Mode = 'local' | 'full';

/** Топ-N по связности для единого графа. Пан/зум-камера (отдельный срез) сделает большой граф удобным. */
const FULL_LIMIT = 600;
/** Логический размер сцены (SVG viewBox). */
const STAGE_W = 1000;
const STAGE_H = 680;

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

interface GraphState {
  nodes: GraphNodeDatum[];
  links: GraphLink[];
  edgeIds: EdgeIds[];
  activeId: string | null;
  total: number;
  truncated: boolean;
}

/**
 * Граф ссылок (ADR-004) на **d3-force** (как графы Obsidian-класса): forceManyBody (разлёт по площади),
 * forceLink (пружины), forceCenter (мягкое центрирование), forceCollide (узлы не наезжают). Drag через
 * `fx/fy`: тянем ноду — она пиннится к курсору, связанные подтягиваются с естественным сопротивлением
 * (чем больше связей/инерции — тем больше сопротивление). Рендер — SVG (вид/анимации из дизайна:
 * пульс/halo/kin/«поток»). Чистые помощники (подсветка, радиус) — `graph-sim.ts` (юнит-тесты);
 * раскладка/drag — d3 + визуальная проверка человеком.
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
  const [, tick] = useState(0); // ре-рендер на каждый tick d3 (позиции живут в узлах, d3 их мутирует)

  const simRef = useRef<Simulation<GraphNodeDatum, GraphLink> | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);

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
      const nodes: GraphNodeDatum[] = data.nodes.map((n) => ({
        id: String(n.id),
        title: n.title ?? basename(n.path),
        path: n.path,
        deg: deg[String(n.id)] ?? 0,
      }));
      const edgeIds: EdgeIds[] = data.edges.map((e) => ({
        source: String(e.source),
        target: String(e.target),
      }));
      const links: GraphLink[] = edgeIds.map((e) => ({ source: e.source, target: e.target }));
      const activeId = nodes.find((n) => n.path === center)?.id ?? null;
      const full = mode === 'full' ? (data as FullGraph) : null;
      setGraph({
        nodes,
        links,
        edgeIds,
        activeId,
        total: full ? full.totalFiles : nodes.length,
        truncated: full ? full.truncated : false,
      });
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, depth, center]);

  // ── d3-force симуляция на смену данных ──
  useEffect(() => {
    if (!graph) {
      simRef.current?.stop();
      simRef.current = null;
      return;
    }
    setLoading(true);
    const sim = forceSimulation<GraphNodeDatum, GraphLink>(graph.nodes)
      // Отталкивание масштабируется по степени: хабы (много связей) расталкивают сильнее, иначе их
      // стягивают в центр собственные рёбра. Лист ≈ -500, хаб (deg 20) ≈ -1300.
      .force(
        'charge',
        forceManyBody<GraphNodeDatum>()
          .strength((d) => -(500 + d.deg * 40))
          .distanceMax(900),
      )
      .force(
        'link',
        forceLink<GraphNodeDatum, GraphLink>(graph.links)
          .id((d) => d.id)
          .distance(115)
          .strength(0.45),
      )
      .force('center', forceCenter<GraphNodeDatum>(STAGE_W / 2, STAGE_H / 2).strength(0.04))
      .force(
        'collide',
        forceCollide<GraphNodeDatum>()
          .radius((d) => nodeRadius(d.deg) + 12)
          .iterations(2),
      )
      .on('tick', () => tick((v) => v + 1));
    sim.alpha(1).restart();
    simRef.current = sim;
    const timer = setTimeout(() => setLoading(false), 600);
    return () => {
      clearTimeout(timer);
      sim.stop();
    };
  }, [graph]);

  useEffect(
    () => () => {
      simRef.current?.stop();
      simRef.current = null;
    },
    [],
  );

  // ── drag: пиннуем ноду (fx/fy) + разогрев; связанные подтягиваются физикой с сопротивлением ──
  const toLocal = (e: { clientX: number; clientY: number }) => {
    const r = svgRef.current?.getBoundingClientRect();
    if (!r) return { x: 0, y: 0 };
    return {
      x: ((e.clientX - r.left) / r.width) * STAGE_W,
      y: ((e.clientY - r.top) / r.height) * STAGE_H,
    };
  };
  const onDown = useCallback(
    (node: GraphNodeDatum) => (e: React.MouseEvent) => {
      e.preventDefault();
      const sim = simRef.current;
      if (!sim) return;
      // Освобождаем ранее «закреплённые» ноды: при перетягивании другой (связанной) ноды прежние
      // снова включаются в физику — pin не навсегда (как в Obsidian).
      for (const other of sim.nodes()) {
        if (other.id !== node.id) {
          other.fx = null;
          other.fy = null;
        }
      }
      setDragId(node.id);
      sim.alphaTarget(0.3).restart();
      node.fx = node.x;
      node.fy = node.y;
      let moved = false;
      const move = (ev: MouseEvent) => {
        moved = true;
        const p = toLocal(ev);
        node.fx = p.x;
        node.fy = p.y;
      };
      const up = () => {
        sim.alphaTarget(0);
        setDragId(null);
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', up);
        if (moved) {
          // перетащили → нода ОСТАЁТСЯ там, где бросили (sticky, как в Obsidian): fx/fy НЕ сбрасываем,
          // соседи переселяются вокруг неё. Освобождение — только при следующем drag или новых данных.
        } else {
          // клик без движения → не закрепляем; открываем файл
          node.fx = null;
          node.fy = null;
          close();
          void openFile(node.path);
        }
      };
      window.addEventListener('mousemove', move);
      window.addEventListener('mouseup', up);
    },
    [close, openFile],
  );

  const focus = dragId ?? hover;
  const nbrs = useMemo(() => (graph ? neighborSet(graph.edgeIds, focus) : null), [graph, focus]);
  const kin = useMemo(
    () => (graph ? kinSet(graph.edgeIds, graph.activeId) : new Set<string>()),
    [graph],
  );

  const showCanvas = mode === 'full' || !!center;

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
            {t('graph.stat', { nodes: graph.nodes.length, edges: graph.edgeIds.length })}
          </span>
        )}
        <button
          className="graph-close"
          onClick={close}
          title={t('graph.close')}
          aria-label={t('graph.close')}
        >
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
              return graph.links.map((l, i) => {
                const s = l.source as GraphNodeDatum;
                const tt = l.target as GraphNodeDatum;
                if (s.x == null || s.y == null || tt.x == null || tt.y == null) return null;
                const sId = endpointId(l.source);
                const tId = endpointId(l.target);
                const active = nbrs ? nbrs.has(sId) && nbrs.has(tId) : true;
                const lit = dragId != null && (sId === dragId || tId === dragId);
                const flow =
                  !dragId && graph.activeId != null && (sId === graph.activeId || tId === graph.activeId);
                const fcls = flow ? ' flow f' + ((flowN++ % 4) + 1) : '';
                return (
                  <line
                    key={i}
                    x1={s.x}
                    y1={s.y}
                    x2={tt.x}
                    y2={tt.y}
                    className={'g-edge' + (active ? '' : ' dim') + (lit ? ' lit' : '') + fcls}
                  />
                );
              });
            })()}
            {graph.nodes.map((n) => {
              if (n.x == null || n.y == null) return null;
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
                  transform={`translate(${n.x},${n.y})`}
                  onMouseEnter={() => setHover(n.id)}
                  onMouseLeave={() => setHover(null)}
                  onMouseDown={onDown(n)}
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
