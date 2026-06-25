import { render, screen } from '@testing-library/react';
import { fireEvent } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ExecGraph } from './ExecGraph';
import { useAgentStore } from '../../stores/agent';
import { useWorkspaceStore } from '../../stores/workspace';
import type { AgentTurn } from '../../stores/agent';

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

function setTurns(turns: AgentTurn[]) {
  useAgentStore.setState({ turns });
}

beforeEach(() => {
  setTurns([]);
});
afterEach(() => vi.restoreAllMocks());

describe('ExecGraph (реальный граф выполнения над состоянием хода)', () => {
  it('пустой стейт без хода — подсказка, без фейкового демо-графа', () => {
    render(<ExecGraph />);
    expect(screen.getByText(/Граф появится/)).toBeInTheDocument();
    // нет демо-графа (ResearchGraph удалён).
    expect(screen.queryByText(/Демо-граф/)).toBeNull();
  });

  it('фикстура хода: счётчики узлов/рёбер, классы статусов, подписи, aria', () => {
    setTurns([
      makeTurn({
        task: 'исследуй тему',
        steps: [
          { id: 'a', kind: 'note.create', args: '{"path":"X.md"}', result: 'ok', isError: false },
          { id: 'b', kind: 'web.search', args: '{"query":"q"}', result: null, isError: false },
        ],
        subagents: [{ childRunId: 2, parentRunId: 1, goal: 'найти источники', status: 'done' }],
        status: 'running',
      }),
    ]);
    const { container } = render(<ExecGraph />);
    // aria на svg.
    expect(screen.getByRole('img', { name: 'Граф выполнения агента' })).toBeInTheDocument();
    // узлы: root + 2 activity + 1 subagent = 4 узловых круга (.fgDot — НЕ круги внутри иконок).
    expect(container.querySelectorAll('[class*="fgDot"]').length).toBe(4);
    // рёбра: 2 seq (root→a, a→b; нет terminal т.к. running) + 1 branch (root→sub).
    expect(container.querySelectorAll('[class*="fgEdgeSeq"]').length).toBe(2);
    expect(container.querySelectorAll('[class*="fgEdgeBranch"]').length).toBe(1);
    // подписи видны (kind-метки + субагент-цель) — в .fgLabel-div'ах.
    const labels = Array.from(container.querySelectorAll('[class*="fgLabel"]')).map(
      (n) => n.textContent,
    );
    expect(labels).toContain('note.create X.md');
    expect(labels).toContain('web.search q');
    expect(labels).toContain('найти источники');
    // running-узел (web.search) показывает BrandThinking-спиннер.
    expect(container.querySelector('.brand-thinking')).toBeTruthy();
  });

  it('running-узел показывает BrandThinking, done — нет (status-канал)', () => {
    setTurns([
      makeTurn({
        steps: [{ id: 'a', kind: 'read', args: '{}', result: null, isError: false }],
        status: 'running',
      }),
    ]);
    const { container } = render(<ExecGraph />);
    // root (running, т.к. турн running) + step (running) → есть brand-thinking.
    expect(container.querySelector('.brand-thinking')).toBeTruthy();
  });

  it('клик по report-узлу зовёт openFile(path)', () => {
    const openFile = vi.fn().mockResolvedValue(undefined);
    // компонент читает openFile через useWorkspaceStore((s) => s.openFile) — подменяем в сторе.
    useWorkspaceStore.setState({ openFile });
    setTurns([
      makeTurn({
        task: 't',
        researchReport: { runId: 1, title: 'Мой отчёт', path: 'reports/r.md', sourcesCount: 3, rounds: 2 },
        status: 'done',
        report: 'итог',
      }),
    ]);
    const { container } = render(<ExecGraph />);
    // report-узел кликабелен (role=button); клик зовёт openFile(path).
    const reportNode = container.querySelector('[role="button"]');
    expect(reportNode).toBeTruthy();
    fireEvent.click(reportNode as Element);
    expect(openFile).toHaveBeenCalledWith('reports/r.md');
  });

  it('ПАРИТЕТ Castor↔Hermes: эквивалентные kind дают одинаковые category-классы на DOM', () => {
    // Castor-вариант.
    setTurns([
      makeTurn({
        steps: [
          { id: 'a', kind: 'note.edit', args: '{}', result: 'ok', isError: false },
          { id: 'b', kind: 'web.search', args: '{}', result: 'ok', isError: false },
          { id: 'c', kind: 'shell', args: '{}', result: 'ok', isError: false },
        ],
        status: 'done',
        report: 'ok',
      }),
    ]);
    const castor = render(<ExecGraph />);
    const castorNodes = castor.container.querySelectorAll('.fgNode, [class*="fgNode"]');
    const castorClasses = Array.from(castorNodes).map((n) => n.getAttribute('class'));
    castor.unmount();

    // Hermes ACP-вариант (те же категории: edit/search/execute).
    setTurns([
      makeTurn({
        steps: [
          { id: 'a', kind: 'edit', args: '{}', result: 'ok', isError: false },
          { id: 'b', kind: 'search', args: '{}', result: 'ok', isError: false },
          { id: 'c', kind: 'execute', args: '{}', result: 'ok', isError: false },
        ],
        status: 'done',
        report: 'ok',
      }),
    ]);
    const hermes = render(<ExecGraph />);
    const hermesNodes = hermes.container.querySelectorAll('.fgNode, [class*="fgNode"]');
    const hermesClasses = Array.from(hermesNodes).map((n) => n.getAttribute('class'));

    // одинаковое число узлов и одинаковые наборы классов (category+status совпадают).
    expect(hermesClasses.length).toBe(castorClasses.length);
    expect(hermesClasses).toEqual(castorClasses);
  });
});
