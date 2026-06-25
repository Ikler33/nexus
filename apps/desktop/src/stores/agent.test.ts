import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { tauriApi, type AgentHistoryMsg, type AgentStreamEvent } from '../lib/tauri-api';
import { useAgentStore } from './agent';

/**
 * W-4: десктоп-чат агента мультитёрный, но прогон — one-shot per run_id. Стор должен слать историю
 * прошлых ходов в `agent.run`, иначе follow-up не помнит контекст и не предлагает правки (changeset-
 * гейт не появлялся на 2-м/3-м сообщении — ST-G3). Тут проверяем КОНТРАКТ FE→бэкенд: история
 * собирается из `turns[]` и передаётся; и что proposal на 2-м ходу рисует changeset.
 */
describe('agent store — мультитёрн история (W-4)', () => {
  type Call = { task: string; history: AgentHistoryMsg[]; onEvent: (e: AgentStreamEvent) => void };
  let calls: Call[];

  beforeEach(() => {
    calls = [];
    useAgentStore.setState({ turns: [], context: null, approving: false });
    vi.spyOn(tauriApi.agent, 'run').mockImplementation((task, _autonomy, onEvent, history = []) => {
      calls.push({ task, history, onEvent });
      return Promise.resolve(calls.length); // runId = порядковый
    });
  });
  afterEach(() => vi.restoreAllMocks());

  it('2-й ход получает историю 1-го (user-задача + assistant-ответ)', async () => {
    // Ход 1 → доводим до терминала (final), чтобы 2-й ход не был no-op (один активный ход за раз).
    useAgentStore.getState().run('создай заметку про оплату');
    expect(calls).toHaveLength(1);
    expect(calls[0].history).toEqual([]); // первый ход — без истории
    calls[0].onEvent({ type: 'final', text: 'Готово: создал черновик «Оплата».' });
    expect(useAgentStore.getState().turns[0].status).toBe('done');

    // Ход 2 → история = [user(задача1), assistant(отчёт1)].
    useAgentStore.getState().run('теперь добавь раздел про кэш');
    expect(calls).toHaveLength(2);
    expect(calls[1].history).toEqual([
      { role: 'user', text: 'создай заметку про оплату' },
      { role: 'assistant', text: 'Готово: создал черновик «Оплата».' },
    ]);
  });

  it('toolCall сохраняет title (ACP/Hermes), без title → null (нативный Кастор)', () => {
    useAgentStore.getState().run('задача');
    // ACP-событие несёт человеко-подпись.
    calls[0].onEvent({
      type: 'toolCall',
      id: 't1',
      kind: 'fetch',
      args: '{"url":"docs.rs"}',
      title: 'Fetching docs.rs',
    });
    // Нативное событие — без title.
    calls[0].onEvent({ type: 'toolCall', id: 't2', kind: 'note.edit', args: '{"path":"A.md"}' });
    const steps = useAgentStore.getState().turns[0].steps;
    expect(steps).toHaveLength(2);
    expect(steps[0].title).toBe('Fetching docs.rs');
    expect(steps[1].title).toBeNull();
  });

  it('proposal на 2-м ходу рисует changeset (status awaiting) этого хода', async () => {
    useAgentStore.getState().run('задача 1');
    calls[0].onEvent({ type: 'final', text: 'ответ 1' });
    useAgentStore.getState().run('задача 2');
    // proposal приходит в onEvent ВТОРОГО хода → changeset + awaiting именно у него.
    calls[1].onEvent({
      type: 'proposal',
      runId: 2,
      files: [{ path: 'A.md', add: 3, del: 0, status: 'new', kind: 'file', actionId: 201 }],
    });
    const turns = useAgentStore.getState().turns;
    expect(turns).toHaveLength(2);
    expect(turns[1].status).toBe('awaiting');
    expect(turns[1].changeset.map((f) => f.path)).toEqual(['A.md']);
    // 1-й ход не затронут поздним proposal-ом 2-го.
    expect(turns[0].changeset).toEqual([]);
  });

  // ACP-EXEC: proposal с exec-файлом → changeset несёт kind:'exec' (карточка рисует его командой).
  it('proposal с exec-permission маппит kind:exec в changeset', async () => {
    useAgentStore.getState().run('собери');
    calls[0].onEvent({
      type: 'proposal',
      runId: 1,
      files: [
        { path: 'A.md', add: 3, del: 0, status: 'new', kind: 'file', actionId: 1 },
        { path: 'cargo build', add: 0, del: 0, status: 'edit', kind: 'exec', actionId: 2 },
      ],
    });
    const cs = useAgentStore.getState().turns[0].changeset;
    expect(cs.map((f) => f.kind)).toEqual(['file', 'exec']);
    expect(cs.find((f) => f.path === 'cargo build')?.kind).toBe('exec');
  });

  // W-4 (ревью): errored/cancelled ход не должен ломать альтернацию ролей (часть LLM-серверов 400-ит
   // на двух user подряд) — даём плейсхолдер-assistant.
  it('errored ход → в истории user+assistant(плейсхолдер), роли строго чередуются', async () => {
    useAgentStore.getState().run('сломанная задача');
    calls[0].onEvent({ type: 'error', message: 'boom' });
    expect(useAgentStore.getState().turns[0].status).toBe('error');
    useAgentStore.getState().run('следующая задача');
    const h = calls[1].history;
    expect(h).toEqual([
      { role: 'user', text: 'сломанная задача' },
      { role: 'assistant', text: '(нет ответа)' },
    ]);
    // Строгая альтернация: соседние сообщения — разных ролей.
    for (let i = 1; i < h.length; i++) expect(h[i].role).not.toBe(h[i - 1].role);
  });

  it('история кэпится последними ходами (бюджет контекста)', async () => {
    // Прогоняем 10 ходов до терминала; 11-й должен унести историю НЕ больше кэпа (8 ходов = 16 сообщений).
    for (let i = 0; i < 10; i++) {
      useAgentStore.getState().run(`задача ${i}`);
      calls[i].onEvent({ type: 'final', text: `ответ ${i}` });
    }
    useAgentStore.getState().run('финальная задача');
    const lastHistory = calls[calls.length - 1].history;
    expect(lastHistory.length).toBeLessThanOrEqual(16);
    // И это именно ХВОСТ: первая запись истории — не «задача 0».
    expect(lastHistory[0]).not.toEqual({ role: 'user', text: 'задача 0' });
  });
});

