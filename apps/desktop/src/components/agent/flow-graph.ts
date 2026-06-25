/**
 * Граф выполнения агента — ЧИСТАЯ модель (без React/DOM), вид над состоянием ПОСЛЕДНЕГО хода.
 *
 * Заменяет фейковый статичный ResearchGraph: вертикальное дерево-таймлайн (trunk = последовательность
 * шагов, ветви = делегирование субагентам). Работает ОДИНАКОВО для Castor (нативный) и Hermes (ACP):
 * оба бэкенда наполняют `turn.steps`/`turn.subagents`/`turn.execItems` через ОДИН и тот же редьюсер
 * `AgentStreamEvent` в `stores/agent.ts`, поэтому модель ничего не знает о происхождении хода. Единственная
 * точка нормализации — `classifyKind` (унифицирует Castor-точечные глаголы и Hermes-ACP-енам).
 *
 * Рёбра содержат ТОЛЬКО то, что доказывают данные: ПОСЛЕДОВАТЕЛЬНОСТЬ (порядок массива `turn.steps`)
 * + ДЕЛЕГИРОВАНИЕ (parentRunId/childRunId). НИКАКИХ выдуманных причинных рёбер «инструмент A → инструмент B».
 *
 * NB: `turn.plan` (запланированные шаги) сюда НЕ входит НАМЕРЕННО — он живёт во вкладке «План» (PlanLive).
 * Граф строится из ИСПОЛНЕННОГО состояния (steps+subagents+execItems+report), чтобы вкладки не дублировались
 * и граф был честным (то, что РЕАЛЬНО произошло, а не то, что планировалось).
 */

import type { AgentSubagentState } from '../../lib/tauri-api';
import type { AgentTurn, ExecItem, SubagentNode } from '../../stores/agent';

// ── Категории узлов (общий словарь для иконки+цвета; status — отдельный канал) ───────────────────
export type Category =
  | 'file'
  | 'command'
  | 'web'
  | 'read'
  | 'think'
  | 'subagent'
  | 'root'
  | 'report';

export type FlowStatus = 'running' | 'done' | 'error' | 'pending';

export type FlowNodeKind = 'root' | 'activity' | 'subagent' | 'report' | 'collapsed';

export interface FlowNode {
  id: string;
  kind: FlowNodeKind;
  category: Category;
  status: FlowStatus;
  /** Усечённая (~40 симв.) подпись для отрисовки. */
  label: string;
  /** Полный (неусечённый) текст для нативного <title>. */
  full: string;
  /** Best-effort деталь из args (path/query/command/url) — БЕЗ сырого stdout (приватность §5.6). */
  detail?: string;
  /** Путь для report-узла (клик → openFile). */
  path?: string;
  /** Глубина вложенности (0 — основная линия; >0 — субагент-лейн). */
  depth: number;
  /** Субагент на паузе — пунктирная обводка. */
  paused?: boolean;
}

export type FlowEdgeType = 'seq' | 'branch' | 'return';

export interface FlowEdge {
  id: string;
  from: string;
  to: string;
  type: FlowEdgeType;
}

export interface FlowGraph {
  nodes: FlowNode[];
  edges: FlowEdge[];
}

// ── Константы раскладки (экспортируются для тестов) ──────────────────────────────────────────────
export const ROW_H = 46;
export const TRUNK_X = 28;
export const LANE_W = 26;
export const NODE_R = 7;
export const TOP_PAD = 12;
export const MAX_NODES = 200;
/** Усечение подписей. */
const LABEL_MAX = 40;
/** Сдвиг подписи вправо от центра узла. */
export const LABEL_DX = NODE_R + 8;

// ── classifyKind: единая таблица (Castor точечные глаголы + Hermes ACP енам), default 'think' ────
/**
 * Сопоставляет свободную строку `kind` категории. НИКОГДА не бросает: неизвестные/будущие глаголы
 * безопасно падают в 'think' (иконка обобщается, но рендер не падает). Паритет Castor/Hermes —
 * `classifyKind('edit') === classifyKind('note.edit')` и т.д.
 */
