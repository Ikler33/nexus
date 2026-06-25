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
