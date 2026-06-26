import { afterEach, describe, expect, it } from 'vitest';

import { useUIStore } from './ui';

afterEach(() => {
  useUIStore.setState({
    chatOpen: false,
    homeOpen: true,
    newsOpen: false,
    boardOpen: false,
    todayOpen: false,
    // W-6: сбрасываем и плавающие слои — иначе утекают между тестами (граф/Tasks/Inbox/Sync/…).
    graphOpen: false,
    tasksOpen: false,
    inboxOpen: false,
    syncOpen: false,
    pluginsOpen: false,
    conflictOpen: false,
    versionsOpen: false,
  });
});

describe('ui-стор: открытие AI-панели из полноэкранных вью (баг владельца 2026-06-11)', () => {
  it('openChat с Home выводит в workspace (панель гейтится !homeOpen && !newsOpen)', () => {
    useUIStore.setState({ chatOpen: false, homeOpen: true, newsOpen: false });
    useUIStore.getState().openChat();
    const s = useUIStore.getState();
    expect(s.chatOpen).toBe(true);
    expect(s.homeOpen).toBe(false);
    expect(s.newsOpen).toBe(false);
  });

  it('toggleChat: открытие с News уводит из News; повторный клик в workspace закрывает', () => {
    useUIStore.setState({ chatOpen: false, homeOpen: false, newsOpen: true });
    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().newsOpen).toBe(false);

    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(false);
  });

  it('toggleChat при открытой-но-скрытой панели (ушли на Home) возвращает её, а не закрывает', () => {
    useUIStore.setState({ chatOpen: true, homeOpen: true, newsOpen: false });
    useUIStore.getState().toggleChat();
    const s = useUIStore.getState();
    expect(s.chatOpen).toBe(true);
    expect(s.homeOpen).toBe(false);
  });
});

describe('ui-стор: взаимоисключение примарных вью home/news/board (BOARD-4)', () => {
  it('openBoard гасит home и news (одновременно открыта только одна примарная вью)', () => {
    useUIStore.setState({ homeOpen: true, newsOpen: false, boardOpen: false });
    useUIStore.getState().openBoard();
    const s = useUIStore.getState();
    expect(s.boardOpen).toBe(true);
    expect(s.homeOpen).toBe(false);
    expect(s.newsOpen).toBe(false);
  });

  it('openHome/openNews/openChat гасят board (не остаётся два примарных вью true)', () => {
    useUIStore.setState({ boardOpen: true, homeOpen: false, newsOpen: false });
    useUIStore.getState().openHome();
    expect(useUIStore.getState().boardOpen).toBe(false);

    useUIStore.setState({ boardOpen: true, homeOpen: false, newsOpen: false });
    useUIStore.getState().openNews();
    expect(useUIStore.getState().boardOpen).toBe(false);

    useUIStore.setState({ boardOpen: true, homeOpen: false, newsOpen: false, chatOpen: false });
    useUIStore.getState().openChat();
    expect(useUIStore.getState().boardOpen).toBe(false);
  });
});

describe('ui-стор: переход на main-вью гасит плавающие слои (W-6, ST-D1)', () => {
  it('openNews из открытого графа/Tasks/Inbox/Sync — гасит их (навигация срабатывает из любого слоя)', () => {
    useUIStore.setState({
      homeOpen: true,
      newsOpen: false,
      graphOpen: true,
      tasksOpen: true,
      inboxOpen: true,
      syncOpen: true,
      pluginsOpen: true,
    });
    useUIStore.getState().openNews();
    const s = useUIStore.getState();
    expect(s.newsOpen).toBe(true);
    expect(s.homeOpen).toBe(false);
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
    useUIStore.setState({ homeOpen: false, conflictOpen: true, versionsOpen: true });
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
    useUIStore.setState({ chatOpen: true, graphOpen: true, homeOpen: false });
    useUIStore.getState().toggleChat();
    const s = useUIStore.getState();
    expect(s.chatOpen).toBe(true); // не закрыли — вернули
    expect(s.graphOpen).toBe(false);
  });
});

