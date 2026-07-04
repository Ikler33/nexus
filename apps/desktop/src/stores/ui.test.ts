import { afterEach, describe, expect, it } from 'vitest';

import { selectMainView, selectReadingEscBlocked, selectTrapOverlay, useUIStore } from './ui';

afterEach(() => {
  useUIStore.setState({
    chatOpen: false,
    mainView: 'home',
    // W-6: сбрасываем и плавающие слои — иначе утекают между тестами (граф/Tasks/Inbox/Sync/…).
    graphOpen: false,
    tasksOpen: false,
    inboxOpen: false,
    syncOpen: false,
    pluginsOpen: false,
    conflictOpen: false,
    versionsOpen: false,
    // B2: trap/floating-слои, участвующие в blocked-наборе toggleChat.
    goalsOpen: false,
    memoryOpen: false,
    episodesOpen: false,
    digestOpen: false,
    contradictionsOpen: false,
    paletteOpen: false,
    cheatsheetOpen: false,
    tweaksOpen: false,
  });
});

describe('ui-стор: открытие AI-панели из полноэкранных вью (баг владельца 2026-06-11)', () => {
  it('openChat с Home выводит в workspace (панель гейтится mainView===editor)', () => {
    useUIStore.setState({ chatOpen: false, mainView: 'home' });
    useUIStore.getState().openChat();
    const s = useUIStore.getState();
    expect(s.chatOpen).toBe(true);
    expect(s.mainView).toBe('editor');
  });

  it('toggleChat: открытие с News уводит из News; повторный клик в workspace закрывает', () => {
    useUIStore.setState({ chatOpen: false, mainView: 'news' });
    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().mainView).toBe('editor');

    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(false);
  });

  it('toggleChat при открытой-но-скрытой панели (ушли на Home) возвращает её, а не закрывает', () => {
    useUIStore.setState({ chatOpen: true, mainView: 'home' });
    useUIStore.getState().toggleChat();
    const s = useUIStore.getState();
    expect(s.chatOpen).toBe(true);
    expect(s.mainView).toBe('editor');
  });
});

describe('ui-стор: взаимоисключение примарных вью home/news/board (BOARD-4)', () => {
  it('openBoard гасит прочие примарные вью (открыта только одна — структурно)', () => {
    useUIStore.setState({ mainView: 'home' });
    useUIStore.getState().openBoard();
    expect(useUIStore.getState().mainView).toBe('board');
  });

  it('openHome/openNews/openChat уводят из board (не остаётся два примарных вью)', () => {
    useUIStore.setState({ mainView: 'board' });
    useUIStore.getState().openHome();
    expect(useUIStore.getState().mainView).toBe('home');

    useUIStore.setState({ mainView: 'board' });
    useUIStore.getState().openNews();
    expect(useUIStore.getState().mainView).toBe('news');

    useUIStore.setState({ mainView: 'board', chatOpen: false });
    useUIStore.getState().openChat();
    expect(useUIStore.getState().mainView).toBe('editor');
  });
});

