//! CM6 ghost-text для inline-LLM (IL-2/3, спека `docs/specs/inline-llm.md`, AC-IL-1..10). Предложение
//! модели показывается серым ghost-текстом у курсора; `Tab` принять, `Esc` отклонить. Чистый CM6 (без
//! сети/стора): стрим-контроллер (`stores/inline.ts`) шлёт сюда эффекты, клавиатуру — `inlineKeymap`.

import { type EditorState, Prec, StateEffect, StateField, type Extension } from '@codemirror/state';
import {
  Decoration,
  type DecorationSet,
  EditorView,
  keymap,
  WidgetType,
} from '@codemirror/view';

import i18n from '../../i18n/setup';

/** Состояние активного ghost-предложения. `from..to` — диапазон вставки/замены (для `continue`
 *  `from==to==pos`; для `rewrite`/`summarize` `from..to` — выделение, `pos` — конец выделения). */
export interface GhostState {
  /** Позиция-якорь, где рисуется ghost-виджет. */
  pos: number;
  /** Начало диапазона замены при accept. */
  from: number;
  /** Конец диапазона замены при accept. */
  to: number;
  /** Накопленный текст предложения (или текст ошибки при `isError`). */
  text: string;
  /** Идёт ли ещё стрим (для индикации/последующих веток). */
  streaming: boolean;
  /** Это не предложение, а транзиентная inline-ошибка у курсора (AC-IL-7). */
  isError?: boolean;
}

/** Начать новый ghost (сбрасывает прежний). */
export const setGhost = StateEffect.define<{ pos: number; from: number; to: number }>();
/** Дописать стримовую дельту в ghost. */
export const appendGhost = StateEffect.define<string>();
/** Стрим завершён (текст финальный, но ещё не принят). */
export const endGhostStream = StateEffect.define<void>();
/** Убрать ghost (accept/reject/cancel/правка). */
export const clearGhost = StateEffect.define<void>();
/** Показать inline-ошибку у курсора (AC-IL-7) — текст-сообщение. */
export const setGhostError = StateEffect.define<string>();

/** Серый неинтерактивный виджет предложения (+ подсказка Tab/Esc, когда стрим завершён). */
class GhostWidget extends WidgetType {
  constructor(
    readonly text: string,
    readonly showHint: boolean,
  ) {
    super();
  }
  eq(other: GhostWidget) {
    return other.text === this.text && other.showHint === this.showHint;
  }
  toDOM() {
    const span = document.createElement('span');
    span.className = 'cm-inline-ghost';
    span.setAttribute('aria-hidden', 'true');
    span.textContent = this.text;
    if (this.showHint) {
      const hint = document.createElement('span');
      hint.className = 'cm-inline-ghost-hint';
      hint.textContent = `  ${i18n.t('inline.hintAccept')} · ${i18n.t('inline.hintReject')}`;
      span.appendChild(hint);
    }
    return span;
  }
  ignoreEvent() {
    return true;
  }
}

/** Индикатор «генерируется» у курсора, пока не пришёл первый токен (AC-IL-1: статус виден сразу). */
class PendingWidget extends WidgetType {
  eq() {
    return true;
  }
  toDOM() {
    const span = document.createElement('span');
    span.className = 'cm-inline-ghost cm-inline-ghost-pending';
    span.setAttribute('aria-hidden', 'true');
    span.textContent = `✦ ${i18n.t('inline.generating')}`;
    return span;
  }
  ignoreEvent() {
    return true;
  }
}

/** Транзиентная inline-ошибка у курсора (AC-IL-7): без модала, без краша, снимается по Esc/правке/таймауту. */
class ErrorWidget extends WidgetType {
  constructor(readonly message: string) {
    super();
  }
  eq(other: ErrorWidget) {
    return other.message === this.message;
  }
  toDOM() {
    const span = document.createElement('span');
    span.className = 'cm-inline-ghost cm-inline-ghost-error';
    span.setAttribute('role', 'status');
    span.textContent = `⚠ ${this.message}`;
    return span;
  }
  ignoreEvent() {
    return true;
  }
}

