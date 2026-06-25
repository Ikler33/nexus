import { describe, expect, it } from 'vitest';

import {
  classifyKind,
  deriveFlowGraph,
  layoutFlow,
  ROW_H,
  TRUNK_X,
  LANE_W,
  TOP_PAD,
  MAX_NODES,
  type Category,
} from './flow-graph';
import type { AgentTurn } from '../../stores/agent';

/** Минимальный валидный ход с переопределяемыми полями (остальное — пустые дефолты). */
function makeTurn(over: Partial<AgentTurn>): AgentTurn {
  return {
    key: 0,
    epoch: 1,
    runId: 1,
    task: 'task',
    assistantText: '',
    steps: [],
    changeset: [],
    plan: [],
    subagents: [],
    execItems: [],
    researchReport: null,
    report: null,
    error: null,
    status: 'running',
    ...over,
  };
}

describe('classifyKind (нормализация Castor + Hermes ACP, default think, never throws)', () => {
  const castor: Array<[string, Category]> = [
    ['note.create', 'file'],
    ['note.edit', 'file'],
    ['note.delete', 'file'],
    ['web.search', 'web'],
    ['web.fetch', 'web'],
    ['shell', 'command'],
    ['note.read', 'read'],
    ['recall', 'read'],
    ['think', 'think'],
  ];
  const hermes: Array<[string, Category]> = [
    ['edit', 'file'],
    ['write', 'file'],
    ['create', 'file'],
    ['delete', 'file'],
    ['move', 'file'],
    ['execute', 'command'],
    ['fetch', 'web'],
    ['search', 'web'],
    ['read', 'read'],
  ];
  it.each(castor)('Castor %s → %s', (k, cat) => {
    expect(classifyKind(k)).toBe(cat);
  });
  it.each(hermes)('Hermes ACP %s → %s', (k, cat) => {
    expect(classifyKind(k)).toBe(cat);
  });
  it('неизвестный глагол → think (никогда не бросает)', () => {
    expect(classifyKind('frobnicate')).toBe('think');
    expect(classifyKind('')).toBe('think');
    // @ts-expect-error — намеренно невалидный вход (free string in prod)
    expect(() => classifyKind(undefined)).not.toThrow();
  });
  it('ПАРИТЕТ Castor↔Hermes: одна категория для эквивалентных глаголов', () => {
    expect(classifyKind('edit')).toBe(classifyKind('note.edit'));
    expect(classifyKind('execute')).toBe(classifyKind('shell'));
    expect(classifyKind('search')).toBe(classifyKind('web.search'));
    expect(classifyKind('read')).toBe(classifyKind('note.read'));
  });
});

