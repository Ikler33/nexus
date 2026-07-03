import type { EditorView } from '@codemirror/view';
import { create } from 'zustand';

import {
  appendGhost,
  clearGhost,
  endGhostStream,
  setGhost,
  setGhostError,
} from '../lib/editor/inlineGhost';
import i18n from '../i18n/setup';
import type { InlineMode } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';

/**
 * Контроллер inline-LLM (IL-2/3, спека `docs/specs/inline-llm.md`): связывает CM6 ghost (`inlineGhost.ts`)
 * со стрим-командой бэкенда (`tauriApi.inline`). Один активный стрим за раз (AC-IL-8): новый `runInline`
 * гасит прежний. Токены копятся и применяются раз в кадр (rAF-троттл, как чат V2.4 / AC-IL-2). Ошибка —
 * тихая нотификация у курсора (AC-IL-7) + флаг в сторе (для aria-live). Ghost живёт в CM; здесь —
 * стрим/rAF + UI-флаги (active/streaming/error).
 */

let cancelStream: (() => void) | null = null;
let pending = '';
let rafId: number | null = null;
let errorTimer: ReturnType<typeof setTimeout> | null = null;

/** Сколько держать inline-ошибку у курсора до авто-снятия. */
const ERROR_TTL_MS = 6000;

function cancelFlush() {
  if (rafId != null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
}
function clearErrorTimer() {
  if (errorTimer != null) {
    clearTimeout(errorTimer);
    errorTimer = null;
  }
}

interface InlineState {
  /** Есть активная inline-сессия (стрим идёт ИЛИ ghost ждёт accept/reject). */
  active: boolean;
  /** Идёт ли приём токенов. */
  streaming: boolean;
  /** Текущий режим (для индикации). */
  mode: InlineMode | null;
  /** Сообщение ошибки (локализованное) для aria-live; `null` — нет. */
  error: string | null;
  /** Запустить inline-генерацию у курсора/выделения текущего редактора. */
  runInline: (view: EditorView, mode: InlineMode) => void;
  /** Остановить активный стрим (не трогает ghost — его гасят accept/reject). Идемпотентно. */
  cancelInline: () => void;
  /** Сбросить показанную ошибку. */
  clearError: () => void;
}

export const useInlineStore = create<InlineState>((set) => {
  /** Показать inline-ошибку у курсора + флаг для SR; авто-снятие через TTL (AC-IL-7). */
  const showError = (view: EditorView, message: string) => {
    clearErrorTimer();
    view.dispatch({ effects: setGhostError.of(message) });
    set({ active: false, streaming: false, mode: null, error: message });
    errorTimer = setTimeout(() => {
      view.dispatch({ effects: clearGhost.of() });
      errorTimer = null;
    }, ERROR_TTL_MS);
  };

  return {
    active: false,
    streaming: false,
    mode: null,
    error: null,

    runInline(view, mode) {
      // Один активный inline за раз (AC-IL-8): гасим прежний стрим/таймер и его ghost.
      cancelStream?.();
      cancelStream = null;
      cancelFlush();
      clearErrorTimer();
      pending = '';
      view.dispatch({ effects: clearGhost.of() });
      set({ error: null });

      const sel = view.state.selection.main;
      let from: number;
      let to: number;
      let pos: number;
      let context: string;
      let selection: string | undefined;

      if (mode === 'continue') {
        // Контекст = текст до курсора (D2); вставка в позиции курсора.
        pos = sel.head;
        from = sel.head;
        to = sel.head;
        context = view.state.sliceDoc(0, sel.head);
        selection = undefined;
        if (context.trim() === '') {
          showError(view, i18n.t('inline.errNoText'));
          return;
        }
      } else {
        // rewrite/summarize работают по выделению (D4); результат заменит его.
        if (sel.empty) {
          showError(view, i18n.t('inline.errNoSelection'));
          return;
        }
        from = sel.from;
        to = sel.to;
        pos = sel.to; // ghost-превью показываем после выделения
        context = '';
        selection = view.state.sliceDoc(sel.from, sel.to);
      }

      view.dispatch({ effects: setGhost.of({ pos, from, to }) });
      set({ active: true, streaming: true, mode, error: null });

      const flush = () => {
        rafId = null;
        if (!pending) return;
        const chunk = pending;
        pending = '';
        view.dispatch({ effects: appendGhost.of(chunk) });
      };
      const scheduleFlush = () => {
        // rAF может исчезнуть при teardown jsdom (vitest, node 25) — мок-стрим довершается
        // после сноса окружения; фолбэк на setTimeout не даёт unhandled-error.
        if (rafId == null)
          rafId =
            typeof requestAnimationFrame === 'function'
              ? requestAnimationFrame(flush)
              : (setTimeout(flush, 16) as unknown as number);
      };

      cancelStream = tauriApi.inline.complete(mode, context, selection, (event) => {
        switch (event.type) {
          case 'token':
            pending += event.text;
            scheduleFlush();
            break;
          case 'done':
            cancelFlush();
            if (pending) {
              view.dispatch({ effects: appendGhost.of(pending) });
              pending = '';
            }
            view.dispatch({ effects: endGhostStream.of() });
            cancelStream = null;
            set({ streaming: false });
            break;
          case 'error':
            cancelFlush();
            pending = '';
            cancelStream = null;
            showError(view, event.message);
            break;
        }
      });
    },

    cancelInline() {
      cancelStream?.();
      cancelStream = null;
      cancelFlush();
      clearErrorTimer();
      pending = '';
      set({ active: false, streaming: false, mode: null });
    },

    clearError() {
      set({ error: null });
    },
  };
});
