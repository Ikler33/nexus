import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import * as mockAgent from './agent';
import type { AgentStreamEvent } from '../tauri-api';

/**
 * Мок ОБЯЗАН зеркалить контракт UI-1a (`Channel<AgentStreamEvent>`): те же формы событий, тот же порядок
 * эмиссии, та же семантика approve. Эти тесты — «check-mock-зеркало»: если мок начнёт врать (другой
 * порядок / поля), превью и UI-тесты подтверждали бы неверное поведение (урок mock-must-match-backend).
 */

function collect() {
  const events: AgentStreamEvent[] = [];
  return { events, onEvent: (e: AgentStreamEvent) => events.push(e) };
}

const typesOf = (events: AgentStreamEvent[]) => events.map((e) => e.type);

/** Поллит условие вместо фикс-слипов (STEP_MS мока = 8мс, но порядок недетерминирован по времени). */
async function until(cond: () => boolean, ms = 3000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error('условие не наступило за таймаут');
    await new Promise((r) => setTimeout(r, 10));
  }
}

beforeEach(() => mockAgent.__reset());
afterEach(() => mockAgent.__reset());

describe('mock/agent — зеркало контракта UI-1a', () => {
  it('confirm: порядок assistantToken → toolCall → toolResult → contextUsage → proposal → diff… (ждёт approve до final)', async () => {
    const { events, onEvent } = collect();
    const runId = await mockAgent.run('задача', 'confirm', onEvent);

    // Без approve гейт висит на proposal — final НЕ приходит (fail-closed, как UiDecisionSource).
    await new Promise((r) => setTimeout(r, 400));
    const types = typesOf(events);
    expect(types).toContain('assistantToken');
    expect(types).toContain('toolCall');
    expect(types).toContain('toolResult');
    expect(types).toContain('contextUsage');
    expect(types).toContain('proposal');
    expect(types).toContain('diff');
    expect(types).not.toContain('final'); // ещё ждёт решения

    // Порядок ключевых событий (контракт): assistantToken до toolCall до toolResult до proposal.
    const pos = (t: AgentStreamEvent['type']) => types.indexOf(t);
    expect(pos('assistantToken')).toBeLessThan(pos('toolCall'));
    expect(pos('toolCall')).toBeLessThan(pos('toolResult'));
    expect(pos('toolResult')).toBeLessThan(pos('proposal'));
    expect(pos('proposal')).toBeLessThan(pos('diff'));

    // Approve разблокирует гейт → приходит final.
    const proposal = events.find((e) => e.type === 'proposal');
    expect(proposal?.type).toBe('proposal');
    if (proposal?.type !== 'proposal') throw new Error('нет proposal');
    await mockAgent.approve(
      runId,
      proposal.files.map((f) => ({ actionId: f.actionId, approve: true })),
    );
    await new Promise((r) => setTimeout(r, 50));
    expect(typesOf(events)).toContain('final');
  });

  it('формы событий зеркалят AgentStreamEvent (toolCall.id == toolResult.id; proposal.files несут actionId/status)', async () => {
    const { events, onEvent } = collect();
    await mockAgent.run('задача', 'confirm', onEvent);
    await new Promise((r) => setTimeout(r, 400));

    const call = events.find((e) => e.type === 'toolCall');
    const result = events.find((e) => e.type === 'toolResult');
    if (call?.type !== 'toolCall' || result?.type !== 'toolResult') throw new Error('нет call/result');
    expect(call.id).toBe(result.id); // корреляция по id (контракт)
    expect(typeof call.kind).toBe('string');
    expect(typeof call.args).toBe('string');
    expect(result.isError).toBe(false);

    const proposal = events.find((e) => e.type === 'proposal');
    if (proposal?.type !== 'proposal') throw new Error('нет proposal');
    expect(proposal.runId).toBeGreaterThan(0);
    for (const f of proposal.files) {
      expect(typeof f.path).toBe('string');
      expect(typeof f.add).toBe('number');
      expect(typeof f.del).toBe('number');
      expect(['new', 'edit']).toContain(f.status);
      expect(['file', 'exec']).toContain(f.kind); // ACP-EXEC: род действия на проводе
      expect(typeof f.actionId).toBe('number');
    }
    // ACP-EXEC: мок несёт хотя бы один exec-permission (без ±строк) и файлы.
    expect(proposal.files.some((f) => f.kind === 'exec')).toBe(true);
    expect(proposal.files.some((f) => f.kind === 'file')).toBe(true);

    const usage = events.find((e) => e.type === 'contextUsage');
    if (usage?.type !== 'contextUsage') throw new Error('нет contextUsage');
    expect(usage.used).toBeGreaterThan(0);
    expect(usage.window).toBeGreaterThan(usage.used);
  });

  it('auto: дифы без proposal (Auto-тир) → final без ожидания approve', async () => {
    const { events, onEvent } = collect();
    await mockAgent.run('задача', 'auto', onEvent);
    await new Promise((r) => setTimeout(r, 400));
    const types = typesOf(events);
    expect(types).toContain('diff');
    expect(types).not.toContain('proposal'); // auto не предлагает — применяет
    expect(types).toContain('final');
  });

  it('cancel обрывает стрим (final не приходит)', async () => {
    const { events, onEvent } = collect();
    const runId = await mockAgent.run('задача', 'auto', onEvent);
    await new Promise((r) => setTimeout(r, 20));
    await mockAgent.cancel(runId);
    await new Promise((r) => setTimeout(r, 200));
    expect(typesOf(events)).not.toContain('final');
  });

  it('undo возвращает число применённых действий', async () => {
    const { events, onEvent } = collect();
    const runId = await mockAgent.run('задача', 'auto', onEvent);
    await new Promise((r) => setTimeout(r, 400));
    // auto применил все 3 файла.
    expect(typesOf(events)).toContain('final');
    expect(await mockAgent.undo(runId)).toBe(3);
    // Идемпотентно: повтор — 0.
    expect(await mockAgent.undo(runId)).toBe(0);
  });
});

