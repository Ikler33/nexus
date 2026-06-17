import { afterEach, describe, expect, it } from 'vitest';

import { useUIStore } from './ui';

afterEach(() => {
  useUIStore.setState({ chatOpen: false, homeOpen: true, newsOpen: false, boardOpen: false });
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
