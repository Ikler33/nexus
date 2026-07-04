import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import * as mockAgent from './agent';
import { streamChat } from './vault';
import type { AgentStreamEvent, ChatStreamEvent } from '../tauri-api';

/**
 * P0-2 — гейт мок-паритета (защита от дрейфа, урок mock-must-match-backend / MEM-5):
 * (а) мок агента обязан эмитить КАЖДЫЙ вариант юниона `AgentStreamEvent`;
 * (б) мок чата обязан эмитить КАЖДЫЙ вариант юниона `ChatStreamEvent`;
 * (в) число литеральных инлайн-моков в tauri-api.ts не растёт (полная миграция — стадия F-2).
 *
 * Механика (а)/(б): список вариантов выводится ИЗ ТИПА через `satisfies Record<Union['type'], true>`
 * — новый вариант юниона ЛОМАЕТ typecheck этого файла (заставляя внести его в список), после чего
 * runtime-тест КРАСНЫЙ, пока мок не начнёт вариант эмитить. Двухступенчатый капкан: дыра не может
 * появиться молча ни в типе, ни в моке.
 */

// ── Список вариантов агент-стрима: НЕ руками из головы, а под контролем типа. ────────────────────
const AGENT_EVENT_TYPES = Object.keys({
  assistantToken: true,
  toolCall: true,
  toolResult: true,
  contextUsage: true,
  proposal: true,
  diff: true,
  final: true,
  error: true,
  execProposal: true,
  execResult: true,
  planProposed: true,
  planStepStatus: true,
  subagentStatus: true,
  report: true,
} satisfies Record<AgentStreamEvent['type'], true>) as AgentStreamEvent['type'][];

// ── Список вариантов чат-стрима (все, что бэкенд-`chat_rag` способен слать). ─────────────────────
const CHAT_EVENT_TYPES = Object.keys({
  sources: true,
  webSources: true,
  memorySources: true,
  episodeSources: true,
  token: true,
  reasoning: true,
  reasoningSummary: true,
  done: true,
  error: true,
} satisfies Record<ChatStreamEvent['type'], true>) as ChatStreamEvent['type'][];

/** Поллит условие с таймаутом (STEP_MS мока = 8мс — сценарии быстрые, но недетерминированно). */
async function until(cond: () => boolean, ms = 3000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error('условие не наступило за таймаут');
    await new Promise((r) => setTimeout(r, 10));
  }
}

/** Прогоняет чат-мок до терминального события (done|error), возвращает все события. */
function chatScenario(question: string, opts?: Parameters<typeof streamChat>[2]) {
  return new Promise<ChatStreamEvent[]>((resolve, reject) => {
    const events: ChatStreamEvent[] = [];
    const t = setTimeout(() => reject(new Error('чат-мок не завершился за таймаут')), 5000);
    streamChat(
      question,
      (e) => {
        events.push(e);
        if (e.type === 'done' || e.type === 'error') {
          clearTimeout(t);
          resolve(events);
        }
      },
      opts,
    );
  });
}

const sortedUnique = (types: string[]) => [...new Set(types)].sort();

beforeEach(() => mockAgent.__reset());
afterEach(() => mockAgent.__reset());