describe('ui-стор: переход на main-вью гасит плавающие слои (W-6, ST-D1)', () => {
  it('openNews из открытого графа/Tasks/Inbox/Sync — гасит их (навигация срабатывает из любого слоя)', () => {
    useUIStore.setState({
      mainView: 'home',
      graphOpen: true,
      tasksOpen: true,
      inboxOpen: true,
      syncOpen: true,
      pluginsOpen: true,
    });
    useUIStore.getState().openNews();
    const s = useUIStore.getState();
    expect(s.mainView).toBe('news');
    // Все блокирующие слои погашены → News реально виден.
    expect(s.graphOpen).toBe(false);
    expect(s.tasksOpen).toBe(false);
    expect(s.inboxOpen).toBe(false);
    expect(s.syncOpen).toBe(false);
    expect(s.pluginsOpen).toBe(false);
  });

  it('openAgent/openHome/openBoard/openToday тоже гасят граф и оверлеи', () => {
    for (const open of ['openAgent', 'openHome', 'openBoard', 'openToday'] as const) {
      useUIStore.setState({ graphOpen: true, tasksOpen: true, inboxOpen: true });
      useUIStore.getState()[open]();
      const s = useUIStore.getState();
      expect({ open, ...s }).toMatchObject({ graphOpen: false, tasksOpen: false, inboxOpen: false });
    }
  });

  it('conflict/versions (модальные safe-flow) при переходе НЕ гасятся', () => {
    useUIStore.setState({ mainView: 'editor', conflictOpen: true, versionsOpen: true });
    useUIStore.getState().openHome();
    const s = useUIStore.getState();
    expect(s.conflictOpen).toBe(true);
    expect(s.versionsOpen).toBe(true);
  });

  // Ревью W-6: аналогичные nav-в-workspace пути (chat / inspector) тоже должны гасить блокирующие слои.
  it('openChat / openInspectorSection из открытого графа+оверлеев — гасят их', () => {
    useUIStore.setState({ chatOpen: false, graphOpen: true, tasksOpen: true, syncOpen: true });
    useUIStore.getState().openChat();
    let s = useUIStore.getState();
    expect(s.chatOpen).toBe(true);
    expect(s.graphOpen).toBe(false);
    expect(s.tasksOpen).toBe(false);
    expect(s.syncOpen).toBe(false);

    useUIStore.setState({ graphOpen: true, inboxOpen: true });
    useUIStore.getState().openInspectorSection('backlinks');
    s = useUIStore.getState();
    expect(s.graphOpen).toBe(false);
    expect(s.inboxOpen).toBe(false);
    expect(s.pendingInspectorSection).toBe('backlinks');
  });

  it('toggleChat: чат «открыт», но скрыт за графом → возврат в поле зрения (гасит граф)', () => {
    useUIStore.setState({ chatOpen: true, graphOpen: true, mainView: 'editor' });
    useUIStore.getState().toggleChat();
    const s = useUIStore.getState();
    expect(s.chatOpen).toBe(true); // не закрыли — вернули
    expect(s.graphOpen).toBe(false);
  });
});

describe('ui-стор: toggleChat знает ВСЕ блокирующие слои (B2 — goals/memory/episodes/digest/contradictions)', () => {
  // Каждая из 5 панелей, которых не было в рукописном списке: чат «открыт», но скрыт под панелью →
  // toggleChat возвращает его в поле зрения (гасит панель), а НЕ закрывает чат под ней.
  const panels = [
    'goalsOpen',
    'memoryOpen',
    'episodesOpen',
    'digestOpen',
    'contradictionsOpen',
  ] as const;
  for (const panel of panels) {
    it(`чат скрыт под ${panel} → возврат в поле зрения (панель гаснет, чат остаётся открыт)`, () => {
      useUIStore.setState({ chatOpen: true, mainView: 'editor', [panel]: true });
      useUIStore.getState().toggleChat();
      const s = useUIStore.getState();
      expect(s.chatOpen).toBe(true); // не закрыли — вернули
      expect(s[panel]).toBe(false); // блокирующий слой погашен
    });
  }

  it('дрейф-гард: ЛЮБОЙ блокирующий слой возвращает чат (набор проверки = набор гашения)', () => {
    // Возврат чата гасит ровно {mainView→editor} ∪ FLOATS_AND_TRAPS_CLOSED → повторный toggle при
    // любом взведённом ключе из этого набора обязан возвращать чат, не закрывать.
    useUIStore.setState({ chatOpen: true, mainView: 'editor', paletteOpen: true });
    useUIStore.getState().toggleChat();
    let s = useUIStore.getState();
    expect(s.chatOpen).toBe(true);
    expect(s.paletteOpen).toBe(false);

    // Чистый workspace (ничего не блокирует) → toggle закрывает чат, как и раньше.
    useUIStore.getState().toggleChat();
    s = useUIStore.getState();
    expect(s.chatOpen).toBe(false);
  });
});