describe('deriveFlowGraph (узлы/статусы/рёбра из исполненного состояния хода)', () => {
  it('фикстура: root + 4 activity + 2 subagent + 1 report; seq-цепь; 2 branch-ребра', () => {
    const turn = makeTurn({
      task: 'исследуй тему',
      steps: [
        { id: 'a', kind: 'note.create', args: '{"path":"X.md"}', result: 'ok', isError: false },
        { id: 'b', kind: 'web.search', args: '{"query":"q"}', result: null, isError: false }, // running
        { id: 'c', kind: 'read', args: 'bad json', result: 'r', isError: true }, // error
        { id: 'd', kind: 'think', args: '{}', result: 'done', isError: false },
      ],
      subagents: [
        { childRunId: 2, parentRunId: 1, goal: 'найти источники', status: 'done', summary: 'нашёл 5' },
        { childRunId: 3, parentRunId: 1, goal: 'суммировать', status: 'running', summary: undefined },
      ],
      researchReport: { runId: 1, title: 'Отчёт', path: 'reports/r.md', sourcesCount: 5, rounds: 2 },
      status: 'running',
    });
    const g = deriveFlowGraph(turn);
    const byKind = (k: string) => g.nodes.filter((n) => n.kind === k);
    expect(byKind('root')).toHaveLength(1);
    expect(byKind('activity')).toHaveLength(4);
    expect(byKind('subagent')).toHaveLength(2);
    expect(byKind('report')).toHaveLength(1);
    expect(g.nodes).toHaveLength(8);

    // статусы activity выведены из result/isError.
    const stepStatus = (id: string) => g.nodes.find((n) => n.id === `step:${id}`)!.status;
    expect(stepStatus('a')).toBe('done');
    expect(stepStatus('b')).toBe('running');
    expect(stepStatus('c')).toBe('error');
    expect(stepStatus('d')).toBe('done');

    // деталь из args (path/query); кривой json → нет детали.
    expect(g.nodes.find((n) => n.id === 'step:a')!.detail).toBe('X.md');
    expect(g.nodes.find((n) => n.id === 'step:b')!.detail).toBe('q');
    expect(g.nodes.find((n) => n.id === 'step:c')!.detail).toBeUndefined();

    // subagent статусы.
    expect(g.nodes.find((n) => n.id === 'sub:2')!.status).toBe('done');
    expect(g.nodes.find((n) => n.id === 'sub:3')!.status).toBe('running');

    // seq-цепь: root→step:a→step:b→step:c→step:d→report (5 рёбер).
    const seq = g.edges.filter((e) => e.type === 'seq');
    expect(seq).toHaveLength(5);
    expect(seq[0]).toMatchObject({ from: 'root', to: 'step:a' });
    expect(seq[4].to).toBe('report:1');

    // 2 branch-ребра root→sub.
    const branch = g.edges.filter((e) => e.type === 'branch');
    expect(branch).toHaveLength(2);
    branch.forEach((e) => {
      expect(e.from).toBe('root');
      expect(e.to.startsWith('sub:')).toBe(true);
    });
  });

  it('НЕГАТИВ: нет activity→activity branch; branch.to всегда субагент, branch.from = root/родитель', () => {
    const turn = makeTurn({
      steps: [
        { id: 'a', kind: 'read', args: '{}', result: 'r', isError: false },
        { id: 'b', kind: 'edit', args: '{}', result: 'r', isError: false },
      ],
      subagents: [{ childRunId: 9, parentRunId: 1, goal: 'g', status: 'running' }],
    });
    const g = deriveFlowGraph(turn);
    const branch = g.edges.filter((e) => e.type === 'branch');
    // нет ни одного branch-ребра между двумя activity.
    const isActivity = (id: string) => id.startsWith('step:');
    for (const e of branch) {
      expect(isActivity(e.from) && isActivity(e.to)).toBe(false);
      expect(e.to.startsWith('sub:')).toBe(true);
      expect(e.from === 'root').toBe(true);
    }
    // нет ребра step↔sub.
    expect(g.edges.some((e) => e.from === 'step:a' && e.to === 'sub:9')).toBe(false);
  });

  it('вложенность субагентов: subB.parentRunId === subA.childRunId → branch from subA, depth+1', () => {
    const turn = makeTurn({
      subagents: [
        { childRunId: 2, parentRunId: 1, goal: 'A', status: 'running' },
        { childRunId: 3, parentRunId: 2, goal: 'B', status: 'running' }, // вложен в A
      ],
    });
    const g = deriveFlowGraph(turn);
    const a = g.nodes.find((n) => n.id === 'sub:2')!;
    const b = g.nodes.find((n) => n.id === 'sub:3')!;
    expect(a.depth).toBe(1);
    expect(b.depth).toBe(2);
    const branchToB = g.edges.find((e) => e.type === 'branch' && e.to === 'sub:3')!;
    expect(branchToB.from).toBe('sub:2');
  });

  it('пустой ход: 0 steps/subagents, status idle → только root', () => {
    const g = deriveFlowGraph(makeTurn({ status: 'idle', task: 'x' }));
    expect(g.nodes).toHaveLength(1);
    expect(g.nodes[0].kind).toBe('root');
    expect(g.edges).toHaveLength(0);
  });

  it('null-ход → пустой граф', () => {
    const g = deriveFlowGraph(null);
    expect(g.nodes).toHaveLength(0);
    expect(g.edges).toHaveLength(0);
  });

  it('paused-субагент: status→running + paused-флаг', () => {
    const g = deriveFlowGraph(
      makeTurn({ subagents: [{ childRunId: 2, parentRunId: 1, goal: 'g', status: 'paused' }] }),
    );
    const sub = g.nodes.find((n) => n.id === 'sub:2')!;
    expect(sub.status).toBe('running');
    expect(sub.paused).toBe(true);
  });

  it('execItems де-дуп: command-step существует → НЕ добавляем exec-узлы (без двойной отрисовки)', () => {
    const turn = makeTurn({
      steps: [{ id: 's1', kind: 'shell', args: '{}', result: 'ok', isError: false }],
      execItems: [{ runId: 1, actionId: 5, summary: 'shell.run · 2 args', exitCode: 0, finalized: true }],
    });
    const g = deriveFlowGraph(turn);
    expect(g.nodes.filter((n) => n.id.startsWith('exec:'))).toHaveLength(0);
    expect(g.nodes.filter((n) => n.category === 'command')).toHaveLength(1); // только step
  });

  it('execItems без command-step (Linux-песочница) → добавляем как command-узлы', () => {
    const turn = makeTurn({
      steps: [{ id: 's1', kind: 'read', args: '{}', result: 'ok', isError: false }],
      execItems: [
        { runId: 1, actionId: 5, summary: 'build', exitCode: 0, finalized: true },
        { runId: 1, actionId: 6, summary: 'test', exitCode: 1, finalized: true },
        { runId: 1, actionId: 7, summary: 'run', exitCode: null, finalized: false },
      ],
    });
    const g = deriveFlowGraph(turn);
    const execNodes = g.nodes.filter((n) => n.id.startsWith('exec:'));
    expect(execNodes).toHaveLength(3);
    expect(g.nodes.find((n) => n.id === 'exec:5')!.status).toBe('done');
    expect(g.nodes.find((n) => n.id === 'exec:6')!.status).toBe('error');
    expect(g.nodes.find((n) => n.id === 'exec:7')!.status).toBe('running');
  });

  it('терминальный done-узел: status done + report → report-узел status done', () => {
    const g = deriveFlowGraph(makeTurn({ status: 'done', report: 'итог' }));
    const term = g.nodes.find((n) => n.kind === 'report')!;
    expect(term.status).toBe('done');
    expect(term.id).toBe('done:1');
  });

  it('терминальный error-узел: status error → report-узел status error', () => {
    const g = deriveFlowGraph(makeTurn({ status: 'error', error: 'сбой' }));
    const term = g.nodes.find((n) => n.kind === 'report')!;
    expect(term.status).toBe('error');
    expect(term.id).toBe('error:1');
    // seq-цепь честно завершается на error-узле.
    const lastSeq = g.edges.filter((e) => e.type === 'seq').at(-1)!;
    expect(lastSeq.to).toBe('error:1');
  });

  it('усечение длинных подписей до ~40 симв.', () => {
    const long = 'a'.repeat(120);
    const g = deriveFlowGraph(makeTurn({ task: long }));
    const root = g.nodes[0];
    expect(root.label.length).toBeLessThanOrEqual(40);
    expect(root.full).toBe(long); // полный — в title
  });

  it('LIVE-append: добавление шага сохраняет id прежних узлов + один новый', () => {
    const base = makeTurn({
      steps: [{ id: 'a', kind: 'read', args: '{}', result: 'r', isError: false }],
    });
    const g1 = deriveFlowGraph(base);
    const ids1 = g1.nodes.map((n) => n.id);
    const next = makeTurn({
      steps: [
        ...base.steps,
        { id: 'b', kind: 'edit', args: '{}', result: null, isError: false },
      ],
    });
    const g2 = deriveFlowGraph(next);
    const ids2 = g2.nodes.map((n) => n.id);
    // все прежние id присутствуют (стабильное keying), плюс ровно один новый.
    for (const id of ids1) expect(ids2).toContain(id);
    expect(ids2.length).toBe(ids1.length + 1);
    expect(ids2).toContain('step:b');
  });

  it('MAX_NODES кап: 250 шагов → root + collapsed + последние (MAX_NODES-1) activity', () => {
    const steps = Array.from({ length: 250 }, (_, i) => ({
      id: `s${i}`,
      kind: 'read',
      args: '{}',
      result: 'ok',
      isError: false,
    }));
    const g = deriveFlowGraph(makeTurn({ steps }));
    expect(g.nodes.filter((n) => n.kind === 'collapsed')).toHaveLength(1);
    const activity = g.nodes.filter((n) => n.kind === 'activity');
    expect(activity).toHaveLength(MAX_NODES - 1);
    // collapsed подпись отражает число скрытых (250 - 199 = 51).
    const collapsed = g.nodes.find((n) => n.kind === 'collapsed')!;
    expect(collapsed.label).toContain('51');
    // последний видимый шаг = s249 (живой хвост).
    expect(g.nodes.some((n) => n.id === 'step:s249')).toBe(true);
    expect(g.nodes.some((n) => n.id === 'step:s0')).toBe(false);
  });
});

