import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type ForceCollide,
  type ForceLink,
  type ForceManyBody,
  type ForceX,
  type ForceY,
  type Simulation,
} from 'd3-force';
import { Maximize2, Minus, Plus, Settings, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import type { FullGraph } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../chrome/BrandThinking';
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

/** Топ-N по связности для единого графа. */
const FULL_LIMIT = 600;
/** Логический размер сцены (SVG viewBox). */
const STAGE_W = 1000;
const STAGE_H = 680;

/** Камера пан/зума (DP-6/v2c): прямоугольник viewBox. */
interface Camera {
  x: number;
  y: number;
  w: number;
  h: number;
}
const HOME_CAM: Camera = { x: 0, y: 0, w: STAGE_W, h: STAGE_H };
/** Пределы зума: от ×8 приближения до ×3 отдаления. */
const MIN_W = STAGE_W / 8;
const MAX_W = STAGE_W * 3;

/** Зум вокруг точки (лог. координаты сцены): factor < 1 — приближение. */
function zoomCamera(cam: Camera, factor: number, cx: number, cy: number): Camera {
  const w = Math.min(MAX_W, Math.max(MIN_W, cam.w * factor));
  const k = w / cam.w;
  const h = cam.h * k;
  return { x: cx - (cx - cam.x) * k, y: cy - (cy - cam.y) * k, w, h };
}

/** Камера под все узлы с полем (авто-fit). */
function fitCamera(nodes: GraphNodeDatum[]): Camera {
  const xs = nodes.map((n) => n.x).filter((v): v is number => v != null);
  const ys = nodes.map((n) => n.y).filter((v): v is number => v != null);
  if (xs.length === 0) return HOME_CAM;
  const pad = 70;
  const minX = Math.min(...xs) - pad;
  const maxX = Math.max(...xs) + pad;
  const minY = Math.min(...ys) - pad;
  const maxY = Math.max(...ys) + pad;
  // Сохраняем аспект сцены, накрывая bounding box целиком.
  let w = maxX - minX;
  let h = maxY - minY;
  const aspect = STAGE_W / STAGE_H;
  if (w / h > aspect) h = w / aspect;
  else w = h * aspect;
  w = Math.min(MAX_W, Math.max(MIN_W, w));
  h = (w / STAGE_W) * STAGE_H;
  return { x: (minX + maxX) / 2 - w / 2, y: (minY + maxY) / 2 - h / 2, w, h };
}

/** Параметры физики — пользователь крутит вживую (как ⚙️ в Obsidian); сохраняются в localStorage. */
interface GraphSettings {
  repel: number; // база отталкивания: заряд = -(repel + deg*30); выше = сильнее разлёт
  linkDist: number; // длина пружин-связей
  gravity: number; // притяжение к центру (forceX/Y): выше = плотнее, ниже = разлёт
  sizeScale: number; // множитель радиуса узла
}
const DEFAULT_SETTINGS: GraphSettings = { repel: 420, linkDist: 110, gravity: 0.05, sizeScale: 1 };
const SETTINGS_KEY = 'nexus.graph.settings.v1';
function loadSettings(): GraphSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) return { ...DEFAULT_SETTINGS, ...(JSON.parse(raw) as Partial<GraphSettings>) };
  } catch {
    /* нет localStorage / битый JSON → дефолты */
  }
  return DEFAULT_SETTINGS;
}