describe('P0-2 гейт (а): мок агента эмитит каждый вариант юниона AgentStreamEvent', () => {
  it('объединение эмиссий сценариев == все варианты юниона (равенство множеств)', async () => {
    const events: AgentStreamEvent[] = [];
    const onEvent = (e: AgentStreamEvent) => events.push(e);
    const types = () => events.map((e) => e.type);

    // Сценарий 1 (основной, confirm): триггеры exec+report дают полную ленту хода —
    // токены → tool-пара → план/субагент → exec-пара → contextUsage → proposal/diff → report → final.
    const runId = await mockAgent.run('exec report: полный прогон', 'confirm', onEvent);
    await until(() => types().includes('proposal'));
    const proposal = events.find((e) => e.type === 'proposal');
    if (proposal?.type !== 'proposal') throw new Error('нет proposal');
    await mockAgent.approve(
      runId,
      proposal.files.map((f) => ({ actionId: f.actionId, approve: true })),
    );
    await until(() => types().includes('final'));

    // Сценарий 2: терминальная ошибка хода (провайдер упал) — единственный вариант вне сценария 1.
    // Узкий демо-маркер: обычное слово «ошибка» в задаче мок не роняет (анти-футган смоука).
    await mockAgent.run('демо-ошибка провайдера', 'confirm', onEvent);
    await until(() => types().includes('error'));

    // Равенство МНОЖЕСТВ: и «мок не эмитит вариант» (дыра), и «эмитит неизвестный тип» — красный.
    expect(sortedUnique(types())).toEqual(sortedUnique(AGENT_EVENT_TYPES));
  });
});

describe('P0-2 гейт (б): мок чата эмитит каждый вариант юниона ChatStreamEvent', () => {
  it('объединение эмиссий сценариев == все варианты юниона (равенство множеств)', async () => {
    const all: ChatStreamEvent[] = [];
    // Vault-режим с памятью/эпизодами/Глубоким: sources → episodeSources → memorySources →
    // reasoning/reasoningSummary → token → done.
    all.push(...(await chatScenario('Roadmap', { episodic: true, deep: true })));
    // Web-режим: webSources (без sources — web-план замещает RAG-ветку, как в chat_rag).
    all.push(...(await chatScenario('что нового в мире', { web: true })));
    // Терминальная ошибка (форма Error{message, deniedKind?}) — узкий демо-маркер.
    all.push(...(await chatScenario('демо-ошибка провайдера')));

    expect(sortedUnique(all.map((e) => e.type))).toEqual(sortedUnique(CHAT_EVENT_TYPES));
  });
});

describe('P0-2 гейт (в): литеральные инлайн-моки в API-слое не прибавляются', () => {
  it('число `Promise.resolve(`-веток не больше baseline', () => {
    // Baseline-история ratchet'а ВНИЗ: 23 (P0-2, 2026-07-02) → 19 (F-2a, 2026-07-03: vault-домен
    // вынесен в lib/api/vault/, его 4 инлайн-мока — vault.rescan/notesCount/fileMtime +
    // attachments.write — переехали в mock/vault.ts) → 18 (F-2b, 2026-07-04: chat-домен вынесен
    // в lib/api/chat/, его 1 инлайн-мок — chat.sessions.toNote — переехал в mock/sessions.ts).
    // Скоуп подсчёта с F-2a — баррел tauri-api.ts ПЛЮС весь новый слой lib/api/** (иначе
    // инлайн-моки могли бы тихо копиться в доменных модулях мимо храповика). Полная миграция
    // остатка — срезы F-2c+; до них тест держит планку «не хуже»: новый инлайн-мок — красный тест
    // (новые моки обязаны жить в mock/* и зеркалить контракт). Мигрировали часть — ПОНИЗЬТЕ baseline.
    const INLINE_MOCK_BASELINE = 18;
    // cwd vitest = apps/desktop (vitest.config там) — import.meta.url в jsdom не file-схема.
    const files = [join(process.cwd(), 'src/lib/tauri-api.ts'), ...tsFilesUnder(join(process.cwd(), 'src/lib/api'))];
    const count = files
      .map((f) => (readFileSync(f, 'utf8').match(/Promise\.resolve\(/g) ?? []).length)
      .reduce((a, b) => a + b, 0);
    expect(count).toBeLessThanOrEqual(INLINE_MOCK_BASELINE);
  });
});

/** Все .ts/.tsx под каталогом (рекурсивно), без тестов — скоуп подсчёта инлайн-моков API-слоя. */
function tsFilesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true, recursive: true })
    .filter((e) => e.isFile() && /\.tsx?$/.test(e.name) && !/\.(test|spec)\.tsx?$/.test(e.name))
    .map((e) => join(e.parentPath, e.name));
}
