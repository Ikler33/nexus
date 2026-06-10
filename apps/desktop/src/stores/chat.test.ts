import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../lib/tauri-api';
import { useChatStore } from './chat';

// В vitest (не Tauri) `streamRag` проксируется в мок `mock/vault.streamChat` (sources→токены→done).
// jsdom под node 25 не отдаёт рабочий localStorage — мокаем in-memory (свежий на тест) для персиста (#17).
beforeEach(() => {
  const ls = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (ls.has(k) ? (ls.get(k) as string) : null),
    setItem: (k: string, v: string) => void ls.set(k, String(v)),
    removeItem: (k: string) => void ls.delete(k),
    clear: () => ls.clear(),
  });
  useChatStore.getState().hydrate(null); // сброс vaultKey + messages
  useChatStore.setState({ streaming: false, grounded: true });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('chat store (Ф1-8)', () => {
  it('send: вопрос → user+assistant, стрим → готовый ответ с источниками', async () => {
    useChatStore.getState().send('Roadmap');

    const initial = useChatStore.getState();
    expect(initial.messages).toHaveLength(2);
    expect(initial.messages[0]).toMatchObject({ role: 'user', content: 'Roadmap' });
    expect(initial.messages[1].role).toBe('assistant');
    expect(initial.streaming).toBe(true);

    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
      timeout: 2000,
    });

    const reply = useChatStore.getState().messages[1];
    expect(reply.streaming).toBeFalsy();
    expect(reply.content.length).toBeGreaterThan(0);
    expect(reply.sources?.length ?? 0).toBeGreaterThan(0);
  });

  it('пустой вопрос игнорируется', () => {
    useChatStore.getState().send('   ');
    expect(useChatStore.getState().messages).toHaveLength(0);
    expect(useChatStore.getState().streaming).toBe(false);
  });

  it('vault-режим (дефолт) шлёт grounded:true; общий — grounded:false (V4.4)', () => {
    const spy = vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});

    useChatStore.getState().send('вопрос');
    expect(spy).toHaveBeenLastCalledWith(
      'вопрос',
      expect.any(Function),
      expect.objectContaining({ grounded: true }),
    );

    useChatStore.setState({ messages: [], streaming: false });
    useChatStore.getState().setGrounded(false);
    expect(useChatStore.getState().grounded).toBe(false);
    useChatStore.getState().send('привет');
    expect(spy).toHaveBeenLastCalledWith(
      'привет',
      expect.any(Function),
      expect.objectContaining({ grounded: false }),
    );
  });

  it('общий чат: ретрив не вызывается → ответ без источников (V4.4)', async () => {
    useChatStore.getState().setGrounded(false);
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
      timeout: 2000,
    });
    const reply = useChatStore.getState().messages[1];
    expect(reply.content.length).toBeGreaterThan(0);
    expect(reply.sources?.length ?? 0).toBe(0); // общий режим источников не возвращает
  });

  it('setGrounded игнорируется во время стрима', () => {
    vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.getState().send('Roadmap'); // streaming=true
    useChatStore.getState().setGrounded(false);
    expect(useChatStore.getState().grounded).toBe(true); // на лету не переключается
  });

  it('stop прекращает стрим', () => {
    useChatStore.getState().send('Roadmap');
    expect(useChatStore.getState().streaming).toBe(true);
    useChatStore.getState().stop();
    expect(useChatStore.getState().streaming).toBe(false);
    expect(useChatStore.getState().messages.every((m) => !m.streaming)).toBe(true);
  });

  it('clear очищает сессию после завершения', async () => {
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
      timeout: 2000,
    });
    useChatStore.getState().clear();
    expect(useChatStore.getState().messages).toHaveLength(0);
  });

  it('троттлит рендер токенов: N токенов коалесятся в один кадр (AC-Б10-4)', () => {
    const rafCbs: FrameRequestCallback[] = [];
    const rafSpy = vi.fn((cb: FrameRequestCallback) => {
      rafCbs.push(cb);
      return rafCbs.length;
    });
    vi.stubGlobal('requestAnimationFrame', rafSpy);
    vi.stubGlobal('cancelAnimationFrame', vi.fn());

    const N = 200;
    vi.spyOn(tauriApi.chat, 'streamRag').mockImplementation((_q, onEvent) => {
      onEvent({ type: 'sources', sources: [] });
      for (let i = 0; i < N; i++) onEvent({ type: 'token', text: 'x' });
      return () => {};
    });

    useChatStore.getState().send('вопрос');

    // N токенов → rAF запланирован ОДИН раз (коалесинг), не N → ≤N ре-рендеров (AC-Б10-4).
    expect(rafSpy).toHaveBeenCalledTimes(1);
    // Буфер ещё не применён к стейту (кадр не сработал) — токены не текут по одному.
    const mid = useChatStore.getState().messages.find((m) => m.role === 'assistant');
    expect(mid?.content).toBe('');

    // Прогоняем кадр → весь буфер применяется ОДНИМ апдейтом.
    rafCbs.forEach((cb) => cb(0));
    const after = useChatStore.getState().messages.find((m) => m.role === 'assistant');
    expect(after?.content).toBe('x'.repeat(N));
  });

  it('персист: история сохраняется и восстанавливается через hydrate (#17)', async () => {
    useChatStore.getState().hydrate('/vault/A'); // открыли vault A
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), { timeout: 2000 });
    expect(useChatStore.getState().messages.length).toBeGreaterThanOrEqual(2);

    // Симулируем перезапуск: стираем стейт в памяти, затем hydrate того же vault.
    useChatStore.setState({ messages: [] });
    useChatStore.getState().hydrate('/vault/A');
    const restored = useChatStore.getState().messages;
    expect(restored.length).toBeGreaterThanOrEqual(2);
    expect(restored[0]).toMatchObject({ role: 'user', content: 'Roadmap' });
    expect(restored.every((m) => !m.streaming)).toBe(true); // стрим-флаги сняты при загрузке
  });

  it('персист: у разных vault раздельные истории (#17)', async () => {
    useChatStore.getState().hydrate('/vault/A');
    useChatStore.getState().send('вопрос про A');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), { timeout: 2000 });

    // Переключение на vault B → пусто (история A не протекает).
    useChatStore.getState().hydrate('/vault/B');
    expect(useChatStore.getState().messages).toHaveLength(0);

    // Возврат к A → история на месте.
    useChatStore.getState().hydrate('/vault/A');
    expect(useChatStore.getState().messages[0]).toMatchObject({ content: 'вопрос про A' });
  });

  it('reasoning (R1): сводка патчится в сообщение; событие raw `reasoning` игнорируется', () => {
    vi.spyOn(tauriApi.chat, 'streamRag').mockImplementation((_q, onEvent) => {
      onEvent({ type: 'sources', sources: [] });
      onEvent({ type: 'reasoningSummary', text: 'Анализирую' });
      onEvent({ type: 'reasoning', text: 'сырой CoT, который не показываем' });
      onEvent({ type: 'reasoningSummary', text: 'Формулирую ответ' });
      return () => {};
    });

    useChatStore.getState().send('вопрос');

    const reply = useChatStore.getState().messages.find((m) => m.role === 'assistant');
    expect(reply?.reasoningSummary).toBe('Формулирую ответ'); // живёт последняя сводка
    // Сырой CoT нигде не оседает — событие принято и проигнорировано.
    expect(JSON.stringify(reply)).not.toContain('сырой CoT');
  });

  it('reasoning (R1): живая сводка НЕ персистится (эфемерна), ответ — да', async () => {
    useChatStore.getState().hydrate('/vault/R');
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), { timeout: 2000 });

    // «Перезапуск» → hydrate: сводка не восстанавливается, ответ — да.
    useChatStore.setState({ messages: [] });
    useChatStore.getState().hydrate('/vault/R');
    const restored = useChatStore.getState().messages.find((m) => m.role === 'assistant');
    expect(restored?.content.length).toBeGreaterThan(0);
    expect(restored?.reasoningSummary).toBeUndefined();
  });

  it('персист: clear сохраняет пустую историю (#17)', async () => {
    useChatStore.getState().hydrate('/vault/A');
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), { timeout: 2000 });
    useChatStore.getState().clear();

    // «Перезапуск» с мусором в памяти → hydrate возвращает пусто (clear персистнул []).
    useChatStore.setState({ messages: [{ id: 'x', role: 'user', content: 'stale' }] });
    useChatStore.getState().hydrate('/vault/A');
    expect(useChatStore.getState().messages).toHaveLength(0);
  });

  // Аудит 2026-06-10: смена vault ПОСРЕДИ стрима — осиротевший стрим дорезается ДО смены ключа:
  // хвост финализируется в историю СТАРОГО vault (не утекает в новый), новый vault чист.
  it('hydrate при активном стриме: стрим дорезан, история не утекает между vault', async () => {
    useChatStore.getState().hydrate('/vault/A');
    useChatStore.getState().send('вопрос про A');
    expect(useChatStore.getState().streaming).toBe(true);

    // Переключаемся на B, НЕ дожидаясь конца стрима.
    useChatStore.getState().hydrate('/vault/B');
    expect(useChatStore.getState().streaming).toBe(false);
    expect(useChatStore.getState().messages).toHaveLength(0); // B чист — стрим A не протёк

    // История A финализирована ПОД КЛЮЧОМ A (вопрос + дорезанный ответ без стрим-флагов).
    useChatStore.getState().hydrate('/vault/A');
    const a = useChatStore.getState().messages;
    expect(a[0]).toMatchObject({ role: 'user', content: 'вопрос про A' });
    expect(a.every((m) => !m.streaming)).toBe(true);
  });
});
