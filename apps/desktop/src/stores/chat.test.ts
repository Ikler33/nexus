import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { tauriApi, type ChatStreamEvent } from '../lib/tauri-api';
import { __reset as resetMockSessions } from '../lib/mock/sessions';
import { disclosureOpen, useChatStore } from './chat';
import { usePrefsStore } from './prefs';

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
  resetMockSessions();
  useChatStore.getState().hydrate(null); // сброс контекста vault + ленты
  useChatStore.setState({ streaming: false, mode: 'vault', pinned: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('chat store (Ф1-8)', () => {
  // P6-PIN: закрепление заметок в контекст.
  it('togglePin закрепляет/открепляет путь', () => {
    useChatStore.getState().togglePin('A.md');
    expect(useChatStore.getState().pinned).toEqual(['A.md']);
    useChatStore.getState().togglePin('B.md');
    expect(useChatStore.getState().pinned).toEqual(['A.md', 'B.md']);
    useChatStore.getState().togglePin('A.md'); // повтор — открепляет
    expect(useChatStore.getState().pinned).toEqual(['B.md']);
  });

  it('togglePin: кап PIN_MAX=5 (старейший вытесняется)', () => {
    for (let i = 0; i < 6; i++) useChatStore.getState().togglePin(`N${i}.md`);
    const p = useChatStore.getState().pinned;
    expect(p).toHaveLength(5);
    expect(p).not.toContain('N0.md'); // старейший вытеснен
    expect(p[4]).toBe('N5.md');
  });

  it('togglePin/clearPins — no-op во время стрима', () => {
    useChatStore.setState({ pinned: ['A.md'], streaming: true });
    useChatStore.getState().togglePin('B.md');
    useChatStore.getState().clearPins();
    expect(useChatStore.getState().pinned).toEqual(['A.md']); // заморожено
  });

  it('send передаёт pinned в streamRag', () => {
    const spy = vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.setState({ pinned: ['Pinned.md'] });
    useChatStore.getState().send('вопрос');
    expect(spy).toHaveBeenCalledWith(
      'вопрос',
      expect.any(Function),
      expect.objectContaining({ pinned: ['Pinned.md'] }),
    );
    useChatStore.getState().stop();
  });

  it('hydrate (смена vault) чистит pinned — нет кросс-vault утечки', () => {
    useChatStore.setState({ pinned: ['A.md', 'B.md'] });
    useChatStore.getState().hydrate('/vault/B');
    expect(useChatStore.getState().pinned).toEqual([]);
  });

  it('dropPinsUnder открепляет удалённый путь/поддерево (CURATE)', () => {
    useChatStore.setState({ pinned: ['Notes/Idea.md', 'Notes/Sub/X.md', 'Other.md'] });
    useChatStore.getState().dropPinsUnder('Notes');
    expect(useChatStore.getState().pinned).toEqual(['Other.md']);
  });

  it('renamePins переписывает закреплённые пути (CURATE)', () => {
    useChatStore.setState({ pinned: ['Old.md', 'Other.md'] });
    useChatStore.getState().renamePins('Old.md', 'New.md');
    expect(useChatStore.getState().pinned).toEqual(['New.md', 'Other.md']);
  });

  it('regenerate: убирает последний обмен, переспрашивает, чистит историю сессии', async () => {
    const del = vi.spyOn(tauriApi.chat.sessions, 'deleteLastExchange').mockResolvedValue();
    const stream = vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Вопрос' },
        { id: 'a1', role: 'assistant', content: 'Старый ответ' },
      ],
      sessionId: 42,
      streaming: false,
    });
    useChatStore.getState().regenerate(); // sync-сигнатура; работа в async-IIFE (ждёт lastSave)
    await vi.waitFor(() => expect(del).toHaveBeenCalledWith(42)); // прошлая пара убрана из истории
    const msgs = useChatStore.getState().messages;
    expect(msgs).toHaveLength(2); // переспрошенный вопрос + новый ответ (не накопили)
    expect(msgs[0]).toMatchObject({ role: 'user', content: 'Вопрос' });
    expect(msgs[1].role).toBe('assistant');
    expect(stream).toHaveBeenCalledWith('Вопрос', expect.any(Function), expect.any(Object));
    useChatStore.getState().stop();
  });

  it('regenerate на ОШИБОЧНОМ ответе не зовёт deleteLastExchange (прошлый хороший обмен цел)', async () => {
    const del = vi.spyOn(tauriApi.chat.sessions, 'deleteLastExchange').mockResolvedValue();
    vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Вопрос' },
        { id: 'a1', role: 'assistant', content: '', error: 'сеть упала' },
      ],
      sessionId: 42,
      streaming: false,
    });
    useChatStore.getState().regenerate();
    // Ошибочный обмен не персистился → не чистим БД (иначе снесли бы предыдущий хороший). Переспрос идёт.
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(true));
    expect(del).not.toHaveBeenCalled();
    useChatStore.getState().stop();
  });

  it('regenerate — no-op во время стрима и без завершённого обмена', () => {
    const stream = vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.setState({ messages: [{ id: 'u1', role: 'user', content: 'q' }], streaming: false });
    useChatStore.getState().regenerate(); // нет пары user+assistant
    expect(stream).not.toHaveBeenCalled();
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'q' },
        { id: 'a1', role: 'assistant', content: 'a' },
      ],
      streaming: true,
    });
    useChatStore.getState().regenerate(); // идёт стрим
    expect(stream).not.toHaveBeenCalled();
  });

  it('send без закреплений — pinned undefined', () => {
    const spy = vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.getState().send('вопрос');
    expect(spy).toHaveBeenCalledWith(
      'вопрос',
      expect.any(Function),
      expect.objectContaining({ pinned: undefined }),
    );
    useChatStore.getState().stop();
  });

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

  it('режимы: vault → grounded:true; general → grounded:false; web → web:true', () => {
    const spy = vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});

    useChatStore.getState().send('вопрос');
    expect(spy).toHaveBeenLastCalledWith(
      'вопрос',
      expect.any(Function),
      expect.objectContaining({ grounded: true, web: false }),
    );

    useChatStore.setState({ messages: [], streaming: false, web: false });
    useChatStore.getState().setMode('general');
    expect(useChatStore.getState().mode).toBe('general');
    useChatStore.getState().send('привет');
    expect(spy).toHaveBeenLastCalledWith(
      'привет',
      expect.any(Function),
      expect.objectContaining({ grounded: false, web: false }),
    );

    // Web — флаг ПОВЕРХ режима (ревизия 11.06): не сбрасывает выбранный режим.
    useChatStore.setState({ messages: [], streaming: false, web: false });
    useChatStore.getState().setMode('vault');
    useChatStore.getState().toggleWeb();
    expect(useChatStore.getState().mode).toBe('vault');
    useChatStore.getState().send('что нового');
    expect(spy).toHaveBeenLastCalledWith(
      'что нового',
      expect.any(Function),
      expect.objectContaining({ grounded: true, web: true }),
    );
    useChatStore.setState({ streaming: false }); // стрим-гард: во время стрима тоггл заморожен
    useChatStore.getState().toggleWeb(); // выкл — режим остался vault
    expect(useChatStore.getState().web).toBe(false);
    expect(useChatStore.getState().mode).toBe('vault');
  });

  it('общий чат: ретрив не вызывается → ответ без источников (V4.4)', async () => {
    useChatStore.getState().setMode('general');
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
      timeout: 2000,
    });
    const reply = useChatStore.getState().messages[1];
    expect(reply.content.length).toBeGreaterThan(0);
    expect(reply.sources?.length ?? 0).toBe(0); // общий режим источников не возвращает
  });

  it('setMode игнорируется во время стрима', () => {
    vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.getState().send('Roadmap'); // streaming=true
    useChatStore.getState().setMode('general');
    expect(useChatStore.getState().mode).toBe('vault'); // на лету не переключается
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

  it('сессии (#CS): done пишет обмен в БД; hydrate продолжает последнюю сессию', async () => {
    useChatStore.getState().hydrate('/vault/A');
    useChatStore.getState().send('Roadmap');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), { timeout: 2000 });
    // Обмен ушёл в сессию (мок-БД): sessionId присвоен.
    await vi.waitFor(() => expect(useChatStore.getState().sessionId).not.toBeNull());

    // «Перезапуск»: пустой стейт → hydrate → подтянулась последняя сессия из БД.
    useChatStore.setState({ messages: [], sessionId: null });
    useChatStore.getState().hydrate('/vault/A');
    await vi.waitFor(() =>
      expect(useChatStore.getState().messages.length).toBeGreaterThanOrEqual(2),
    );
    const restored = useChatStore.getState().messages;
    expect(restored[0]).toMatchObject({ role: 'user', content: 'Roadmap' });
    expect(restored.every((m) => !m.streaming)).toBe(true);
  });

  it('сессии (#CS): hydrate(null) чистит ленту и сессию (vault закрыт)', async () => {
    useChatStore.getState().hydrate('/vault/A');
    useChatStore.getState().send('вопрос про A');
    await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), { timeout: 2000 });

    useChatStore.getState().hydrate(null);
    expect(useChatStore.getState().messages).toHaveLength(0);
    expect(useChatStore.getState().sessionId).toBeNull();
  });

  it('сессии (#CS): newSession сбрасывает ленту; следующий обмен — НОВАЯ сессия, старая жива', async () => {
    const { list } = await import('../lib/mock/sessions');
    useChatStore.getState().hydrate('/vault/A');
    useChatStore.getState().send('первый диалог');
    await vi.waitFor(() => expect(useChatStore.getState().sessionId).not.toBeNull());
    const firstId = useChatStore.getState().sessionId;

    useChatStore.getState().newSession();
    expect(useChatStore.getState().messages).toHaveLength(0);
    expect(useChatStore.getState().sessionId).toBeNull();

    useChatStore.getState().send('второй диалог');
    await vi.waitFor(() => expect(useChatStore.getState().sessionId).not.toBeNull());
    expect(useChatStore.getState().sessionId).not.toBe(firstId);
    expect((await list()).length).toBe(2); // ничего не удаляем — память «второго мозга»
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
    await vi.waitFor(() => expect(useChatStore.getState().sessionId).not.toBeNull());

    // «Перезапуск» → hydrate: сводка не восстанавливается, ответ — да.
    useChatStore.setState({ messages: [], sessionId: null });
    useChatStore.getState().hydrate('/vault/R');
    await vi.waitFor(() =>
      expect(useChatStore.getState().messages.some((m) => m.role === 'assistant')).toBe(true),
    );
    const restored = useChatStore.getState().messages.find((m) => m.role === 'assistant');
    expect(restored?.content.length).toBeGreaterThan(0);
    expect(restored?.reasoningSummary).toBeUndefined();
  });

  // Аудит 2026-06-10 (актуализировано под сессии #CS): смена vault ПОСРЕДИ стрима — осиротевший
  // стрим дорезается, новый контекст чист.
  it('hydrate при активном стриме: стрим дорезан, лента не утекает', () => {
    useChatStore.getState().hydrate('/vault/A');
    vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
    useChatStore.getState().send('вопрос про A');
    expect(useChatStore.getState().streaming).toBe(true);

    useChatStore.getState().hydrate(null);
    expect(useChatStore.getState().streaming).toBe(false);
    expect(useChatStore.getState().messages).toHaveLength(0);
  });

  // audit B12: epoch-гард onEvent — поздние события остановленного стрима не трогают финализированный ответ.
  it('onEvent после stop() игнорирует поздние события старого стрима', () => {
    let captured: ((e: ChatStreamEvent) => void) | undefined;
    vi.spyOn(tauriApi.chat, 'streamRag').mockImplementation((_q, onEvent) => {
      captured = onEvent;
      return () => {};
    });
    useChatStore.getState().send('вопрос');
    useChatStore.getState().stop(); // ответ финализирован (streaming=false)
    const before = useChatStore.getState().messages.at(-1)?.content;

    // поздний done после stop
    captured?.({ type: 'done', full: 'ПОЗДНИЙ ОТВЕТ старого стрима' } as ChatStreamEvent);
    expect(useChatStore.getState().messages.at(-1)?.content).toBe(before); // не заменён
  });

  // audit B12: loadSession после await не затирает активный стрим, стартовавший за время загрузки.
  it('loadSession не затирает чат, если за время загрузки стартовал send', async () => {
    const stored = [{ role: 'assistant', content: 'старая история', sourcesJson: null }];
    let resolveMsgs: (v: typeof stored) => void = () => {};
    vi.spyOn(tauriApi.chat.sessions, 'messages').mockReturnValue(
      new Promise<typeof stored>((res) => {
        resolveMsgs = res;
      }) as ReturnType<typeof tauriApi.chat.sessions.messages>,
    );
    vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});

    const p = useChatStore.getState().loadSession(7); // ждёт messages()
    useChatStore.getState().send('новый вопрос'); // во время загрузки → streaming=true
    resolveMsgs(stored);
    await p;

    expect(useChatStore.getState().messages.some((m) => m.content === 'новый вопрос')).toBe(true);
    expect(useChatStore.getState().messages.some((m) => m.content === 'старая история')).toBe(false);
  });

  // audit B12: disclosureOpen — LRU-кап вместо полной чистки при переполнении.
  it('disclosureOpen: LRU-кап (>600) вытесняет старейшие, не чистит все', () => {
    disclosureOpen.clear();
    for (let i = 0; i < 610; i++) disclosureOpen.set(`msg${i}:src`, true);
    expect(disclosureOpen.size).toBeLessThanOrEqual(600);
    expect(disclosureOpen.get('msg0:src')).toBeUndefined(); // старейшие вытеснены по одному
    expect(disclosureOpen.get('msg609:src')).toBe(true); // свежие раскрытия сохранены
  });

  // MEM-3 (AC-MEM-6): авто-ПРЕДЛОЖЕНИЕ факта после обмена при включённой памяти агента.
  describe('MEM-3 авто-предложение факта', () => {
    it('done при aiAgentMemory=on → propose → pendingFact под последним ответом', async () => {
      usePrefsStore.setState({ aiAgentMemory: true });
      const propose = vi
        .spyOn(tauriApi.memory, 'propose')
        .mockResolvedValue('пользователь пишет на Rust');
      useChatStore.getState().send('я пишу на Rust');
      await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
        timeout: 2000,
      });
      await vi.waitFor(() =>
        expect(useChatStore.getState().pendingFact?.text).toBe('пользователь пишет на Rust'),
      );
      expect(propose).toHaveBeenCalled();
      // Чип привязан к ПОСЛЕДНЕМУ (assistant) сообщению.
      const last = useChatStore.getState().messages.at(-1);
      expect(useChatStore.getState().pendingFact?.messageId).toBe(last?.id);
    });

    it('aiAgentMemory=off → propose не зовётся, чипа нет (D5)', async () => {
      usePrefsStore.setState({ aiAgentMemory: false });
      const propose = vi.spyOn(tauriApi.memory, 'propose').mockResolvedValue('факт');
      useChatStore.getState().send('вопрос');
      await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
        timeout: 2000,
      });
      expect(propose).not.toHaveBeenCalled();
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('confirmFact пишет факт (source=auto) и снимает чип', async () => {
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue(1);
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'дедлайн в пятницу' } });
      await useChatStore.getState().confirmFact();
      expect(add).toHaveBeenCalledWith('дедлайн в пятницу', 'auto');
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('dismissFact снимает чип, в БД ничего не пишет (D1)', () => {
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue(1);
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'что-то' } });
      useChatStore.getState().dismissFact();
      expect(useChatStore.getState().pendingFact).toBeNull();
      expect(add).not.toHaveBeenCalled();
    });

    it('новый send снимает прежнее предложение факта', () => {
      vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'старый' } });
      useChatStore.getState().send('новый вопрос');
      expect(useChatStore.getState().pendingFact).toBeNull();
      useChatStore.getState().stop();
    });
  });
});
