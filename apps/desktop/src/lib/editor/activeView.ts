import type { EditorView } from '@codemirror/view';

/**
 * Реестр активного CM6-редактора (IL-3): команды палитры/глобальные хоткеи не имеют прямого доступа к
 * `EditorView` (несколько групп/вкладок), поэтому редактор регистрирует свой view при фокусе/монтировании,
 * а команды берут его отсюда. Простой модульный синглтон — активен ровно один редактор (последний фокус).
 */
let active: EditorView | null = null;

/** Регистрирует активный редактор (фокус/монтирование) либо снимает (`null` при размонтировании). */
export function setActiveEditorView(view: EditorView | null): void {
  active = view;
}

/** Снимает регистрацию ровно этого view (idempotent — не трогает, если активен уже другой). */
export function clearActiveEditorView(view: EditorView): void {
  if (active === view) active = null;
}

/** Текущий редактор в фокусе (или `null`, если ни один не открыт/не в фокусе). */
export function getActiveEditorView(): EditorView | null {
  return active;
}
