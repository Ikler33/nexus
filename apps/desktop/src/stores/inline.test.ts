import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ghostActive, ghostField, ghostTextOf } from '../components/editor/inlineGhost';
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

  it('rewrite без выделения → ошибка no-selection, ghost не создаётся', () => {
    const view = makeView('текст', 5); // курсор, выделения нет
    useInlineStore.getState().runInline(view, 'rewrite');
    expect(useInlineStore.getState().error).toBe('no-selection');
    expect(ghostActive(view.state)).toBe(false);
    view.destroy();
  });

  it('continue в пустом документе → ошибка no-text', () => {
    const view = makeView('', 0);
    useInlineStore.getState().runInline(view, 'continue');
    expect(useInlineStore.getState().error).toBe('no-text');
    expect(ghostActive(view.state)).toBe(false);
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
