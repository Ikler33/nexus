import { afterEach, describe, expect, it } from 'vitest';

import { useUIStore } from './ui';

afterEach(() => {
  useUIStore.setState({ chatOpen: false, homeOpen: true, newsOpen: false });
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
