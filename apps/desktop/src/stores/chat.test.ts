import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useChatStore } from './chat';

// В vitest (не Tauri) `streamRag` проксируется в мок `mock/vault.streamChat` (sources→токены→done).
beforeEach(() => {
  useChatStore.setState({ messages: [], streaming: false });
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
});