const KIND_TABLE: Record<string, Category> = {
  // file ⊇ Castor {note.create, note.edit, note.delete} + Hermes ACP {edit, write, create, delete, move}
  'note.create': 'file',
  'note.edit': 'file',
  'note.delete': 'file',
  edit: 'file',
  write: 'file',
  create: 'file',
  delete: 'file',
  move: 'file',
  // command ⊇ Castor {shell} + Hermes ACP {execute} + общие {exec, process, git}
  shell: 'command',
  execute: 'command',
  exec: 'command',
  process: 'command',
  git: 'command',
  // web ⊇ Castor {web.search, web.fetch} + Hermes ACP {fetch, search}
  'web.search': 'web',
  'web.fetch': 'web',
  search: 'web',
  fetch: 'web',
  // read ⊇ Castor {note.read, recall, vault.read} + Hermes ACP {read} + общие {grep}
  read: 'read',
  'note.read': 'read',
  recall: 'read',
  'vault.read': 'read',
  grep: 'read',
  // think ⊇ {think, plan, reason} + default
  think: 'think',
  plan: 'think',
  reason: 'think',
};

export function classifyKind(kind: string): Category {
  return KIND_TABLE[kind] ?? 'think';
}

// ── Хелперы ──────────────────────────────────────────────────────────────────────────────────────
function truncate(s: string, max = LABEL_MAX): string {
  if (s.length <= max) return s;
  return s.slice(0, max - 1).trimEnd() + '…';
}

/** Короткая человекочитаемая подпись activity-узла (последний сегмент точечного глагола). */
function kindLabel(kind: string): string {
  return kind;
}

/**
 * Best-effort деталь из JSON args: первый из path|query|command|url. Тот же try/catch-паттерн, что и
 * `proposedContentByPath` (AgentView). НИКОГДА не вытаскивает сырой stdout (приватность §5.6).
 */
function detailFromArgs(args: string): string | undefined {
  try {
    const a = JSON.parse(args) as Record<string, unknown>;
    for (const key of ['path', 'query', 'command', 'url'] as const) {
      const v = a[key];
      if (typeof v === 'string' && v.length > 0) return truncate(v);
    }
  } catch {
    /* кривой args — детали нет */
  }
  return undefined;
}

/** Статус subagent-состояния → FlowStatus (paused/spawned/running → running). */
function subStatus(s: AgentSubagentState): FlowStatus {
  if (s === 'done') return 'done';
  if (s === 'failed') return 'error';
  return 'running'; // spawned | running | paused
}

/** Статус exec-айтема (нет step-дубля) → FlowStatus. */
function execStatus(it: ExecItem): FlowStatus {
  if (!it.finalized) return 'running';
  return it.exitCode === 0 ? 'done' : 'error';
}

// ── deriveFlowGraph: строит {nodes, edges} из исполненного состояния хода ─────────────────────────
/**
 * Узлы: root (задача) · activity (по `turn.steps` В ПОРЯДКЕ МАССИВА = временной порядок) ·
 * subagent (ветви по `turn.subagents`, вложенность по parentRunId/childRunId) · command-узлы из
 * `turn.execItems` ТОЛЬКО если нет соответствующего step (де-дуп, иначе команда нарисуется дважды) ·
 * terminal report/done/error.
 *
 * Рёбра: seq (trunk: root→a0→…→terminal) · branch (родитель→субагент, эльбоу). НЕТ activity→activity
 * branch-рёбер, НЕТ ребра между step и субагентом (нет поля корреляции — было бы выдуманной причинностью).
 *
 * Кап MAX_NODES: при steps.length>MAX_NODES → root + псевдо-узел 'collapsed' («+N earlier») + последние
 * (MAX_NODES-1) шагов (живой хвост — то, что важно).
 */