// P0-2: редкие варианты юниона — триггеры по тексту задачи. Формы сверены с Rust wire.rs поле-в-поле
// (runId/actionId/exitCode/sourcesCount — явный camelCase; см. `agent::connect::wire::AgentStreamEvent`).
describe('mock/agent P0-2 — exec/report/error по триггерам задачи', () => {
  it('«exec»: пара execProposal→execResult после плана, до proposal; формы wire байт-в-байт', async () => {
    const { events, onEvent } = collect();
    const runId = await mockAgent.run('exec: собери проект', 'confirm', onEvent);
    await until(() => typesOf(events).includes('proposal'));

    const types = typesOf(events);
    const pos = (t: AgentStreamEvent['type']) => types.indexOf(t);
    // Реалистичный порядок цикла: exec-вызов идёт ВНУТРИ хода (после плана), до end-of-turn changeset.
    expect(pos('planProposed')).toBeLessThan(pos('execProposal'));
    expect(pos('execProposal')).toBeLessThan(pos('execResult'));
    expect(pos('execResult')).toBeLessThan(pos('proposal'));

    const ep = events.find((e) => e.type === 'execProposal');
    const er = events.find((e) => e.type === 'execResult');
    if (ep?.type !== 'execProposal' || er?.type !== 'execResult') throw new Error('нет exec-пары');
    // Корреляция пары и прогона (как run_id/action_id ledger'а).
    expect(ep.runId).toBe(runId);
    expect(er.runId).toBe(runId);
    expect(er.actionId).toBe(ep.actionId);
    // Силуэт-приватность §5.6: summary — строка-силуэт; результат — exit-код + finalized, БЕЗ stdout.
    expect(ep.summary.length).toBeGreaterThan(0);
    expect(er.exitCode).toBe(0);
    expect(er.finalized).toBe(true);
    // Байт-в-байт имена ключей провода (никаких лишних/переименованных полей).
    expect(Object.keys(ep).sort()).toEqual(['actionId', 'runId', 'summary', 'type']);
    expect(Object.keys(er).sort()).toEqual(['actionId', 'exitCode', 'finalized', 'runId', 'type']);
  });

  it('«отчёт»: report после фазы changeset, перед final; форма wire байт-в-байт', async () => {
    const { events, onEvent } = collect();
    const runId = await mockAgent.run('подготовь отчёт по теме', 'auto', onEvent);
    await until(() => typesOf(events).includes('final'));

    const types = typesOf(events);
    expect(types.indexOf('diff')).toBeLessThan(types.indexOf('report')); // после changeset
    expect(types.indexOf('report')).toBeLessThan(types.indexOf('final')); // ближе к финалу

    const rep = events.find((e) => e.type === 'report');
    if (rep?.type !== 'report') throw new Error('нет report');
    expect(rep.runId).toBe(runId);
    expect(rep.title.length).toBeGreaterThan(0);
    expect(rep.path.endsWith('.md')).toBe(true);
    expect(rep.sourcesCount).toBeGreaterThan(0);
    expect(rep.rounds).toBeGreaterThan(0);
    expect(Object.keys(rep).sort()).toEqual(['path', 'rounds', 'runId', 'sourcesCount', 'title', 'type']);
  });

  it('«ошибка»: терминальный error — final НЕ приходит (как в реальном цикле)', async () => {
    const { events, onEvent } = collect();
    await mockAgent.run('ошибка провайдера', 'auto', onEvent);
    await until(() => typesOf(events).includes('error'));
    await new Promise((r) => setTimeout(r, 100));

    const types = typesOf(events);
    expect(types.at(-1)).toBe('error'); // error терминален
    expect(types).not.toContain('final');
    const err = events.find((e) => e.type === 'error');
    if (err?.type !== 'error') throw new Error('нет error');
    expect(err.message.length).toBeGreaterThan(0);
    expect(Object.keys(err).sort()).toEqual(['message', 'type']);
  });

  it('дефолтная задача БЕЗ триггеров: exec/report/error не эмитятся (сценарии превью не тронуты)', async () => {
    const { events, onEvent } = collect();
    await mockAgent.run('задача', 'auto', onEvent);
    await until(() => typesOf(events).includes('final'));
    const types = typesOf(events);
    for (const t of ['execProposal', 'execResult', 'report', 'error'] as const) {
      expect(types).not.toContain(t);
    }
  });
});