describe('ui-стор: примарная вью «Сегодня» (TODAY-1) — взаимоисключение + dead-button', () => {
  it('openToday гасит прочие примарные вью (только одна — структурно)', () => {
    useUIStore.setState({ mainView: 'home' });
    useUIStore.getState().openToday();
    expect(useUIStore.getState().mainView).toBe('today');
  });

  it('openHome/openNews/openBoard уводят из «Сегодня» (не остаётся два примарных вью)', () => {
    for (const open of ['openHome', 'openNews', 'openBoard'] as const) {
      useUIStore.setState({ mainView: 'today' });
      useUIStore.getState()[open]();
      expect(useUIStore.getState().mainView).not.toBe('today');
    }
  });

  it('openChat из «Сегодня» уводит в редактор — иначе мёртвая кнопка чата (AI-панель за Today)', () => {
    useUIStore.setState({ mainView: 'today', chatOpen: false });
    useUIStore.getState().openChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().mainView).toBe('editor');
  });

  it('toggleChat: открытие из «Сегодня» уводит из неё; скрытая панель за Today возвращается', () => {
    // открытие чата из Today
    useUIStore.setState({ chatOpen: false, mainView: 'today' });
    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().mainView).toBe('editor');

    // панель уже «открыта», но скрыта за Today → клик возвращает её (re-surface)
    useUIStore.setState({ chatOpen: true, mainView: 'today' });
    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().mainView).toBe('editor');
  });

  it('toggleToday выключает вью (показывается редактор), не трогая chatOpen', () => {
    useUIStore.setState({ mainView: 'today', chatOpen: true });
    useUIStore.getState().toggleToday();
    expect(useUIStore.getState().mainView).toBe('editor');
    expect(useUIStore.getState().chatOpen).toBe(true);
  });
});

describe('ui-стор: взаимоисключение оверлеев goals/tasks/inbox (P9-ревью #5)', () => {
  afterEach(() => useUIStore.setState({ goalsOpen: false, tasksOpen: false, inboxOpen: false }));

  it('открытие одной модалки закрывает остальные (не стекаются два focus-trap)', () => {
    useUIStore.setState({ goalsOpen: false, tasksOpen: false, inboxOpen: false });
    useUIStore.getState().toggleTasks();
    expect(useUIStore.getState().tasksOpen).toBe(true);

    useUIStore.getState().toggleInbox(); // открываем Inbox — Tasks должен закрыться
    expect(useUIStore.getState().inboxOpen).toBe(true);
    expect(useUIStore.getState().tasksOpen).toBe(false);

    useUIStore.getState().toggleGoals(); // открываем Goals — Inbox закрывается
    expect(useUIStore.getState().goalsOpen).toBe(true);
    expect(useUIStore.getState().inboxOpen).toBe(false);
  });

  it('повторный тоггл закрывает свою модалку, не трогая другие', () => {
    useUIStore.setState({ goalsOpen: false, tasksOpen: true, inboxOpen: false });
    useUIStore.getState().toggleTasks();
    expect(useUIStore.getState().tasksOpen).toBe(false);
  });
});

describe('ui-стор: TAGCLICK-1 — отложенный тег-фильтр сайдбара', () => {
  it('openTagFilter кладёт тег, показывает сайдбар и выходит из reading', () => {
    useUIStore.setState({ pendingTagFilter: null, sidebarOpen: false, reading: true });
    useUIStore.getState().openTagFilter('ideas');
    const s = useUIStore.getState();
    expect(s.pendingTagFilter).toBe('ideas');
    expect(s.sidebarOpen).toBe(true);
    expect(s.reading).toBe(false);
  });
  it('consumeTagFilter сбрасывает отложенный тег', () => {
    useUIStore.setState({ pendingTagFilter: 'ideas' });
    useUIStore.getState().consumeTagFilter();
    expect(useUIStore.getState().pendingTagFilter).toBeNull();
  });
});