export function deriveFlowGraph(turn: AgentTurn | null): FlowGraph {
  const nodes: FlowNode[] = [];
  const edges: FlowEdge[] = [];
  if (!turn) return { nodes, edges };

  const runId = turn.runId ?? 0;

  // root всегда первый.
  const rootLabel = turn.task || 'task';
  nodes.push({
    id: 'root',
    kind: 'root',
    category: 'root',
    status:
      turn.status === 'error' ? 'error' : turn.status === 'done' ? 'done' : 'running',
    label: truncate(rootLabel),
    full: rootLabel,
    depth: 0,
  });

  // mainline (trunk) id'шники — для seq-рёбер в строгом порядке.
  const mainLineIds: string[] = ['root'];

  // ── activity-узлы из steps (в порядке массива; кап MAX_NODES) ──
  const steps = turn.steps ?? [];
  const overCap = steps.length > MAX_NODES;
  // Какие step.id уже отрисованы как activity — для де-дупа execItems.
  const stepIds = new Set<string>();
  let visibleSteps = steps;
  if (overCap) {
    // root + collapsed + последние (MAX_NODES-1) шагов.
    const tail = steps.slice(steps.length - (MAX_NODES - 1));
    const hidden = steps.length - tail.length;
    nodes.push({
      id: 'collapsed',
      kind: 'collapsed',
      category: 'think',
      status: 'done',
      label: `+${hidden} earlier`,
      full: `+${hidden} earlier steps`,
      depth: 0,
    });
    mainLineIds.push('collapsed');
    visibleSteps = tail;
  }
  for (const st of visibleSteps) {
    stepIds.add(st.id);
    const detail = detailFromArgs(st.args);
    const id = `step:${st.id}`;
    nodes.push({
      id,
      kind: 'activity',
      category: classifyKind(st.kind),
      status: st.result == null ? 'running' : st.isError ? 'error' : 'done',
      label: truncate(kindLabel(st.kind)),
      full: st.kind,
      detail,
      depth: 0,
    });
    mainLineIds.push(id);
  }

  // ── execItems → command-узлы ТОЛЬКО без соответствующего step (де-дуп, иначе двойная отрисовка) ──
  // На macOS exec обычно течёт через steps; на Linux-песочнице execResult может прийти без step.
  // Соответствие: actionId совпадает с suffix step.id вида `exec:${actionId}` ИЛИ есть command-step.
  // Консервативно: если ЛЮБОЙ step категории 'command' существует, считаем exec уже представленным
  // (де-дуп по design «only add exec nodes when no matching step exists»). Иначе добавляем хвостом.
  const execItems = turn.execItems ?? [];
  const hasCommandStep = visibleSteps.some((st) => classifyKind(st.kind) === 'command');
  if (!hasCommandStep) {
    for (const it of execItems) {
      const id = `exec:${it.actionId}`;
      if (stepIds.has(`${it.actionId}`)) continue; // уже как step
      nodes.push({
        id,
        kind: 'activity',
        category: 'command',
        status: execStatus(it),
        label: truncate(it.summary || 'command'),
        full: it.summary || 'command',
        depth: 0,
      });
      mainLineIds.push(id);
    }
  }

  // ── seq-рёбра: между каждой парой соседних mainline-узлов (строго в порядке исполнения) ──
  // (terminal-узел добавится в mainLineIds ниже, перед построением seq-рёбер.)

  // ── subagent-ветви ──
  // Резолв родителя: если другой субагент childRunId === this.parentRunId → ветвь от него (вложенность,
  // depth = parent.depth+1); иначе → ветвь от root (max_depth=1: оркестратор-run = trunk-root).
  // ЭТО ЧЕСТНО: ребро утверждает «основной run породил субагента» (из parentRunId), НЕ ложную тонкую
  // причинность «этот конкретный шаг породил субагента» — поля корреляции step↔subagent НЕТ.
  const subs = turn.subagents ?? [];
  const subById = new Map<number, SubagentNode>(); // childRunId → node
  for (const s of subs) subById.set(s.childRunId, s);
  const subDepth = new Map<number, number>(); // childRunId → depth
  const subNodeId = (childRunId: number) => `sub:${childRunId}`;
  const resolveDepth = (s: SubagentNode, guard = 0): number => {
    if (guard > 32) return 1; // защита от циклов
    const cached = subDepth.get(s.childRunId);
    if (cached != null) return cached;
    const parent = subById.get(s.parentRunId);
    const d = parent ? resolveDepth(parent, guard + 1) + 1 : 1;
    subDepth.set(s.childRunId, d);
    return d;
  };
  for (const s of subs) {
    const depth = resolveDepth(s);
    const id = subNodeId(s.childRunId);
    const goal = s.goal || 'subagent';
    nodes.push({
      id,
      kind: 'subagent',
      category: 'subagent',
      status: subStatus(s.status),
      label: truncate(goal),
      full: s.summary ? `${goal} — ${s.summary}` : goal,
      detail: s.summary ? truncate(s.summary) : undefined,
      depth,
      paused: s.status === 'paused',
    });
    // branch-ребро от резолвнутого родителя.
    const parentSub = subById.get(s.parentRunId);
    const fromId = parentSub ? subNodeId(parentSub.childRunId) : 'root';
    edges.push({ id: `b:${s.childRunId}`, from: fromId, to: id, type: 'branch' });
  }

  // ── terminal-узел (report / done / error) ──
  let terminalId: string | null = null;
  if (turn.researchReport) {
    terminalId = `report:${runId}`;
    nodes.push({
      id: terminalId,
      kind: 'report',
      category: 'report',
      status: 'done',
      label: truncate(turn.researchReport.title),
      full: turn.researchReport.title,
      path: turn.researchReport.path,
      depth: 0,
    });
  } else if (turn.status === 'done' && turn.report) {
    terminalId = `done:${runId}`;
    nodes.push({
      id: terminalId,
      kind: 'report',
      category: 'report',
      status: 'done',
      label: 'Done',
      full: turn.report,
      depth: 0,
    });
  } else if (turn.status === 'error') {
    terminalId = `error:${runId}`;
    nodes.push({
      id: terminalId,
      kind: 'report',
      category: 'report',
      status: 'error',
      label: 'Error',
      full: turn.error ?? 'error',
      depth: 0,
    });
  }
  if (terminalId) mainLineIds.push(terminalId);

  // seq-рёбра по итоговой mainline.
  for (let i = 1; i < mainLineIds.length; i++) {
    edges.push({
      id: `s:${mainLineIds[i - 1]}->${mainLineIds[i]}`,
      from: mainLineIds[i - 1],
      to: mainLineIds[i],
      type: 'seq',
    });
  }

  return { nodes, edges };
}

