import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { describe, expect, it } from 'vitest';

import {
  acceptGhost,
  appendGhost,
  clearGhost,
  ghostActive,
  ghostField,
  ghostTextOf,
  rejectGhost,
  setGhost,
} from './inlineGhost';

const stateWith = (doc: string) => EditorState.create({ doc, extensions: [ghostField] });
const viewWith = (doc: string) => new EditorView({ state: stateWith(doc), parent: document.body });

describe('inlineGhost (IL-2)', () => {
  it('setGhost+appendGhost накапливают текст (AC-IL-2)', () => {
    let s = stateWith('abc');
    expect(ghostActive(s)).toBe(false);
    s = s.update({ effects: setGhost.of({ pos: 3, from: 3, to: 3 }) }).state;
    s = s.update({ effects: appendGhost.of('XY') }).state;
    s = s.update({ effects: appendGhost.of('Z') }).state;
    expect(ghostActive(s)).toBe(true);
    expect(ghostTextOf(s)).toBe('XYZ');
  });

  it('clearGhost и правка пользователя снимают ghost (dismiss-on-type)', () => {
    let s = stateWith('abc');
    s = s.update({ effects: setGhost.of({ pos: 3, from: 3, to: 3 }) }).state;
    s = s.update({ effects: appendGhost.of('hi') }).state;
    s = s.update({ changes: { from: 3, insert: '!' } }).state; // правка → dismiss
    expect(ghostActive(s)).toBe(false);
  });

  it('acceptGhost вставляет текст в from..to и снимает ghost (AC-IL-3)', () => {
    const view = viewWith('Hello');
    view.dispatch({ selection: { anchor: 5 }, effects: setGhost.of({ pos: 5, from: 5, to: 5 }) });
    view.dispatch({ effects: appendGhost.of(' world') });
    expect(acceptGhost(view)).toBe(true);
    expect(view.state.doc.toString()).toBe('Hello world');
    expect(view.state.selection.main.head).toBe(11);
    expect(ghostActive(view.state)).toBe(false);
    view.destroy();
  });

  it('rejectGhost убирает ghost, документ/курсор не трогает (AC-IL-4)', () => {
    const view = viewWith('Hello');
    view.dispatch({ effects: setGhost.of({ pos: 5, from: 5, to: 5 }) });
    view.dispatch({ effects: appendGhost.of(' world') });
    expect(rejectGhost(view)).toBe(true);
    expect(view.state.doc.toString()).toBe('Hello');
    expect(ghostActive(view.state)).toBe(false);
    view.destroy();
  });

  it('accept/reject без активного ghost → false (Tab/Esc проходят штатно, AC-IL-5)', () => {
    const view = viewWith('Hello');
    expect(acceptGhost(view)).toBe(false);
    expect(rejectGhost(view)).toBe(false);
    view.destroy();
  });

  it('rewrite-режим: accept заменяет диапазон from..to (AC-IL-9)', () => {
    const view = viewWith('старый текст тут');
    // имитируем выделение «старый» (0..6): ghost-превью после выделения, замена 0..6.
    view.dispatch({ effects: setGhost.of({ pos: 6, from: 0, to: 6 }) });
    view.dispatch({ effects: appendGhost.of('новый') });
    void clearGhost; // используется контроллером; тут проверяем accept-замену
    expect(acceptGhost(view)).toBe(true);
    expect(view.state.doc.toString()).toBe('новый текст тут');
    view.destroy();
  });
});