/**
 * W-23: фронт ДОЛЖЕН принимать ВСЕ варианты контракта `AgentStreamEvent` (план/субагенты/exec/отчёт),
 * а не молча терять их (раньше TS-юнион нёс 8 из 14 → 6 событий бэкенда не разбирались). Тут проверяем,
 * что каждое событие аккумулируется в СВОЁМ ходе. Рендер этих полей — W-24/25/26.
 */
describe('agent store — приём всех событий контракта (W-23)', () => {
  let onEvent: (e: AgentStreamEvent) => void;

  beforeEach(() => {
    useAgentStore.setState({ turns: [], context: null, approving: false });
    vi.spyOn(tauriApi.agent, 'run').mockImplementation((_task, _autonomy, cb) => {
      onEvent = cb;
      return Promise.resolve(1);
    });
    useAgentStore.getState().run('задача');
  });
  afterEach(() => vi.restoreAllMocks());

  const turn = () => useAgentStore.getState().turns[0];

  it('planProposed сохраняет план, planStepStatus обновляет шаг по id', () => {
    onEvent({
      type: 'planProposed',
      runId: 1,
      steps: [
        { id: 'a', label: 'Шаг A', status: 'running' },
        { id: 'b', label: 'Шаг B', status: 'pending' },
      ],
    });
    expect(turn().plan.map((s) => s.id)).toEqual(['a', 'b']);
    onEvent({ type: 'planStepStatus', id: 'a', status: 'done' });
    expect(turn().plan.find((s) => s.id === 'a')?.status).toBe('done');
    // Другой шаг не затронут.
    expect(turn().plan.find((s) => s.id === 'b')?.status).toBe('pending');
  });

  it('subagentStatus делает upsert по childRunId', () => {
    onEvent({
      type: 'subagentStatus',
      parentRunId: 1,
      childRunId: 1001,
      goal: 'подзадача',
      status: 'running',
    });
    expect(turn().subagents).toHaveLength(1);
    expect(turn().subagents[0].status).toBe('running');
    // Повторное событие того же ребёнка — обновляет, не дублирует.
    onEvent({
      type: 'subagentStatus',
      parentRunId: 1,
      childRunId: 1001,
      goal: 'подзадача',
      status: 'done',
      summary: 'итог',
    });
    expect(turn().subagents).toHaveLength(1);
    expect(turn().subagents[0].status).toBe('done');
    expect(turn().subagents[0].summary).toBe('итог');
  });

  it('execProposal заводит запись, execResult проставляет exit-код по actionId', () => {
    onEvent({ type: 'execProposal', runId: 1, actionId: 77, summary: 'shell.run (2 args)' });
    expect(turn().execItems).toHaveLength(1);
    expect(turn().execItems[0]).toMatchObject({ actionId: 77, exitCode: null, finalized: false });
    onEvent({ type: 'execResult', runId: 1, actionId: 77, exitCode: 0, finalized: true });
    expect(turn().execItems).toHaveLength(1); // обновление, не дубль
    expect(turn().execItems[0]).toMatchObject({ actionId: 77, exitCode: 0, finalized: true });
  });

  it('execResult без предложения заводит запись (факт исполнения не теряется)', () => {
    onEvent({ type: 'execResult', runId: 1, actionId: 9, exitCode: 1, finalized: true });
    expect(turn().execItems).toHaveLength(1);
    expect(turn().execItems[0]).toMatchObject({ actionId: 9, exitCode: 1, finalized: true, summary: '' });
  });

  it('report сохраняет карточку отчёта deep-research', () => {
    onEvent({
      type: 'report',
      runId: 1,
      title: 'Отчёт по теме',
      path: 'Research/тема-2026-06-24.md',
      sourcesCount: 12,
      rounds: 3,
    });
    expect(turn().researchReport).toMatchObject({
      title: 'Отчёт по теме',
      path: 'Research/тема-2026-06-24.md',
      sourcesCount: 12,
      rounds: 3,
    });
  });
});