describe('layoutFlow (детерминированная O(n) раскладка — позиции ассертируемы)', () => {
  it('детерминизм + точные позиции: trunk на TRUNK_X, субагент на TRUNK_X+LANE_W', () => {
    const turn = makeTurn({
      task: 't',
      steps: [{ id: 'a', kind: 'read', args: '{}', result: 'r', isError: false }],
      subagents: [{ childRunId: 2, parentRunId: 1, goal: 'g', status: 'running' }],
    });
    const g = deriveFlowGraph(turn);
    const l1 = layoutFlow(g, 254);
    const l2 = layoutFlow(g, 254);
    // одинаковый вход → одинаковый выход.
    expect([...l1.positions.entries()]).toEqual([...l2.positions.entries()]);
    // порядок строк: root (0), sub:2 (1, эмитится сразу под root), step:a (2).
    expect(l1.positions.get('root')).toEqual({ x: TRUNK_X, y: TOP_PAD });
    expect(l1.positions.get('sub:2')).toEqual({ x: TRUNK_X + LANE_W, y: TOP_PAD + ROW_H });
    expect(l1.positions.get('step:a')).toEqual({ x: TRUNK_X, y: TOP_PAD + 2 * ROW_H });
    // высота растёт с числом строк (3 узла).
    expect(l1.height).toBe(TOP_PAD + 3 * ROW_H);
    expect(l1.width).toBe(254);
  });

  it('n=1 (только root) обрабатывается без спец-кейса', () => {
    const g = deriveFlowGraph(makeTurn({ status: 'idle' }));
    const l = layoutFlow(g, 254);
    expect(l.positions.get('root')).toEqual({ x: TRUNK_X, y: TOP_PAD });
    expect(l.height).toBe(TOP_PAD + ROW_H);
  });

  it('вложенная глубина: nested-субагент на TRUNK_X+2*LANE_W', () => {
    const g = deriveFlowGraph(
      makeTurn({
        subagents: [
          { childRunId: 2, parentRunId: 1, goal: 'A', status: 'running' },
          { childRunId: 3, parentRunId: 2, goal: 'B', status: 'running' },
        ],
      }),
    );
    const l = layoutFlow(g, 254);
    expect(l.positions.get('sub:2')!.x).toBe(TRUNK_X + LANE_W);
    expect(l.positions.get('sub:3')!.x).toBe(TRUNK_X + 2 * LANE_W);
  });

  it('fallback-ширина 0 → width нормализуется в >=0', () => {
    const g = deriveFlowGraph(makeTurn({ status: 'idle' }));
    const l = layoutFlow(g, 0);
    expect(l.width).toBeGreaterThanOrEqual(0);
  });
});
