import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceRadial,
  forceSimulation,
  forceX,
  forceY,
  type Force,
  type ForceCollide,
  type ForceLink,
  type ForceManyBody,
  type ForceRadial,
  type ForceX,
  type ForceY,
  type Simulation,
} from 'd3-force';
import { Link2, Maximize2, Minus, Plus, Settings, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi } from '../../lib/tauri-api';
import type { FullGraph, LinkSuggestion } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { BrandThinking } from '../chrome/BrandThinking';
import {
  CHARGE_DISTANCE_MAX,
  CHARGE_DISTANCE_MIN,
  chargeStrength,
  clampNodePosition,
  endpointId,
  gravityStrength,
  kinSet,
  neighborSet,
  nodeColor,
  nodeRadius,
  ORPHAN_RING_FACTOR,
  RADIAL_STRENGTH,
  topTags,
  VELOCITY_DECAY,
  type EdgeIds,
  type GraphLink,
  type GraphNodeDatum,
} from './graph-sim';
import './graph.css';

type Mode = 'local' | 'full';

/** Топ-N по связности для единого графа. */
const FULL_LIMIT = 600;
/** Сколько тег-чипов показываем в баре (макет graph.jsx: slice(0, 8)). */
const TAG_CHIP_LIMIT = 8;
/** Логические сцены макета graph.jsx: глобальный обзор заметно крупнее локального. */
const STAGE = {
  local: { w: 900, h: 620 },
  full: { w: 1500, h: 1300 },
} as const;

/** Камера пан/зума (DP-6/v2c): прямоугольник viewBox. */
interface Camera {
  x: number;
  y: number;
  w: number;
  h: number;
}
/** Пределы зума макета: scale 0.25…4. */
const MIN_SCALE = 0.25;
const MAX_SCALE = 4;

/** Зум вокруг точки (лог. координаты сцены): factor < 1 — приближение. */
function zoomCamera(cam: Camera, factor: number, cx: number, cy: number, stageW: number): Camera {
  const w = Math.min(stageW / MIN_SCALE, Math.max(stageW / MAX_SCALE, cam.w * factor));
  const k = w / cam.w;
  const h = cam.h * k;
  return { x: cx - (cx - cam.x) * k, y: cy - (cy - cam.y) * k, w, h };
}

/** Камера под все узлы с полем (авто-fit). */
function fitCamera(nodes: GraphNodeDatum[], stage: { w: number; h: number }): Camera {
  const home = { x: 0, y: 0, w: stage.w, h: stage.h };
  const xs = nodes.map((n) => n.x).filter((v): v is number => v != null);
  const ys = nodes.map((n) => n.y).filter((v): v is number => v != null);
  if (xs.length === 0) return home;
  const pad = 70;
  const minX = Math.min(...xs) - pad;
  const maxX = Math.max(...xs) + pad;
  const minY = Math.min(...ys) - pad;
  const maxY = Math.max(...ys) + pad;
  // Сохраняем аспект сцены, накрывая bounding box целиком.
  let w = maxX - minX;
  let h = maxY - minY;
  const aspect = stage.w / stage.h;
  if (w / h > aspect) h = w / aspect;
  else w = h * aspect;
  w = Math.min(stage.w / MIN_SCALE, Math.max(stage.w / MAX_SCALE, w));
  h = (w / stage.w) * stage.h;
  return { x: (minX + maxX) / 2 - w / 2, y: (minY + maxY) / 2 - h / 2, w, h };
}