/**
 * ACP: один `request_permission` = ОДНО атомарное решение, поэтому N файлов делят ОДИН actionId.
 * Стор дедуплицирует решения по actionId (одно на группу). «Подтвердить» — УТВЕРДИТЕЛЬНОЕ действие:
 * одобряет всё, что НЕ отклонено ЯВНО (applied И undefined → approve), любой ЯВНО rejected файл в
 * группе → reject ВСЕЙ permission (нельзя частично одобрить атомарную permission). БЕЗОПАСНОСТЬ:
 * нельзя авто-одобрить файл, который пользователь явно отклонил; «не подтвердили вовсе» fail-close'ится
 * на бэкенде (незавершённый ход → pending → Cancelled).
 */
describe('agent store — atomic ACP-permission approve (общий actionId)', () => {
  afterEach(() => vi.restoreAllMocks());

  const seed = (d0?: 'applied' | 'rejected', d1?: 'applied' | 'rejected') => {
    useAgentStore.setState({
      approving: false,
      turns: [
        {
          key: 0,
          epoch: 1,
          runId: 1,
          task: 'acp',
          assistantText: '',
          steps: [],
          changeset: [
            { path: 'A.md', add: 1, del: 0, status: 'new', kind: 'file', actionId: 300, decision: d0 },
            { path: 'B.md', add: 2, del: 1, status: 'edit', kind: 'file', actionId: 300, decision: d1 },
          ],
          plan: [],
          subagents: [],
          execItems: [],
          researchReport: null,
          report: null,
          error: null,
          status: 'awaiting',
        },
      ],
    });
  };

  it('оба applied → ОДНО решение approve=true (дедуп, не два)', async () => {
    const spy = vi.spyOn(tauriApi.agent, 'approve').mockResolvedValue(undefined);
    seed('applied', 'applied');
    await useAgentStore.getState().approve();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][1]).toEqual([{ actionId: 300, approve: true }]);
  });

  it('частичный reject → reject ВСЕЙ группы (fail-closed AND, не авто-одобрить отклонённый)', async () => {
    const spy = vi.spyOn(tauriApi.agent, 'approve').mockResolvedValue(undefined);
    seed('applied', 'rejected');
    await useAgentStore.getState().approve();
    expect(spy.mock.calls[0][1]).toEqual([{ actionId: 300, approve: false }]);
  });

  it('нерешённые строки (без явного reject) → approve (утвердительное «Подтвердить»)', async () => {
    // Регрессия: клик «Подтвердить» без пометки строк ДОЛЖЕН одобрить permission, а не отклонить.
    const spy = vi.spyOn(tauriApi.agent, 'approve').mockResolvedValue(undefined);
    seed(undefined, undefined);
    await useAgentStore.getState().approve();
    expect(spy.mock.calls[0][1]).toEqual([{ actionId: 300, approve: true }]);
  });

  it('applied + нерешённый (без явного reject) → approve всей группы', async () => {
    const spy = vi.spyOn(tauriApi.agent, 'approve').mockResolvedValue(undefined);
    seed('applied', undefined);
    await useAgentStore.getState().approve();
    expect(spy.mock.calls[0][1]).toEqual([{ actionId: 300, approve: true }]);
  });
});