describe('ui-стор: REVEAL-ACTIVE-FILE', () => {
  it('requestReveal ставит цель, показывает сайдбар, выходит из reading; seq растёт при повторе', () => {
    useUIStore.setState({ revealTarget: null, sidebarOpen: false, reading: true });
    useUIStore.getState().requestReveal('Notes/A.md');
    const s = useUIStore.getState();
    expect(s.revealTarget?.path).toBe('Notes/A.md');
    expect(s.sidebarOpen).toBe(true);
    expect(s.reading).toBe(false);
    const seq1 = s.revealTarget!.seq;
    useUIStore.getState().requestReveal('Notes/A.md'); // тот же путь → seq растёт (перезапуск скролла)
    expect(useUIStore.getState().revealTarget!.seq).toBe(seq1 + 1);
  });
  it('consumeReveal сбрасывает цель', () => {
    useUIStore.setState({ revealTarget: { path: 'x', seq: 1 } });
    useUIStore.getState().consumeReveal();
    expect(useUIStore.getState().revealTarget).toBeNull();
  });
});

describe('ui-стор: AGENT-SEED (Castor «Быстрый старт», P1-11)', () => {
  afterEach(() => useUIStore.setState({ pendingAgentSeed: null, mainView: 'home' }));

  it('openAgent(seed) открывает агента и кладёт промпт; повтор того же текста растит seq', () => {
    useUIStore.setState({ pendingAgentSeed: null, mainView: 'home' });
    useUIStore.getState().openAgent('разбери входящие');
    const s = useUIStore.getState();
    expect(s.mainView).toBe('agent'); // переход на main-вью «Агент» (гасит home)
    expect(s.pendingAgentSeed?.text).toBe('разбери входящие');
    const seq1 = s.pendingAgentSeed!.seq;
    // Повторный сид того же текста → seq растёт (AgentView перезапускает prefill).
    useUIStore.getState().openAgent('разбери входящие');
    expect(useUIStore.getState().pendingAgentSeed!.seq).toBe(seq1 + 1);
  });

  it('openAgent() без seed открывает агента, но НЕ трогает отложенный промпт (нет затирания)', () => {
    useUIStore.setState({ pendingAgentSeed: { text: 'старый', seq: 5 }, mainView: 'home' });
    useUIStore.getState().openAgent();
    expect(useUIStore.getState().mainView).toBe('agent');
    expect(useUIStore.getState().pendingAgentSeed).toEqual({ text: 'старый', seq: 5 });
  });

  it('openAgent с пустым/пробельным seed не ставит промпт (нет пустого prefill)', () => {
    useUIStore.setState({ pendingAgentSeed: null, mainView: 'home' });
    useUIStore.getState().openAgent('   ');
    expect(useUIStore.getState().mainView).toBe('agent');
    expect(useUIStore.getState().pendingAgentSeed).toBeNull();
  });

  it('consumeAgentSeed сбрасывает отложенный промпт', () => {
    useUIStore.setState({ pendingAgentSeed: { text: 'x', seq: 1 } });
    useUIStore.getState().consumeAgentSeed();
    expect(useUIStore.getState().pendingAgentSeed).toBeNull();
  });
});

