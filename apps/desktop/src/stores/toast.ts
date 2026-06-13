// Глобальная toast-система (P4/POLISH, решение плана #15): единый примитив видимой обратной связи —
// подтверждения захвата, ошибки сохранения, результаты фоновых операций. Раньше такие сигналы либо
// молча терялись, либо жили локально (news.notice). FIFO-очередь с авто-исчезновением; таймеры —
// в модульном Map (не часть UI-стейта).
import { create } from 'zustand';

export type ToastKind = 'info' | 'success' | 'error';

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  /** Опциональное действие (напр. «Открыть», «Отменить») — клик выполняет и закрывает тост. */
  action?: { label: string; run: () => void };
}

/** Сколько тостов держим на экране одновременно (старейший выбрасывается при переполнении). */
const MAX = 3;
/** Длительности авто-исчезновения по типу (мс). */
const DURATION: Record<ToastKind, number> = { info: 4000, success: 4000, error: 7000 };

const timers = new Map<number, ReturnType<typeof setTimeout>>();
let seq = 0;

function clearTimer(id: number): void {
  const t = timers.get(id);
  if (t) {
    clearTimeout(t);
    timers.delete(id);
  }
}

interface ToastState {
  toasts: Toast[];
  addToast: (
    message: string,
    opts?: { kind?: ToastKind; action?: Toast['action']; durationMs?: number },
  ) => number;
  dismiss: (id: number) => void;
}

export const useToastStore = create<ToastState>((set, get) => ({
  toasts: [],
  addToast(message, opts = {}) {
    const kind = opts.kind ?? 'info';
    const id = ++seq;
    set((s) => {
      const next = [...s.toasts, { id, kind, message, action: opts.action }];
      while (next.length > MAX) {
        const dropped = next.shift();
        if (dropped) clearTimer(dropped.id);
      }
      return { toasts: next };
    });
    // С действием держим дольше (успеть нажать); ошибки сами по себе живут дольше info/success.
    const ms = opts.durationMs ?? (opts.action ? DURATION[kind] + 3000 : DURATION[kind]);
    timers.set(
      id,
      setTimeout(() => get().dismiss(id), ms),
    );
    return id;
  },
  dismiss(id) {
    clearTimer(id);
    set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) }));
  },
}));