/** Параметры физики — пользователь крутит вживую (как ⚙️ в Obsidian); сохраняются в localStorage. */
interface GraphSettings {
  repel: number; // база отталкивания: заряд = -(repel + deg*30); выше = сильнее разлёт
  linkDist: number; // длина пружин-связей
  gravity: number; // притяжение к центру (forceX/Y): выше = плотнее, ниже = разлёт
  sizeScale: number; // множитель радиуса узла
  group: boolean; // группировка по тегам (макет gs-toggle): общий центроид на первый тег
}
// GRAPH-1 (ресёрч-ретюн физики): дефолты подобраны под когезию «как Obsidian» — сильное центрирование
// побеждает заряд, узлы держатся компактным созвездием, изоляты — аккуратное гало (не разлёт по углам).
// gravity = сила центр-притяжения (глоб.; лок. берёт 0.6×); раньше 0.012 было в ~7× слабее заряда → разлёт.
const DEFAULT_SETTINGS: GraphSettings = {
  repel: 300,
  linkDist: 46,
  gravity: 0.085,
  sizeScale: 1,
  group: false,
};
// v3 (GRAPH-1): ретюн физики не должен перекрываться старым персистом v1/v2 (иначе сохранённый разлёт).
const SETTINGS_KEY = 'nexus.graph.settings.v3';
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
  return path.slice(path.lastIndexOf('/') + 1).replace(/\.md$/, '');
}

interface GraphState {
  nodes: GraphNodeDatum[];
  links: GraphLink[];
  edgeIds: EdgeIds[];
  activeId: string | null;
  total: number;
  truncated: boolean;
  /** Сцена, под которую посеяны позиции (full крупнее local — макет). */
  stage: { w: number; h: number };
  isFull: boolean;
}

/** Поповер изолированной заметки (макет orphan-pop): инфо → AI-предложение связи. */
interface OrphanPop {
  path: string;
  x: number;
  y: number;
  phase: 'info' | 'thinking' | 'done';
  pick?: LinkSuggestion | null;
}

/** Детерминированный LCG (как в макете) — раскладка сирот стабильна между открытиями. */
function makeRnd(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return s / 0x7fffffff;
  };
}