// ── layoutFlow: детерминированная O(n) раскладка дерева-таймлайна ─────────────────────────────────
/**
 * Строит упорядоченный список строк (root → top-level субагенты сразу после root, сгруппированы;
 * вложенные рекурсивно с depth+1 → trunk-activity в порядке → terminal), затем y = TOP_PAD+row*ROW_H,
 * x = TRUNK_X (+depth*LANE_W для субагентов). Чисто и детерминированно: одинаковый вход → одинаковые
 * позиции (тестируется ассертами точных координат).
 */
export function layoutFlow(
  graph: FlowGraph,
  width: number,
): { positions: Map<string, { x: number; y: number }>; height: number; width: number } {
  const byId = new Map<string, FlowNode>();
  for (const n of graph.nodes) byId.set(n.id, n);

  // Дочерние субагенты по from-ребру branch (parent id → [sub id...]).
  const branchChildren = new Map<string, string[]>();
  for (const e of graph.edges) {
    if (e.type !== 'branch') continue;
    const arr = branchChildren.get(e.from) ?? [];
    arr.push(e.to);
    branchChildren.set(e.from, arr);
  }

  const order: string[] = [];
  const emitted = new Set<string>();
  // Рекурсивно эмитим субагентов под узлом (сохраняя порядок добавления = порядок массива subagents).
  const emitSubsOf = (parentId: string) => {
    const kids = branchChildren.get(parentId);
    if (!kids) return;
    for (const kid of kids) {
      if (emitted.has(kid)) continue;
      emitted.add(kid);
      order.push(kid);
      emitSubsOf(kid); // вложенные
    }
  };

  // Основная линия в порядке: root, его субагенты, activity-узлы (каждый со своими субагентами), terminal.
  for (const n of graph.nodes) {
    if (n.kind === 'subagent') continue; // субагенты эмитятся под родителями
    if (emitted.has(n.id)) continue;
    emitted.add(n.id);
    order.push(n.id);
    emitSubsOf(n.id);
  }

  const positions = new Map<string, { x: number; y: number }>();
  order.forEach((id, row) => {
    const node = byId.get(id);
    const depth = node ? node.depth : 0;
    positions.set(id, {
      x: TRUNK_X + depth * LANE_W,
      y: TOP_PAD + row * ROW_H,
    });
  });

  const height = order.length > 0 ? TOP_PAD + order.length * ROW_H : TOP_PAD + ROW_H;
  return { positions, height, width: Math.max(width, 0) };
}