function ghostDecorations(state: GhostState | null): DecorationSet {
  if (!state) return Decoration.none;
  let widget: WidgetType | null;
  if (state.isError) widget = new ErrorWidget(state.text);
  else if (state.text) widget = new GhostWidget(state.text, !state.streaming);
  else if (state.streaming) widget = new PendingWidget();
  else widget = null;
  if (!widget) return Decoration.none;
  return Decoration.set([Decoration.widget({ widget, side: 1 }).range(state.pos)]);
}

/** Поле ghost-состояния: применяет эффекты, маппит позиции, снимает ghost при пользовательской правке. */
export const ghostField = StateField.define<GhostState | null>({
  create() {
    return null;
  },
  update(value, tr) {
    let next = value;
    if (next && tr.docChanged) {
      next = {
        ...next,
        pos: tr.changes.mapPos(next.pos, 1),
        from: tr.changes.mapPos(next.from, -1),
        to: tr.changes.mapPos(next.to, 1),
      };
    }
    let cleared = false;
    let started = false;
    for (const e of tr.effects) {
      if (e.is(setGhost)) {
        next = { pos: e.value.pos, from: e.value.from, to: e.value.to, text: '', streaming: true };
        started = true;
      } else if (e.is(appendGhost) && next) {
        next = { ...next, text: next.text + e.value };
      } else if (e.is(endGhostStream) && next) {
        next = { ...next, streaming: false };
      } else if (e.is(setGhostError)) {
        const at = tr.state.selection.main.head;
        next = { pos: at, from: at, to: at, text: e.value, streaming: false, isError: true };
        started = true;
      } else if (e.is(clearGhost)) {
        next = null;
        cleared = true;
      }
    }
    // Снять ghost при правке пользователя (как автокомплит): правка accept'а несёт clearGhost (cleared),
    // setGhost/setGhostError документ не меняют → любой другой docChange = редактирование → dismiss.
    if (tr.docChanged && !cleared && !started) {
      next = null;
    }
    return next;
  },
  provide: (f) => EditorView.decorations.from(f, ghostDecorations),
});

/** Активен ли ghost (есть предложение ИЛИ показана ошибка). */
export function ghostActive(state: EditorState): boolean {
  return state.field(ghostField, false) != null;
}

/** Текущий ghost-текст (для тестов/индикации) либо `null`. */
export function ghostTextOf(state: EditorState): string | null {
  return state.field(ghostField, false)?.text ?? null;
}

/** Принять предложение: заменить `from..to` на текст ghost, курсор — после вставки (AC-IL-3). Ошибку
 *  принять нельзя (Tab проходит штатно). */
export function acceptGhost(view: EditorView): boolean {
  const g = view.state.field(ghostField, false);
  if (!g || !g.text || g.isError) return false;
  view.dispatch({
    changes: { from: g.from, to: g.to, insert: g.text },
    selection: { anchor: g.from + g.text.length },
    effects: clearGhost.of(),
  });
  return true;
}

/** Отклонить предложение / убрать ошибку: документ/курсор не трогать (AC-IL-4). */
export function rejectGhost(view: EditorView): boolean {
  if (view.state.field(ghostField, false) == null) return false;
  view.dispatch({ effects: clearGhost.of() });
  return true;
}

/** Клавиатура ghost: `Tab` принять / `Esc` отклонить — ТОЛЬКО при активном ghost (AC-IL-5: иначе
 *  Tab/Esc работают штатно). `onResolve` зовётся после accept/reject — контроллер гасит стрим. */
export function inlineKeymap(opts: { onResolve: () => void }): Extension {
  return Prec.highest(
    keymap.of([
      {
        key: 'Tab',
        run: (view) => {
          if (!ghostActive(view.state)) return false;
          const ok = acceptGhost(view);
          if (ok) opts.onResolve();
          return ok;
        },
      },
      {
        key: 'Escape',
        run: (view) => {
          if (!ghostActive(view.state)) return false;
          const ok = rejectGhost(view);
          if (ok) opts.onResolve();
          return ok;
        },
      },
    ]),
  );
}