/** Строка-слайдер панели настроек графа. */
function SettingRow(props: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange: (v: number) => void;
  fmt?: (v: number) => string;
}) {
  return (
    <label className="graph-row">
      <span className="graph-row-label">{props.label}</span>
      <input
        type="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={props.value}
        onChange={(e) => props.onChange(+e.target.value)}
      />
      <span className="graph-row-val graph-mono">
        {props.fmt ? props.fmt(props.value) : props.value}
      </span>
    </label>
  );
}

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
  const [settings, setSettings] = useState<GraphSettings>(loadSettings);
  const [showSettings, setShowSettings] = useState(false);
  const [cam, setCam] = useState<Camera>(HOME_CAM);
  const [, tick] = useState(0); // ре-рендер на каждый tick d3 (позиции живут в узлах, d3 их мутирует)

  const simRef = useRef<Simulation<GraphNodeDatum, GraphLink> | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  // ссылки на силы — чтобы менять их вживую из слайдеров без пересоздания сим (позиции сохраняются)
  const settingsRef = useRef(settings);
  const chargeRef = useRef<ForceManyBody<GraphNodeDatum> | null>(null);
  const linkRef = useRef<ForceLink<GraphNodeDatum, GraphLink> | null>(null);
  const gravXRef = useRef<ForceX<GraphNodeDatum> | null>(null);
  const gravYRef = useRef<ForceY<GraphNodeDatum> | null>(null);
  const collideRef = useRef<ForceCollide<GraphNodeDatum> | null>(null);

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
    const s = settingsRef.current;
    // Отталкивание масштабируется по степени: хабы расталкивают сильнее.
    const charge = forceManyBody<GraphNodeDatum>()
      .strength((d) => -(s.repel + d.deg * 30))
      .distanceMax(950);
    // ВАЖНО: НЕ задаём link.strength → d3 авто-масштабирует обратно степени (рёбра к хабам слабее).
    // Это каноничный механизм d3, который раздвигает хабы; жёсткий uniform-strength их стягивал.
    const link = forceLink<GraphNodeDatum, GraphLink>(graph.links)
      .id((d) => d.id)
      .distance(s.linkDist);
    // Притяжение к центру через forceX/Y (а не forceCenter): это «гравитация» — выше плотнее, ниже разлёт.
    const gravX = forceX<GraphNodeDatum>(STAGE_W / 2).strength(s.gravity);
    const gravY = forceY<GraphNodeDatum>(STAGE_H / 2).strength(s.gravity);
    const collide = forceCollide<GraphNodeDatum>()
      .radius((d) => nodeRadius(d.deg) * s.sizeScale + 12)
      .iterations(2);
    const sim = forceSimulation<GraphNodeDatum, GraphLink>(graph.nodes)
      .force('charge', charge)
      .force('link', link)
      .force('x', gravX)
      .force('y', gravY)
      .force('collide', collide)
      .on('tick', () => tick((v) => v + 1));
    chargeRef.current = charge;
    linkRef.current = link;
    gravXRef.current = gravX;
    gravYRef.current = gravY;
    collideRef.current = collide;
    sim.alpha(1).restart();
    simRef.current = sim;
    // По остыванию раскладки — авто-fit камеры (v2c) и снятие лоадера.
    const timer = setTimeout(() => {
      setLoading(false);
      setCam(fitCamera(sim.nodes()));
    }, 600);
    return () => {
      clearTimeout(timer);
      sim.stop();
    };
  }, [graph]);

  // ── живое применение настроек физики: мутируем силы существующей сим (позиции сохраняются) ──
  useEffect(() => {
    settingsRef.current = settings;
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    } catch {
      /* нет localStorage → просто не сохраняем */
    }
    if (!simRef.current) return;
    chargeRef.current?.strength((d) => -(settings.repel + d.deg * 30));
    linkRef.current?.distance(settings.linkDist);
    gravXRef.current?.strength(settings.gravity);
    gravYRef.current?.strength(settings.gravity);
    collideRef.current?.radius((d) => nodeRadius(d.deg) * settings.sizeScale + 12);
    simRef.current.alpha(0.5).restart();
  }, [settings]);

  useEffect(
    () => () => {
      simRef.current?.stop();
      simRef.current = null;
    },
    [],
  );

  // ── камера (DP-6/v2c): координаты курсора → логические координаты сцены с учётом viewBox ──
  const camRef = useRef(cam);
  camRef.current = cam;
  const toLocal = (e: { clientX: number; clientY: number }) => {
    const r = svgRef.current?.getBoundingClientRect();
    const c = camRef.current;
    if (!r) return { x: 0, y: 0 };
    return {
      x: c.x + ((e.clientX - r.left) / r.width) * c.w,
      y: c.y + ((e.clientY - r.top) / r.height) * c.h,
    };
  };

  // Wheel-зум вокруг курсора (passive: false не нужен — onWheel React достаточно для viewBox).
  const onWheel = (e: React.WheelEvent) => {
    const p = toLocal(e);
    setCam((c) => zoomCamera(c, Math.exp(e.deltaY * 0.0015), p.x, p.y));
  };

  // Пан по пустому фону (mousedown мимо нод; ноды гасят всплытие в onDown).
  const onStagePan = (e: React.MouseEvent) => {
    e.preventDefault();
    const start = { x: e.clientX, y: e.clientY };
    const startCam = camRef.current;
    const r = svgRef.current?.getBoundingClientRect();
    if (!r) return;
    const move = (ev: MouseEvent) => {
      const dx = ((ev.clientX - start.x) / r.width) * startCam.w;
      const dy = ((ev.clientY - start.y) / r.height) * startCam.h;
      setCam({ ...startCam, x: startCam.x - dx, y: startCam.y - dy });
    };
    const up = () => {
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
  };

  const fit = useCallback(() => {
    const nodes = simRef.current?.nodes() ?? [];
    setCam(fitCamera(nodes));
  }, []);

  // ── drag: пиннуем ноду (fx/fy) + разогрев; связанные подтягиваются физикой с сопротивлением ──
  const onDown = useCallback(
    (node: GraphNodeDatum) => (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation(); // не запускать пан фона (DP-6)
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
          className={'graph-close' + (showSettings ? ' on' : '')}
          onClick={() => setShowSettings((v) => !v)}
          title={t('graph.settings')}
          aria-label={t('graph.settings')}
        >
          <Settings size={16} />
        </button>
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
        {showCanvas && loading && (
          <div className="graph-loading graph-thinking">
            <BrandThinking size={28} />
            <span className="mt-label">{t('graph.loading')}</span>
          </div>
        )}
        {showCanvas && graph && (
          <svg
            ref={svgRef}
            className="graph-svg"
            viewBox={`${cam.x} ${cam.y} ${cam.w} ${cam.h}`}
            preserveAspectRatio="xMidYMid meet"
            onWheel={onWheel}
            onMouseDown={onStagePan}
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
              const r = nodeRadius(n.deg) * settings.sizeScale;
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

        {showCanvas && graph && (
          <div className="graph-zoom">
            <button
              className="gz-btn"
              onClick={() => setCam((c) => zoomCamera(c, 0.8, c.x + c.w / 2, c.y + c.h / 2))}
              title={t('graph.zoomIn')}
              aria-label={t('graph.zoomIn')}
            >
              <Plus size={15} />
            </button>
            <button
              className="gz-btn"
              onClick={() => setCam((c) => zoomCamera(c, 1.25, c.x + c.w / 2, c.y + c.h / 2))}
              title={t('graph.zoomOut')}
              aria-label={t('graph.zoomOut')}
            >
              <Minus size={15} />
            </button>
            <button
              className="gz-btn gz-fit"
              onClick={fit}
              title={t('graph.fit')}
              aria-label={t('graph.fit')}
            >
              <Maximize2 size={14} />
            </button>
          </div>
        )}

        {showSettings && (
          <div className="graph-settings">
            <div className="graph-settings-head">
              <span>{t('graph.settings')}</span>
              <button className="graph-reset" onClick={() => setSettings(DEFAULT_SETTINGS)}>
                {t('graph.reset')}
              </button>
            </div>
            <SettingRow
              label={t('graph.repel')}
              min={100}
              max={900}
              step={20}
              value={settings.repel}
              onChange={(v) => setSettings((s) => ({ ...s, repel: v }))}
            />
            <SettingRow
              label={t('graph.linkDist')}
              min={40}
              max={240}
              step={5}
              value={settings.linkDist}
              onChange={(v) => setSettings((s) => ({ ...s, linkDist: v }))}
            />
            <SettingRow
              label={t('graph.gravity')}
              min={0}
              max={0.25}
              step={0.01}
              value={settings.gravity}
              fmt={(v) => v.toFixed(2)}
              onChange={(v) => setSettings((s) => ({ ...s, gravity: v }))}
            />
            <SettingRow
              label={t('graph.nodeSize')}
              min={0.6}
              max={2}
              step={0.1}
              value={settings.sizeScale}
              fmt={(v) => v.toFixed(1) + '×'}
              onChange={(v) => setSettings((s) => ({ ...s, sizeScale: v }))}
            />
          </div>
        )}
      </div>
    </div>
  );
}