describe('ui-стор: F-4 derived-селекторы (mainView / trapOverlay / Esc-прецедент)', () => {
  const view = () => selectMainView(useUIStore.getState());

  it('selectMainView совпадает с приоритетом open-экшенов (каждая ветка)', () => {
    useUIStore.getState().openNews();
    expect(view()).toBe('news');
    useUIStore.getState().openToday();
    expect(view()).toBe('today');
    useUIStore.getState().openAgent();
    expect(view()).toBe('agent');
    useUIStore.getState().openBoard();
    expect(view()).toBe('board');
    useUIStore.getState().openHome();
    expect(view()).toBe('home');
    // Уход из main-вью (тогл текущей) → редактор (все main-флаги погашены).
    useUIStore.getState().toggleHome();
    expect(view()).toBe('editor');
  });

  it('setMainView — единый примитив смены вью; selectMainView читает поле; гасит слои (W-6)', () => {
    // F-4: main-вью = ОДНО поле → «две вью сразу» структурно невозможны; selectMainView === поле.
    for (const v of ['editor', 'board', 'news', 'home', 'today', 'agent'] as const) {
      useUIStore.getState().setMainView(v);
      expect(view()).toBe(v);
      expect(useUIStore.getState().mainView).toBe(v);
    }
    // Nav-семантика: setMainView гасит плавающие/trap-слои (как openX).
    useUIStore.setState({ graphOpen: true, tasksOpen: true, tweaksOpen: true });
    useUIStore.getState().setMainView('news');
    const s = useUIStore.getState();
    expect(s.mainView).toBe('news');
    expect(s.graphOpen).toBe(false);
    expect(s.tasksOpen).toBe(false);
    expect(s.tweaksOpen).toBe(false);
  });

  it('selectTrapOverlay: маппинг каждого оверлея, приоритет верхнего и null', () => {
    const reset = () =>
      useUIStore.setState({
        paletteOpen: false, cheatsheetOpen: false, goalsOpen: false, tasksOpen: false,
        inboxOpen: false, memoryOpen: false, episodesOpen: false, tweaksOpen: false,
      });
    const trap = () => selectTrapOverlay(useUIStore.getState());
    reset(); expect(trap()).toBeNull();
    reset(); useUIStore.setState({ tweaksOpen: true }); expect(trap()).toBe('settings');
    reset(); useUIStore.setState({ episodesOpen: true }); expect(trap()).toBe('episodes');
    reset(); useUIStore.setState({ memoryOpen: true }); expect(trap()).toBe('memory');
    reset(); useUIStore.setState({ inboxOpen: true }); expect(trap()).toBe('inbox');
    reset(); useUIStore.setState({ tasksOpen: true }); expect(trap()).toBe('tasks');
    reset(); useUIStore.setState({ goalsOpen: true }); expect(trap()).toBe('goals');
    reset(); useUIStore.setState({ cheatsheetOpen: true }); expect(trap()).toBe('cheatsheet');
    reset(); useUIStore.setState({ paletteOpen: true }); expect(trap()).toBe('palette');
    // tweaks-дрейф: Настройки + палитра одновременно → верхний детерминирован (palette > settings).
    reset(); useUIStore.setState({ tweaksOpen: true, paletteOpen: true }); expect(trap()).toBe('palette');
    reset();
  });

  it('selectReadingEscBlocked: каждый модальный оверлей блокирует Esc; main/chat/reading — нет', () => {
    const allClosed = {
      paletteOpen: false, graphOpen: false, pluginsOpen: false, syncOpen: false, captureOpen: false,
      templatesOpen: false, versionsOpen: false, cheatsheetOpen: false, conflictOpen: false,
      goalsOpen: false, memoryOpen: false, episodesOpen: false, tasksOpen: false, inboxOpen: false,
      digestOpen: false, contradictionsOpen: false, tweaksOpen: false,
    };
    const blocked = () => selectReadingEscBlocked(useUIStore.getState());
    useUIStore.setState(allClosed);
    expect(blocked()).toBe(false);
    for (const key of Object.keys(allClosed) as (keyof typeof allClosed)[]) {
      useUIStore.setState({ ...allClosed, [key]: true });
      expect(blocked(), key).toBe(true);
    }
    // main-вью/chat/reading Esc НЕ блокируют (у reading свой выход, у оверлеев — свой).
    useUIStore.setState(allClosed);
    useUIStore.getState().openHome();
    useUIStore.setState({ chatOpen: true, reading: true });
    expect(blocked()).toBe(false);
  });
});

describe('ui-стор: FILE-RENAME-COMMAND', () => {
  it('requestRename ставит цель, показывает сайдбар, выходит из reading; seq растёт при повторе', () => {
    useUIStore.setState({ renameTarget: null, sidebarOpen: false, reading: true });
    useUIStore.getState().requestRename('Notes/A.md');
    const s = useUIStore.getState();
    expect(s.renameTarget?.path).toBe('Notes/A.md');
    expect(s.sidebarOpen).toBe(true);
    expect(s.reading).toBe(false);
    const seq1 = s.renameTarget!.seq;
    useUIStore.getState().requestRename('Notes/A.md'); // тот же путь → seq растёт (перезапуск)
    expect(useUIStore.getState().renameTarget!.seq).toBe(seq1 + 1);
  });
  it('consumeRename сбрасывает цель', () => {
    useUIStore.setState({ renameTarget: { path: 'x', seq: 1 } });
    useUIStore.getState().consumeRename();
    expect(useUIStore.getState().renameTarget).toBeNull();
  });
});
