import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { tauriApi, type ChatStreamEvent, type ConsolidationPlan } from '../lib/tauri-api';
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
        .mockResolvedValue(['пользователь пишет на Rust']);
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
      const propose = vi.spyOn(tauriApi.memory, 'propose').mockResolvedValue(['факт']);
      useChatStore.getState().send('вопрос');
      await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
        timeout: 2000,
      });
      expect(propose).not.toHaveBeenCalled();
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('confirmFact пишет факт (source=auto) и снимает чип', async () => {
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'дедлайн в пятницу' } });
      await useChatStore.getState().confirmFact();
      expect(add).toHaveBeenCalledWith('дедлайн в пятницу', 'auto');
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('dismissFact снимает чип, в БД ничего не пишет (D1)', () => {
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'что-то' } });
      useChatStore.getState().dismissFact();
      expect(useChatStore.getState().pendingFact).toBeNull();
      expect(add).not.toHaveBeenCalled();
    });

    it('новый send снимает прежнее предложение факта', () => {
      vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {});
      useChatStore.setState({
        pendingFact: { messageId: 'a1', text: 'старый' },
        pendingFactQueue: ['ещё'],
      });
      useChatStore.getState().send('новый вопрос');
      expect(useChatStore.getState().pendingFact).toBeNull();
      expect(useChatStore.getState().pendingFactQueue).toEqual([]); // MEM-9: очередь тоже снята
      useChatStore.getState().stop();
    });

    it('MEM-9: N фактов из обмена → чипы по очереди (confirm/dismiss продвигают)', async () => {
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.setState({
        pendingFact: { messageId: 'a1', text: 'факт 1' },
        pendingFactQueue: ['факт 2', 'факт 3'],
      });
      await useChatStore.getState().confirmFact();
      expect(add).toHaveBeenCalledWith('факт 1', 'auto');
      expect(useChatStore.getState().pendingFact?.text).toBe('факт 2'); // продвинулись к следующему
      expect(useChatStore.getState().pendingFactQueue).toEqual(['факт 3']);
      useChatStore.getState().dismissFact(); // отклоняем «факт 2» (в БД не пишем)
      expect(useChatStore.getState().pendingFact?.text).toBe('факт 3');
      useChatStore.getState().dismissFact(); // отклоняем «факт 3» — очередь пуста, чип снят
      expect(useChatStore.getState().pendingFact).toBeNull();
      expect(useChatStore.getState().pendingFactQueue).toEqual([]);
      expect(add).toHaveBeenCalledTimes(1); // только подтверждённый «факт 1» записан
    });
  });

  // MEM-8b: консолидация фактов (режим «Предлагать») при подтверждении авто-факта.
  describe('MEM-8b консолидация фактов', () => {
    afterEach(() => usePrefsStore.setState({ aiMemoryConsolidation: false }));

    it('флаг OFF → confirmFact пишет add, consolidate_plan НЕ зван', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: false });
      const plan = vi.spyOn(tauriApi.memory, 'consolidatePlan');
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'факт' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('written');
      expect(add).toHaveBeenCalledWith('факт', 'auto');
      expect(plan).not.toHaveBeenCalled();
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('консолидация ON, но память агента OFF → plain add (гейт = предикат UI)', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: false });
      const plan = vi.spyOn(tauriApi.memory, 'consolidatePlan');
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'факт' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('written');
      expect(add).toHaveBeenCalledWith('факт', 'auto');
      expect(plan).not.toHaveBeenCalled();
    });

    it('флаг ON, op=add → apply(accept), чипа-предложения нет', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue({
        candidate: 'факт',
        source: 'auto',
        op: { kind: 'add' },
      });
      const apply = vi
        .spyOn(tauriApi.memory, 'consolidateApply')
        .mockResolvedValue({ op: 'add', id: 1, inserted: true });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'факт' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('written');
      expect(apply).toHaveBeenCalledWith(expect.objectContaining({ op: { kind: 'add' } }), 'accept');
      expect(useChatStore.getState().pendingConsolidation).toBeNull();
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('флаг ON, op=supersede → чип-предложение, НИЧЕГО не записано до выбора', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      const planObj: ConsolidationPlan = {
        candidate: 'дедлайн среда',
        source: 'auto',
        op: { kind: 'supersede', targetId: 7, oldText: 'дедлайн пятница', targetSource: 'auto' },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'дедлайн среда' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('proposed');
      expect(apply).not.toHaveBeenCalled(); // ничего не применено без клика
      expect(useChatStore.getState().pendingFact).toBeNull(); // fact-чип скрыт
      expect(useChatStore.getState().pendingConsolidation).toEqual({
        messageId: 'a1',
        plan: planObj,
      });
    });

    it('флаг ON, op=noop → noop (ничего не записано)', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue({
        candidate: 'факт',
        source: 'auto',
        op: { kind: 'noop', coveredBy: 3 },
      });
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'факт' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('noop');
      expect(apply).not.toHaveBeenCalled();
      expect(useChatStore.getState().pendingFact).toBeNull();
    });

    it('resolveConsolidation(accept) применяет op и продвигает очередь фактов', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      const planObj: ConsolidationPlan = {
        candidate: 'среда',
        source: 'auto',
        op: { kind: 'supersede', targetId: 7, oldText: 'пятница', targetSource: 'auto' },
      };
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply').mockResolvedValue({
        op: 'supersede',
        id: 2,
        supersededId: 7,
        oldText: 'пятница',
        newText: 'среда',
        inserted: true,
        opGroup: 1,
      });
      useChatStore.setState({
        pendingConsolidation: { messageId: 'a1', plan: planObj },
        pendingFactQueue: ['следующий факт'],
      });
      const r = await useChatStore.getState().resolveConsolidation('accept');
      expect(r).toBe('written');
      expect(apply).toHaveBeenCalledWith(planObj, 'accept');
      expect(useChatStore.getState().pendingConsolidation).toBeNull();
      expect(useChatStore.getState().pendingFact?.text).toBe('следующий факт'); // очередь продвинулась
    });

    it('resolveConsolidation(keepSeparate) → apply(keepSeparate)', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      const planObj: ConsolidationPlan = {
        candidate: 'b',
        source: 'auto',
        op: { kind: 'update', targetId: 1, oldText: 'a', newText: 'a b', targetSource: 'auto' },
      };
      const apply = vi
        .spyOn(tauriApi.memory, 'consolidateApply')
        .mockResolvedValue({ op: 'add', id: 5, inserted: true });
      useChatStore.setState({
        pendingConsolidation: { messageId: 'a1', plan: planObj },
        pendingFactQueue: [],
      });
      await useChatStore.getState().resolveConsolidation('keepSeparate');
      expect(apply).toHaveBeenCalledWith(planObj, 'keepSeparate');
      expect(useChatStore.getState().pendingConsolidation).toBeNull();
    });

    it('dismissConsolidation: НИЧЕГО не пишет, продвигает очередь', () => {
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      const planObj: ConsolidationPlan = {
        candidate: 'x',
        source: 'auto',
        op: { kind: 'update', targetId: 1, oldText: 'a', newText: 'ab', targetSource: 'auto' },
      };
      useChatStore.setState({
        pendingConsolidation: { messageId: 'a1', plan: planObj },
        pendingFactQueue: [],
      });
      useChatStore.getState().dismissConsolidation();
      expect(apply).not.toHaveBeenCalled();
      expect(useChatStore.getState().pendingConsolidation).toBeNull();
    });

    it('consolidate_plan упал → fail-safe обычный add', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockRejectedValue(new Error('down'));
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'факт' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('written');
      expect(add).toHaveBeenCalledWith('факт', 'auto');
    });

    it('epoch-гард: за время plan чип сменился → ничего не применяем', async () => {
      usePrefsStore.setState({ aiMemoryConsolidation: true, aiAgentMemory: true });
      let resolvePlan!: (p: ConsolidationPlan) => void;
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockReturnValue(
        new Promise<ConsolidationPlan>((res) => {
          resolvePlan = res;
        }),
      );
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      const add = vi.spyOn(tauriApi.memory, 'add');
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'старый' } });
      const pending = useChatStore.getState().confirmFact();
      // за время ожидания plan лента сменилась (новый обмен) — чип теперь про другое сообщение
      useChatStore.setState({ pendingFact: { messageId: 'a2', text: 'новый' } });
      resolvePlan({
        candidate: 'старый',
        source: 'auto',
        op: { kind: 'supersede', targetId: 1, oldText: 'o', targetSource: 'auto' },
      });
      const r = await pending;
      expect(r).toBe('noop');
      expect(apply).not.toHaveBeenCalled();
      expect(add).not.toHaveBeenCalled();
    });
  });

  // MEM-8c: авто-режим консолидации (применять слияния/замещения молча, кроме explicit-фактов).
  describe('MEM-8c авто-режим консолидации', () => {
    afterEach(() =>
      usePrefsStore.setState({ aiMemoryConsolidation: false, aiMemoryConsolidationMode: 'propose' }),
    );
    const onAuto = () =>
      usePrefsStore.setState({
        aiMemoryConsolidation: true,
        aiAgentMemory: true,
        aiMemoryConsolidationMode: 'auto',
      });

    it('авто-режим, supersede на AUTO-факт → авто-применено + autoConsolidated, без чипа', async () => {
      onAuto();
      const planObj: ConsolidationPlan = {
        candidate: 'дедлайн среда',
        source: 'auto',
        op: { kind: 'supersede', targetId: 7, oldText: 'дедлайн пятница', targetSource: 'auto' },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply').mockResolvedValue({
        op: 'supersede',
        id: 2,
        supersededId: 7,
        oldText: 'дедлайн пятница',
        newText: 'дедлайн среда',
        inserted: true,
        opGroup: 5,
      });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'дедлайн среда' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('autoConsolidated');
      expect(apply).toHaveBeenCalledWith(planObj, 'accept');
      expect(useChatStore.getState().pendingConsolidation).toBeNull();
      expect(useChatStore.getState().autoConsolidated).toMatchObject({
        op: 'supersede',
        opGroup: 5,
        oldText: 'дедлайн пятница',
        newText: 'дедлайн среда',
      });
    });

    it('авто-режим, но цель EXPLICIT → чип (защита §4.3), НЕ авто-применяем', async () => {
      onAuto();
      const planObj: ConsolidationPlan = {
        candidate: 'новый',
        source: 'auto',
        op: { kind: 'supersede', targetId: 7, oldText: 'явный факт юзера', targetSource: 'explicit' },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'новый' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('proposed');
      expect(apply).not.toHaveBeenCalled();
      expect(useChatStore.getState().pendingConsolidation).toEqual({ messageId: 'a1', plan: planObj });
      expect(useChatStore.getState().autoConsolidated).toBeNull();
    });

    it('авто-режим, update на AUTO-факт → авто-применено + autoConsolidated', async () => {
      onAuto();
      const planObj: ConsolidationPlan = {
        candidate: 'x',
        source: 'auto',
        op: { kind: 'update', targetId: 1, oldText: 'a', newText: 'a b', targetSource: 'auto' },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      vi.spyOn(tauriApi.memory, 'consolidateApply').mockResolvedValue({
        op: 'update',
        id: 1,
        oldText: 'a',
        newText: 'a b',
        opGroup: 9,
      });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'x' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('autoConsolidated');
      expect(useChatStore.getState().autoConsolidated).toMatchObject({ op: 'update', opGroup: 9 });
    });

    it('режим «Предлагать» (дефолт) — supersede всё равно через чип (8b сохранён)', async () => {
      usePrefsStore.setState({
        aiMemoryConsolidation: true,
        aiAgentMemory: true,
        aiMemoryConsolidationMode: 'propose',
      });
      const planObj: ConsolidationPlan = {
        candidate: 'x',
        source: 'auto',
        op: { kind: 'supersede', targetId: 1, oldText: 'o', targetSource: 'auto' },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'x' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('proposed');
      expect(apply).not.toHaveBeenCalled();
    });

    it('авто-режим, degraded-to-add (outcome.op=add) → written, без autoConsolidated', async () => {
      onAuto();
      const planObj: ConsolidationPlan = {
        candidate: 'x',
        source: 'auto',
        op: { kind: 'supersede', targetId: 7, oldText: 'устаревший', targetSource: 'auto' },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      vi.spyOn(tauriApi.memory, 'consolidateApply').mockResolvedValue({
        op: 'add',
        id: 3,
        inserted: true,
      });
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'x' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('written');
      expect(useChatStore.getState().autoConsolidated).toBeNull();
    });

    it('авто-режим, цель с НЕИЗВЕСТНЫМ source → чип (fail-closed §4.3, не молчаливый apply)', async () => {
      onAuto();
      const planObj: ConsolidationPlan = {
        candidate: 'новый',
        source: 'auto',
        // source не 'auto' (напр. будущий imported/synced или регрессия бэка) — fail-closed: НЕ авто.
        op: {
          kind: 'supersede',
          targetId: 7,
          oldText: 'импортированный факт',
          targetSource: 'imported' as 'auto',
        },
      };
      vi.spyOn(tauriApi.memory, 'consolidatePlan').mockResolvedValue(planObj);
      const apply = vi.spyOn(tauriApi.memory, 'consolidateApply');
      useChatStore.setState({ pendingFact: { messageId: 'a1', text: 'новый' } });
      const r = await useChatStore.getState().confirmFact();
      expect(r).toBe('proposed');
      expect(apply).not.toHaveBeenCalled();
      expect(useChatStore.getState().autoConsolidated).toBeNull();
    });

    it('undoConsolidation зовёт consolidateUndo по opGroup и пробрасывает исход', async () => {
      const undo = vi.spyOn(tauriApi.memory, 'consolidateUndo').mockResolvedValue(true);
      await expect(useChatStore.getState().undoConsolidation(5)).resolves.toBe(true);
      expect(undo).toHaveBeenCalledWith(5);
      // false (факт уже изменён / группа откачена) проброшен честно — тост не соврёт «Отменено».
      undo.mockResolvedValue(false);
      await expect(useChatStore.getState().undoConsolidation(5)).resolves.toBe(false);
    });
  });

  // MEM-5: явная команда «запомни …» сохраняет сразу; кнопка «В память»; undo.
  describe('MEM-5: захват факта в память из чата', () => {
    it('явная команда «запомни …» сохраняет факт сразу (source=explicit) + savedFact, без чипа', async () => {
      const propose = vi.spyOn(tauriApi.memory, 'propose').mockResolvedValue(['Работает над RMS B2B']);
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 42, inserted: true });
      useChatStore.getState().send('запомни что я работаю над RMS B2B');
      await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
        timeout: 2000,
      });
      await vi.waitFor(() => expect(useChatStore.getState().savedFact).not.toBeNull());
      expect(propose).toHaveBeenCalled();
      expect(add).toHaveBeenCalledWith('Работает над RMS B2B', 'explicit');
      expect(useChatStore.getState().savedFact).toEqual({
        status: 'saved',
        id: 42,
        text: 'Работает над RMS B2B',
      });
      expect(useChatStore.getState().pendingFact).toBeNull(); // не чип-подтверждение
      expect(useChatStore.getState().explicitSaving).toBe(false);
    });

    it('дубль (inserted=false) → savedFact status=duplicate, БЕЗ id (undo не сотрёт существующий)', async () => {
      vi.spyOn(tauriApi.memory, 'propose').mockResolvedValue(['Уже сохранённый факт']);
      vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 9, inserted: false });
      useChatStore.getState().send('запомни уже сохранённый факт');
      await vi.waitFor(() => expect(useChatStore.getState().savedFact).not.toBeNull(), {
        timeout: 2000,
      });
      expect(useChatStore.getState().savedFact).toEqual({
        status: 'duplicate',
        text: 'Уже сохранённый факт',
      });
    });

    it('сбой записи (add throws) → savedFact status=error (не выдаём за «уже в памяти»)', async () => {
      vi.spyOn(tauriApi.memory, 'propose').mockResolvedValue(['Факт']);
      vi.spyOn(tauriApi.memory, 'add').mockRejectedValue(new Error('db down'));
      useChatStore.getState().send('запомни факт');
      await vi.waitFor(() => expect(useChatStore.getState().savedFact).not.toBeNull(), {
        timeout: 2000,
      });
      expect(useChatStore.getState().savedFact).toEqual({ status: 'error' });
    });

    it('обычная реплика НЕ сохраняет факт сразу (memory.add не зовётся, savedFact пуст)', async () => {
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 1, inserted: true });
      useChatStore.getState().send('расскажи про RMS B2B');
      await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
        timeout: 2000,
      });
      expect(add).not.toHaveBeenCalled();
      expect(useChatStore.getState().savedFact).toBeNull();
    });

    it('captureFromMessage («В память») извлекает и сохраняет факт из обмена сообщения', async () => {
      vi.spyOn(tauriApi.memory, 'propose').mockResolvedValue(['Любит тёмную тему']);
      const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue({ id: 7, inserted: true });
      useChatStore.getState().send('какой стиль предпочесть');
      await vi.waitFor(() => expect(useChatStore.getState().streaming).toBe(false), {
        timeout: 2000,
      });
      const assistant = useChatStore.getState().messages.at(-1)!;
      const saved = await useChatStore.getState().captureFromMessage(assistant.id);
      expect(saved).toBe(true);
      expect(add).toHaveBeenCalledWith('Любит тёмную тему', 'explicit');
      expect(useChatStore.getState().savedFact).toEqual({
        status: 'saved',
        id: 7,
        text: 'Любит тёмную тему',
      });
      expect(useChatStore.getState().capturingId).toBeNull();
    });

    it('undoSavedFact удаляет факт по id', async () => {
      const del = vi.spyOn(tauriApi.memory, 'delete').mockResolvedValue();
      await useChatStore.getState().undoSavedFact(42);
      expect(del).toHaveBeenCalledWith(42);
    });
  });
});
