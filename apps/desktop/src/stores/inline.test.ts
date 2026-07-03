import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ghostActive, ghostField, ghostTextOf } from '../lib/editor/inlineGhost';
import { useInlineStore } from './inline';

// Вне Tauri `tauriApi.inline.complete` проксируется в мок `mock/vault.streamInline` (токены → done).
function makeView(doc: string, anchor: number, head = anchor) {
  const state = EditorState.create({
    doc,
    selection: { anchor, head },
    extensions: [ghostField],
  });
  return new EditorView({ state, parent: document.body });
}

afterEach(() => {
  useInlineStore.getState().cancelInline();
  useInlineStore.setState({ active: false, streaming: false, mode: null, error: null });
});

describe('inline store (IL-2)', () => {
  it('continue: триггер сразу показывает ghost; стрим наполняет его (AC-IL-1/2)', async () => {
    const view = makeView('Жил-был кот', 11);
    useInlineStore.getState().runInline(view, 'continue');

    // Статус «генерируется» виден сразу (ghost активен, streaming=true) — AC-IL-1.
    expect(useInlineStore.getState().active).toBe(true);
    expect(useInlineStore.getState().streaming).toBe(true);
    expect(ghostActive(view.state)).toBe(true);

    await vi.waitFor(() => expect(useInlineStore.getState().streaming).toBe(false), {
      timeout: 2000,
    });
    expect((ghostTextOf(view.state) ?? '').length).toBeGreaterThan(0);
    view.destroy();
  });

  it('rewrite без выделения → ошибка показана у курсора (AC-IL-7)', () => {
    const view = makeView('текст', 5); // курсор, выделения нет
    useInlineStore.getState().runInline(view, 'rewrite');
    const err = useInlineStore.getState().error;
    expect(err).toBeTruthy(); // локализованное сообщение, не голый код
    expect(ghostActive(view.state)).toBe(true); // ошибка-виджет у курсора
    expect(ghostTextOf(view.state)).toBe(err);
    view.destroy();
  });

  it('continue в пустом документе → ошибка показана (AC-IL-7)', () => {
    const view = makeView('', 0);
    useInlineStore.getState().runInline(view, 'continue');
    expect(useInlineStore.getState().error).toBeTruthy();
    expect(ghostActive(view.state)).toBe(true);
    view.destroy();
  });

  it('cancelInline останавливает стрим и сбрасывает active (AC-IL-6/8)', () => {
    const view = makeView('Жил-был кот', 11);
    useInlineStore.getState().runInline(view, 'continue');
    expect(useInlineStore.getState().streaming).toBe(true);
    useInlineStore.getState().cancelInline();
    expect(useInlineStore.getState().active).toBe(false);
    expect(useInlineStore.getState().streaming).toBe(false);
    view.destroy();
  });
});
