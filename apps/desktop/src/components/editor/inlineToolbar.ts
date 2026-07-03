//! Плавающий inline-тулбар по выделению (IL-3, D4/AC-IL-9): при непустом выделении над ним всплывает
//! тулбар с действиями Переписать / Сократить / Продолжить → запускают inline-генерацию по выделению.
//! Реализован как CM6-tooltip (позиционирование/жизненный цикл — самим CodeMirror).

import { type EditorState, StateField } from '@codemirror/state';
import { showTooltip, type Tooltip, tooltips } from '@codemirror/view';

import i18n from '../../i18n/setup';
import type { InlineMode } from '../../lib/tauri-api';
import { useInlineStore } from '../../stores/inline';
import { ghostField } from '../../lib/editor/inlineGhost';

const ACTIONS: { mode: InlineMode; key: string }[] = [
  { mode: 'rewrite', key: 'inline.rewrite' },
  { mode: 'summarize', key: 'inline.summarize' },
  { mode: 'continue', key: 'inline.continue' },
];

/** Тулбар показываем при непустом выделении и БЕЗ активного ghost (не поверх предложения). */
function selectionTooltips(state: EditorState): readonly Tooltip[] {
  const sel = state.selection.main;
  if (sel.empty) return [];
  if (state.field(ghostField, false) != null) return [];
  return [
    {
      pos: sel.from,
      end: sel.to,
      above: true,
      arrow: false,
      create: (view) => {
        const dom = document.createElement('div');
        dom.className = 'cm-inline-toolbar';
        dom.setAttribute('role', 'toolbar');
        dom.setAttribute('aria-label', i18n.t('inline.toolbarLabel'));
        for (const { mode, key } of ACTIONS) {
          const btn = document.createElement('button');
          btn.type = 'button';
          btn.className = 'cm-inline-toolbar-btn';
          btn.textContent = i18n.t(key);
          // mousedown + preventDefault: клик не сбрасывает выделение до чтения его в runInline.
          btn.addEventListener('mousedown', (e) => {
            e.preventDefault();
            useInlineStore.getState().runInline(view, mode);
          });
          dom.appendChild(btn);
        }
        return { dom };
      },
    },
  ];
}

/** Расширение: тулбар по выделению. Пересчитывается при смене выделения/документа/эффектов (ghost). */
const toolbarField = StateField.define<readonly Tooltip[]>({
  create: selectionTooltips,
  update(value, tr) {
    if (!tr.docChanged && !tr.selection && tr.effects.length === 0) return value;
    return selectionTooltips(tr.state);
  },
  provide: (f) => showTooltip.computeN([f], (state) => state.field(f)),
});

/** Полное расширение inline-тулбара (поле + хост тултипов в overlay-режиме). */
export const inlineToolbar = [tooltips({ position: 'absolute' }), toolbarField];

// Экспорт для тестов.
export { selectionTooltips };
