import { useEffect, useMemo, useRef, useState } from 'react';
import type { ComponentType } from 'react';
import { AlertTriangle, BookOpen, FileText, Globe, Share2, Terminal } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { OrbitIcon } from '../common/BrandGlyphs';
import { BrandThinking } from '../common/BrandThinking';
import { useAgentStore } from '../../stores/agent';
import { useWorkspaceStore } from '../../stores/workspace';
import {
  deriveFlowGraph,
  layoutFlow,
  NODE_R,
  LABEL_DX,
  type Category,
  type FlowNode,
  type FlowStatus,
} from './flow-graph';
import styles from './AgentView.module.css';

/** Ширина по умолчанию до первого замера ResizeObserver (узкий правый dock ≈254px; без 0-width SVG). */
const FALLBACK_WIDTH = 254;

/** Иконка по категории (status-канал отдельно: running → BrandThinking, error → AlertTriangle, иначе — иконка категории). */
const CATEGORY_ICON: Record<Category, ComponentType<{ size?: number }>> = {
  file: FileText,
  command: Terminal,
  web: Globe,
  read: BookOpen,
  think: BrandThinking,
  subagent: Share2,
  root: OrbitIcon,
  report: FileText,
};

/** status → класс обводки/кольца узла (зеркало конвенции .piRun/.piDone/.piErr из PlanLive). */
function statusClass(status: FlowStatus): string {
  if (status === 'running' || status === 'pending') return styles.fgRun;
  if (status === 'error') return styles.fgErr;
  return styles.fgDone;
}

/**
 * Граф выполнения агента (ExecGraph) — РЕАЛЬНЫЙ inline-SVG-вид над состоянием ПОСЛЕДНЕГО хода (заменяет
 * фейковый статичный ResearchGraph). Вертикальное дерево-таймлайн: trunk = последовательность шагов,
 * ветви = делегирование субагентам. Подписывается на стор как PlanLive; модель/раскладка чисты и
 * детерминированы (flow-graph.ts) → перерасчёт на каждом стрим-событии дёшев (O(n), без физики).
 *
 * NB: `turn.plan` НЕ показывается тут НАМЕРЕННО — он живёт во вкладке «План». Граф = ИСПОЛНЕННОЕ
 * состояние (steps/subagents/execItems/report), не планируемое.
 *
 * Hermes/Castor паритет: вид чист над turn.* (оба бэкенда питают их через ОДИН редьюсер), единственная
 * нормализация — classifyKind (flow-graph.ts).
 */
export function ExecGraph() {
  const { t } = useTranslation();
  const turns = useAgentStore((s) => s.turns);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const turn = turns.at(-1) ?? null;

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(FALLBACK_WIDTH);

  // Ширина из ResizeObserver на контейнере (fallback 254 до первого замера → без 0-width SVG).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w && w > 0) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const flow = useMemo(
    () => deriveFlowGraph(turn),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- по полям хода (стабильно при стриме)
    [
      turn?.task,
      turn?.steps,
      turn?.subagents,
      turn?.execItems,
      turn?.researchReport,
      turn?.report,
      turn?.status,
      turn?.error,
      turn?.runId,
    ],
  );
  const laid = useMemo(() => layoutFlow(flow, width), [flow, width]);

  // Авто-скролл вниз ТОЛЬКО когда уже прижаты к низу (как лог/лента) — не дёргаем юзера, ушедшего вверх.
  const prevHeight = useRef(0);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const pinned = el.scrollTop + el.clientHeight >= prevHeight.current - 4;
    prevHeight.current = el.scrollHeight;
    if (pinned) el.scrollTop = el.scrollHeight;
  }, [laid.height]);

  // Честный пустой стейт (нет ходов / ход без действий и не завершён) — без фейкового демо-графа.
  const isEmpty = flow.nodes.length === 0 || (flow.nodes.length === 1 && flow.nodes[0]?.kind === 'root' && (turn?.steps?.length ?? 0) === 0 && (turn?.subagents?.length ?? 0) === 0 && turn?.status !== 'done' && turn?.status !== 'error');
  if (isEmpty) {
    // root-only стаб (running без шагов) — короткий узел, чтобы не выглядело сломанным; иначе пусто.
    if (turn?.status === 'running' && flow.nodes.length === 1) {
      // покажем сам root-узел через общий рендер ниже.
    } else {
      return (
        <div className={styles.flow} ref={scrollRef}>
          <div className={styles.fgEmpty}>{t('agent.graph.empty')}</div>
        </div>
      );
    }
  }

  const pos = laid.positions;
  const point = (id: string) => pos.get(id) ?? { x: 0, y: 0 };

  return (
    <div className={styles.flow} ref={scrollRef}>
      <svg
        className={styles.flowSvg}
        width={laid.width}
        height={laid.height}
        viewBox={`0 0 ${laid.width} ${laid.height}`}
        role="img"
        aria-label={t('agent.graph.aria')}
      >
        {/* рёбра */}
        <g>
          {flow.edges.map((e) => {
            const a = point(e.from);
            const b = point(e.to);
            if (e.type === 'branch') {
              // эльбоу: вниз по trunk родителя, затем вправо в лейн субагента.
              const d = `M ${a.x} ${a.y} L ${a.x} ${b.y} L ${b.x} ${b.y}`;
              return <path key={e.id} className={styles.fgEdgeBranch} d={d} fill="none" />;
            }
            if (e.type === 'return') {
              const d = `M ${a.x} ${a.y} L ${b.x} ${b.y}`;
              return <path key={e.id} className={styles.fgEdgeReturn} d={d} fill="none" />;
            }
            // seq — прямой сегмент trunk.
            return (
              <line
                key={e.id}
                className={styles.fgEdgeSeq}
                x1={a.x}
                y1={a.y}
                x2={b.x}
                y2={b.y}
              />
            );
          })}
        </g>
        {/* узлы */}
        <g>
          {flow.nodes.map((n) => (
            <FlowNodeView
              key={n.id}
              node={n}
              x={point(n.id).x}
              y={point(n.id).y}
              maxLabelW={laid.width}
              onOpen={n.path ? () => void openFile(n.path as string) : undefined}
            />
          ))}
        </g>
      </svg>
    </div>
  );
}