describe('ui-стор: примарная вью «Сегодня» (TODAY-1) — взаимоисключение + dead-button', () => {
  it('openToday гасит home/news/board (только одна примарная вью)', () => {
    useUIStore.setState({ homeOpen: true, newsOpen: false, boardOpen: false, todayOpen: false });
    useUIStore.getState().openToday();
    const s = useUIStore.getState();
    expect(s.todayOpen).toBe(true);
    expect(s.homeOpen).toBe(false);
    expect(s.newsOpen).toBe(false);
    expect(s.boardOpen).toBe(false);
  });

  it('openHome/openNews/openBoard гасят todayOpen (не остаётся два примарных вью)', () => {
    for (const open of ['openHome', 'openNews', 'openBoard'] as const) {
      useUIStore.setState({ todayOpen: true, homeOpen: false, newsOpen: false, boardOpen: false });
      useUIStore.getState()[open]();
      expect(useUIStore.getState().todayOpen).toBe(false);
    }
  });

  it('openChat из «Сегодня» гасит todayOpen — иначе мёртвая кнопка чата (AI-панель за todayOpen)', () => {
    useUIStore.setState({ todayOpen: true, chatOpen: false, homeOpen: false });
    useUIStore.getState().openChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().todayOpen).toBe(false);
  });

  it('toggleChat: открытие из «Сегодня» уводит из неё; скрытая панель за Today возвращается', () => {
    // открытие чата из Today
    useUIStore.setState({ chatOpen: false, todayOpen: true, homeOpen: false });
    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().todayOpen).toBe(false);

    // панель уже «открыта», но скрыта за Today → клик возвращает её (re-surface)
    useUIStore.setState({ chatOpen: true, todayOpen: true, homeOpen: false });
    useUIStore.getState().toggleChat();
    expect(useUIStore.getState().chatOpen).toBe(true);
    expect(useUIStore.getState().todayOpen).toBe(false);
  });

  it('toggleToday выключает вью (показывается редактор), не трогая chatOpen', () => {
    useUIStore.setState({ todayOpen: true, chatOpen: true });
    useUIStore.getState().toggleToday();
    expect(useUIStore.getState().todayOpen).toBe(false);
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
  afterEach(() => useUIStore.setState({ pendingAgentSeed: null, agentOpen: false }));

  it('openAgent(seed) открывает агента и кладёт промпт; повтор того же текста растит seq', () => {
    useUIStore.setState({ pendingAgentSeed: null, agentOpen: false, homeOpen: true });
    useUIStore.getState().openAgent('разбери входящие');
    const s = useUIStore.getState();
    expect(s.agentOpen).toBe(true);
    expect(s.homeOpen).toBe(false); // переход на main-вью гасит home (SWITCH_MAIN)
    expect(s.pendingAgentSeed?.text).toBe('разбери входящие');
    const seq1 = s.pendingAgentSeed!.seq;
    // Повторный сид того же текста → seq растёт (AgentView перезапускает prefill).
    useUIStore.getState().openAgent('разбери входящие');
    expect(useUIStore.getState().pendingAgentSeed!.seq).toBe(seq1 + 1);
  });

  it('openAgent() без seed открывает агента, но НЕ трогает отложенный промпт (нет затирания)', () => {
    useUIStore.setState({ pendingAgentSeed: { text: 'старый', seq: 5 }, agentOpen: false });
    useUIStore.getState().openAgent();
    expect(useUIStore.getState().agentOpen).toBe(true);
    expect(useUIStore.getState().pendingAgentSeed).toEqual({ text: 'старый', seq: 5 });
  });

  it('openAgent с пустым/пробельным seed не ставит промпт (нет пустого prefill)', () => {
    useUIStore.setState({ pendingAgentSeed: null, agentOpen: false });
    useUIStore.getState().openAgent('   ');
    expect(useUIStore.getState().agentOpen).toBe(true);
    expect(useUIStore.getState().pendingAgentSeed).toBeNull();
  });

  it('consumeAgentSeed сбрасывает отложенный промпт', () => {
    useUIStore.setState({ pendingAgentSeed: { text: 'x', seq: 1 } });
    useUIStore.getState().consumeAgentSeed();
    expect(useUIStore.getState().pendingAgentSeed).toBeNull();
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
