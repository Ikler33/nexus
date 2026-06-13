import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useToastStore } from './toast';

beforeEach(() => useToastStore.setState({ toasts: [] }));

describe('toast store (TOAST-1)', () => {
  it('addToast добавляет тост и возвращает id', () => {
    const id = useToastStore.getState().addToast('привет');
    const ts = useToastStore.getState().toasts;
    expect(ts).toHaveLength(1);
    expect(ts[0].id).toBe(id);
    expect(ts[0].message).toBe('привет');
    expect(ts[0].kind).toBe('info');
  });

  it('кап в 3: четвёртый выбрасывает старейший (FIFO)', () => {
    const s = useToastStore.getState();
    s.addToast('1');
    s.addToast('2');
    s.addToast('3');
    s.addToast('4');
    expect(useToastStore.getState().toasts.map((t) => t.message)).toEqual(['2', '3', '4']);
  });

  it('dismiss убирает тост', () => {
    const id = useToastStore.getState().addToast('x');
    useToastStore.getState().dismiss(id);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('авто-исчезновение по таймеру', () => {
    vi.useFakeTimers();
    try {
      useToastStore.getState().addToast('исчезни', { durationMs: 1000 });
      expect(useToastStore.getState().toasts).toHaveLength(1);
      vi.advanceTimersByTime(1000);
      expect(useToastStore.getState().toasts).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