function FlowNodeView({
  node,
  x,
  y,
  maxLabelW,
  onOpen,
}: {
  node: FlowNode;
  x: number;
  y: number;
  maxLabelW: number;
  onOpen?: () => void;
}) {
  // status-канал: running→спиннер, error→тревога, root→Orbit; иначе — иконка КАТЕГОРИИ (статус несёт
  // цвет обводки, см. statusClass). CATEGORY_ICON — полная карта по всем категориям (вкл. 'think' —
  // дефолт для неизвестного kind), поэтому отдельная проверка категорий не нужна (раньше 'think' падал
  // в Check, теряя иконку). `Check` тут больше не нужен как иконка узла.
  const StatusIcon =
    node.status === 'running' || node.status === 'pending'
      ? BrandThinking
      : node.status === 'error'
        ? AlertTriangle
        : node.kind === 'root'
          ? OrbitIcon
          : CATEGORY_ICON[node.category];
  const cls = statusClass(node.status);
  const subCls = node.depth > 0 ? styles.fgSub : '';
  const pausedCls = node.paused ? styles.fgPaused : '';
  const labelX = x + LABEL_DX;
  const labelW = Math.max(40, maxLabelW - labelX - 4);
  const clickable = !!onOpen;
  return (
    <g
      className={`${styles.fgNode} ${cls} ${subCls} ${pausedCls}`.trim()}
      transform={`translate(0,0)`}
      onClick={onOpen}
      style={clickable ? { cursor: 'pointer' } : undefined}
      role={clickable ? 'button' : undefined}
      tabIndex={clickable ? 0 : undefined}
      onKeyDown={
        clickable
          ? (ev) => {
              if (ev.key === 'Enter' || ev.key === ' ') {
                ev.preventDefault();
                onOpen?.();
              }
            }
          : undefined
      }
    >
      <title>{node.full}</title>
      {/* кружок узла (обводка/кольцо по status-классу) */}
      <circle className={styles.fgDot} cx={x} cy={y} r={NODE_R} />
      {/* иконка статуса/категории в центре кружка через foreignObject */}
      <foreignObject x={x - 7} y={y - 7} width={14} height={14} aria-hidden>
        <div className={styles.fgIcon}>
          <StatusIcon size={12} />
        </div>
      </foreignObject>
      {/* подпись */}
      <foreignObject x={labelX} y={y - 9} width={labelW} height={18}>
        <div className={styles.fgLabel} title={node.full}>
          {node.label}
          {node.detail ? <span className={styles.fgDetail}> {node.detail}</span> : null}
        </div>
      </foreignObject>
    </g>
  );
}