/**
 * Граф ссылок (ADR-004) на **d3-force**, вид и физика — по макету `graph.jsx`: компактные
 * «созвездия» (короткие пружины, мягкая гравитация), сироты глобального графа — гало мелких
 * точек на кольце (radial-сила + жёсткий кламп полосы), связанные узлы не покидают ядро
 * (coreMax-кламп), сим никогда не замерзает полностью («дыхание» alphaTarget). Лейблы — только
 * у активной/hover-ноды и на среднем зуме (как Obsidian). Drag через `fx/fy`, рендер — SVG.
 * Чистые помощники — `graph-sim.ts` (юнит-тесты).
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
  const [tagFilter, setTagFilter] = useState<string | null>(null);
  const [settings, setSettings] = useState<GraphSettings>(loadSettings);
  const [showSettings, setShowSettings] = useState(false);
  const [orphanPop, setOrphanPop] = useState<OrphanPop | null>(null);
  const [cam, setCam] = useState<Camera>({ x: 0, y: 0, ...{ w: STAGE.local.w, h: STAGE.local.h } });
  const [, tick] = useState(0); // ре-рендер на каждый tick d3 (позиции живут в узлах, d3 их мутирует)

  const simRef = useRef<Simulation<GraphNodeDatum, GraphLink> | null>(null);
  // Для рендер-троттла «дыхания»: тик-клоужер читает drag через ref (state туда не попадает).
  const dragRef = useRef<string | null>(null);
  dragRef.current = dragId;
  const svgRef = useRef<SVGSVGElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  // ссылки на силы — чтобы менять их вживую из слайдеров без пересоздания сим (позиции сохраняются)
  const settingsRef = useRef(settings);
  const chargeRef = useRef<ForceManyBody<GraphNodeDatum> | null>(null);
  const linkRef = useRef<ForceLink<GraphNodeDatum, GraphLink> | null>(null);
  const gravXRef = useRef<ForceX<GraphNodeDatum> | null>(null);
  const gravYRef = useRef<ForceY<GraphNodeDatum> | null>(null);
  const radialRef = useRef<ForceRadial<GraphNodeDatum> | null>(null);
  const collideRef = useRef<ForceCollide<GraphNodeDatum> | null>(null);

  // ── загрузка данных: локальный N-hop считает Rust (глубина = hops); единый — топ-N ──
  useEffect(() => {
    if (mode === 'local' && !center) {
      setGraph(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setOrphanPop(null);
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
      const isFull = mode === 'full';
      const stage = isFull ? STAGE.full : STAGE.local;
      const cx = stage.w / 2;
      const cy = stage.h / 2;
      const rnd = makeRnd(13);
      const nodes: GraphNodeDatum[] = data.nodes.map((n, i) => {
        const d = deg[String(n.id)] ?? 0;
        const node: GraphNodeDatum = {
          id: String(n.id),
          title: n.title ?? basename(n.path),
          path: n.path,
          deg: d,
          tags: n.tags ?? [],
        };
        // Seed-позиции: сироты — плотный нимб ЧУТЬ СНАРУЖИ ядра (GRAPH-1: не у края сцены — иначе
        // «разлетаются по углам»); связанные — у центра; локальный — круг вокруг центра.
        if (isFull && d === 0) {
          const ang = rnd() * Math.PI * 2;
          const ring = Math.min(stage.w, stage.h) * ORPHAN_RING_FACTOR * (0.92 + rnd() * 0.16);
          node.ring = ring;
          node.x = cx + Math.cos(ang) * ring;
          node.y = cy + Math.sin(ang) * ring;
        } else if (isFull) {
          node.x = cx + (rnd() - 0.5) * 240;
          node.y = cy + (rnd() - 0.5) * 240;
        } else {
          const ang = (i / data.nodes.length) * Math.PI * 2;
          node.x = cx + Math.cos(ang) * 120 + (rnd() - 0.5) * 50;
          node.y = cy + Math.sin(ang) * 120 + (rnd() - 0.5) * 50;
        }
        return node;
      });
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
        stage,
        isFull,
      });
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, depth, center]);

  // ── d3-force симуляция на смену данных (силы и клампы — формулы макета graph.jsx) ──
  useEffect(() => {
    if (!graph) {
      simRef.current?.stop();
      simRef.current = null;
      return;
    }
    setLoading(true);
    const s = settingsRef.current;
    const { stage, isFull } = graph;
    const cx = stage.w / 2;
    const cy = stage.h / 2;
    // Отталкивание: хабы сильнее, НО степенной член капнут (`min(deg,8)`) — иначе мега-хаб = «бомба»
    // (deg-40 давал −1560, рвал кластеры). Сироты почти не расталкиваются (рыхлое гало, фактор 0.12).
    // distanceMax=340: конечная отсечка локализует раскладку (d3-док — главный рычаг против разлёта);
    // distanceMin=14 (~радиус узла): совпавшие на старте узлы не получают почти-бесконечную силу и не вылетают.
    const charge = forceManyBody<GraphNodeDatum>()
      .strength((d) => chargeStrength(d, s.repel))
      .distanceMin(CHARGE_DISTANCE_MIN)
      .distanceMax(CHARGE_DISTANCE_MAX);
    // ВАЖНО: НЕ задаём link.strength → d3 авто-масштабирует обратно степени (рёбра к хабам слабее).
    // iterations(2) в глобале — чётче кластерная структура (дёшево на top-N).
    const link = forceLink<GraphNodeDatum, GraphLink>(graph.links)
      .id((d) => d.id)
      .distance(s.linkDist)
      .iterations(isFull ? 2 : 1);
    // Гравитация (главный фикс когезии): центр-притяжение теперь СИЛЬНОЕ (дефолт 0.085) и побеждает заряд.
    const gravStrength = (d: GraphNodeDatum) => gravityStrength(d, s.gravity, isFull);
    const gravX = forceX<GraphNodeDatum>(cx).strength(gravStrength);
    const gravY = forceY<GraphNodeDatum>(cy).strength(gravStrength);
    const radial = forceRadial<GraphNodeDatum>((d) => d.ring ?? 0, cx, cy).strength((d) =>
      d.ring ? RADIAL_STRENGTH : 0,
    );
    const collide = forceCollide<GraphNodeDatum>()
      .radius((d) => nodeRadius(d.deg) * s.sizeScale + 6)
      .iterations(2);
    // Группировка по тегам (макет gs-toggle): мягкое притяжение к центроиду первого тега.
    const groupForce: Force<GraphNodeDatum, GraphLink> = (alpha: number) => {
      if (!settingsRef.current.group) return;
      const cents = new Map<string, { x: number; y: number; n: number }>();
      for (const n of graph.nodes) {
        if (n.ring) continue;
        const g = n.tags[0] ?? '_';
        const c = cents.get(g) ?? { x: 0, y: 0, n: 0 };
        c.x += n.x ?? 0;
        c.y += n.y ?? 0;
        c.n += 1;
        cents.set(g, c);
      }
      for (const n of graph.nodes) {
        if (n.ring) continue;
        const c = cents.get(n.tags[0] ?? '_');
        if (!c || c.n < 2) continue;
        n.vx = (n.vx ?? 0) + (c.x / c.n - (n.x ?? 0)) * 0.03 * alpha;
        n.vy = (n.vy ?? 0) + (c.y / c.n - (n.y ?? 0)) * 0.03 * alpha;
      }
    };
    // Рендер-троттл «дыхания»: первые ~1.6с (укладка) и во время drag рендерим каждый тик,
    // дальше сим жив на alphaTarget, но React-рендер — каждый 3-й тик (~20fps вместо 60:
    // на 600 узлах экономит CPU втрое, микро-движение глазом неотличимо).
    const startedAt = performance.now();
    let tickN = 0;
    const sim = forceSimulation<GraphNodeDatum, GraphLink>(graph.nodes)
      .velocityDecay(VELOCITY_DECAY)
      .force('charge', charge)
      .force('link', link)
      .force('x', gravX)
      .force('y', gravY)
      .force('radial', radial)
      .force('group', groupForce)
      .force('collide', collide)
      .on('tick', () => {
        // Клампы-сейфнет (общий с тестами `clampNodePosition`): сирота — в полосе кольца, связный
        // глобального — ≤ coreMax от центра, общие границы. Узлы симуляции === graph.nodes (переданы
        // в forceSimulation) — без обращения к sim из тика (мок d3 в тестах зовёт тик синхронно).
        for (const n of graph.nodes) {
          if (n.fx != null) continue; // перетаскиваемую не дёргаем
          clampNodePosition(n, cx, cy, stage.w, stage.h, isFull);
        }
        tickN += 1;
        const breathing = performance.now() - startedAt > 1600 && dragRef.current == null;
        if (breathing && tickN % 3 !== 0) return;
        tick((v) => v + 1);
      });
    chargeRef.current = charge;
    linkRef.current = link;
    gravXRef.current = gravX;
    gravYRef.current = gravY;
    radialRef.current = radial;
    collideRef.current = collide;
    // «Дыхание» макета: alphaTarget чуть выше нуля — граф никогда не замерзает полностью.
    sim.alpha(1).alphaTarget(0.02).restart();
    simRef.current = sim;
    // По остыванию раскладки — авто-fit камеры (v2c) и снятие лоадера.
    const timer = setTimeout(() => {
      setLoading(false);
      setCam(fitCamera(sim.nodes(), stage));
    }, 700);
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
    // Те же общие хелперы, что и при первичной настройке — слайдер не откатывает к старой физике.
    chargeRef.current
      ?.strength((d) => chargeStrength(d, settings.repel))
      .distanceMin(CHARGE_DISTANCE_MIN)
      .distanceMax(CHARGE_DISTANCE_MAX);
    linkRef.current?.distance(settings.linkDist);
    const isFull = graph?.isFull ?? false;
    const gravStrength = (d: GraphNodeDatum) => gravityStrength(d, settings.gravity, isFull);
    gravXRef.current?.strength(gravStrength);
    gravYRef.current?.strength(gravStrength);
    collideRef.current?.radius((d) => nodeRadius(d.deg) * settings.sizeScale + 6);
    simRef.current.alpha(0.5).restart();
  }, [settings, graph]);

  useEffect(
    () => () => {
      simRef.current?.stop();
      simRef.current = null;
    },
    [],
  );

  const stage = graph?.stage ?? STAGE.local;

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
    setCam((c) => zoomCamera(c, Math.exp(e.deltaY * 0.0015), p.x, p.y, stage.w));
  };

  // Пан по пустому фону (mousedown мимо нод; ноды гасят всплытие в onDown).
  const onStagePan = (e: React.MouseEvent) => {
    e.preventDefault();
    setOrphanPop(null);
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
    setCam(fitCamera(nodes, stage));
  }, [stage]);

  // ── drag: пиннуем ноду (fx/fy) + разогрев; связанные подтягиваются физикой с сопротивлением ──
  const onDown = useCallback(
    (node: GraphNodeDatum) => (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation(); // не запускать пан фона (DP-6)
      const sim = simRef.current;
      if (!sim) return;
      // Освобождаем ранее «закреплённые» ноды: pin не навсегда (как в Obsidian).
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
      const up = (ev: MouseEvent) => {
        sim.alphaTarget(0.02); // обратно к «дыханию», не к нулю
        setDragId(null);
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', up);
        if (moved) {
          // перетащили → нода ОСТАЁТСЯ там, где бросили (sticky, как в Obsidian).
        } else if (node.deg === 0) {
          // клик по сироте → поповер «Изолированная заметка» (макет orphan-pop), не открытие
          node.fx = null;
          node.fy = null;
          const sr = stageRef.current?.getBoundingClientRect();
          if (sr) {
            setOrphanPop({
              path: node.path,
              x: ev.clientX - sr.left,
              y: ev.clientY - sr.top,
              phase: 'info',
            });
          }
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

  // «Предложить связь» для изолированной заметки (макет op-ai): топ-1 предложения Ф1-9.
  const suggestForOrphan = useCallback((path: string) => {
    setOrphanPop((p) => (p ? { ...p, phase: 'thinking' } : p));
    tauriApi.suggest
      .forFile(path, 1)
      .then((list) =>
        setOrphanPop((p) =>
          p && p.phase === 'thinking' ? { ...p, phase: 'done', pick: list[0] ?? null } : p,
        ),
      )
      .catch(() =>
        setOrphanPop((p) => (p && p.phase === 'thinking' ? { ...p, phase: 'done', pick: null } : p)),
      );
  }, []);

  const focus = dragId ?? hover;
  const nbrs = useMemo(() => (graph ? neighborSet(graph.edgeIds, focus) : null), [graph, focus]);
  const kin = useMemo(
    () => (graph ? kinSet(graph.edgeIds, graph.activeId) : new Set<string>()),
    [graph],
  );

  // ── тег-чипы (макет graph.jsx): топ-8 тегов текущего графа; выбранный гасит остальные узлы ──
  const tagChips = useMemo(() => (graph ? topTags(graph.nodes, TAG_CHIP_LIMIT) : []), [graph]);
  // После перезагрузки данных (mode/depth/центр) выбранного тега может не быть — фильтр не применяем.
  const activeTag = tagFilter != null && tagChips.includes(tagFilter) ? tagFilter : null;
  const tagFaded = useCallback(
    (n: GraphNodeDatum) => activeTag != null && !n.tags.includes(activeTag),
    [activeTag],
  );

  const showCanvas = mode === 'full' || !!center;
  // Лейблы макета: всегда у активной/hover/drag; у остальных — только средний зум (1.25…3.2).
  const scale = stage.w / cam.w;
  const labelsByZoom = scale >= 1.25 && scale <= 3.2;

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
        {tagChips.length > 0 && (
          <div className="graph-tags" role="group" aria-label={t('graph.tags')}>
            {tagChips.map((tag) => (
              <button
                key={tag}
                className={'gt-chip' + (activeTag === tag ? ' on' : '')}
                aria-pressed={activeTag === tag}
                onClick={() => setTagFilter((f) => (f === tag ? null : tag))}
              >
                #{tag}
              </button>
            ))}
          </div>
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

      <div className="graph-stage" ref={stageRef}>
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
                const active =
                  (nbrs ? nbrs.has(sId) && nbrs.has(tId) : true) && !tagFaded(s) && !tagFaded(tt);
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
              const faded = (nbrs != null && !nbrs.has(n.id)) || tagFaded(n);
              const isKin = !isActive && kin.has(n.id);
              // Активная нота красится акцентом из CSS (inline-fill перебил бы класс .active).
              const fill = isActive ? null : nodeColor(n.tags);
              const pin = isActive || n.id === focus;
              const labelOn = pin || labelsByZoom;
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
                  <circle r={r} className="g-dot" style={fill ? { fill } : undefined} />
                  {isActive && <circle r={r + 5} className="g-ring" />}
                  {isKin && <circle r={r + 3.5} className="g-kinring" />}
                  {labelOn && (
                    <text y={r + 14} className="g-label" textAnchor="middle">
                      {n.title}
                    </text>
                  )}
                </g>
              );
            })}
          </svg>
        )}

        {showCanvas && graph && (
          <div className="graph-zoom">
            <button
              className="gz-btn"
              onClick={() => setCam((c) => zoomCamera(c, 0.8, c.x + c.w / 2, c.y + c.h / 2, stage.w))}
              title={t('graph.zoomIn')}
              aria-label={t('graph.zoomIn')}
            >
              <Plus size={15} />
            </button>
            <button
              className="gz-btn"
              onClick={() => setCam((c) => zoomCamera(c, 1.25, c.x + c.w / 2, c.y + c.h / 2, stage.w))}
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

        {/* Поповер изолированной заметки (макет orphan-pop): почему одна + AI-предложение связи. */}
        {orphanPop && (
          <div
            className="orphan-pop"
            style={{ left: orphanPop.x, top: orphanPop.y }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <button
              className="op-close"
              onClick={() => setOrphanPop(null)}
              aria-label={t('graph.close')}
            >
              <X size={13} />
            </button>
            <div className="op-head">
              <span className="op-dot" />
              <span>{t('graph.orphanTitle')}</span>
            </div>
            <div className="op-sub">{t('graph.orphanSub')}</div>
            {orphanPop.phase === 'info' && (
              <button className="op-ai" onClick={() => suggestForOrphan(orphanPop.path)}>
                <BrandThinking size={15} />
                <span>{t('graph.orphanSuggest')}</span>
              </button>
            )}
            {orphanPop.phase === 'thinking' && (
              <div className="op-think">
                <BrandThinking size={16} />
                <span className="mt-label">{t('graph.orphanThinking')}</span>
              </div>
            )}
            {orphanPop.phase === 'done' &&
              (orphanPop.pick ? (
                <div className="op-result">
                  <div className="op-rlabel">{t('graph.orphanResult')}</div>
                  <button
                    className="op-link"
                    onClick={() => {
                      const target = orphanPop.pick?.path;
                      setOrphanPop(null);
                      if (target) {
                        close();
                        void openFile(target);
                      }
                    }}
                  >
                    <Link2 size={13} />
                    <span>[[{orphanPop.pick.title ?? basename(orphanPop.pick.path)}]]</span>
                  </button>
                </div>
              ) : (
                <div className="op-sub">{t('graph.orphanNone')}</div>
              ))}
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
              min={80}
              max={600}
              step={20}
              value={settings.repel}
              onChange={(v) => setSettings((s) => ({ ...s, repel: v }))}
            />
            <SettingRow
              label={t('graph.linkDist')}
              min={24}
              max={140}
              step={2}
              value={settings.linkDist}
              onChange={(v) => setSettings((s) => ({ ...s, linkDist: v }))}
            />
            <SettingRow
              label={t('graph.gravity')}
              min={0.02}
              max={0.2}
              step={0.005}
              value={settings.gravity}
              fmt={(v) => v.toFixed(3)}
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
            {/* Группировка по тегам — gs-toggle макета. */}
            <button
              type="button"
              className="graph-row graph-grouprow"
              onClick={() => setSettings((s) => ({ ...s, group: !s.group }))}
              aria-pressed={settings.group}
            >
              <span className="graph-row-label">{t('graph.groupTags')}</span>
              <span className={'gs-switch' + (settings.group ? ' on' : '')}>
                <span className="gs-knob" />
              </span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
