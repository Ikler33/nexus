import { Compartment, EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { afterEach, describe, expect, it } from 'vitest';
import {
  buildLivePreviewRanges,
  nexusExtensions,
  normalizeTarget,
  wikilinkLabelRange,
  wikilinkLivePreview,
} from './extensions';

describe('normalizeTarget (Ф0-5)', () => {
  it('возвращает цель без изменений', () => {
    expect(normalizeTarget('Note')).toBe('Note');
    expect(normalizeTarget('Projects/Roadmap')).toBe('Projects/Roadmap');
  });

  it('срезает #heading и |alias и тримит', () => {
    expect(normalizeTarget('Note#Section')).toBe('Note');
    expect(normalizeTarget('Note#Section|Alias')).toBe('Note');
    expect(normalizeTarget('  Spaced Note  ')).toBe('Spaced Note');
  });
});

describe('wikilinkLabelRange (live-preview лейбл)', () => {
  it('простая ссылка — весь inner', () => {
    expect(wikilinkLabelRange('Note')).toEqual({ start: 0, end: 4 });
  });
  it('алиас — после `|` до конца', () => {
    // `Note|Alias` → видно `Alias` (offset 5..10)
    expect(wikilinkLabelRange('Note|Alias')).toEqual({ start: 5, end: 10 });
  });
  it('heading без алиаса — target до `#`', () => {
    expect(wikilinkLabelRange('Note#Sec')).toEqual({ start: 0, end: 4 });
  });
  it('heading + алиас — после `|`', () => {
    expect(wikilinkLabelRange('Note#Sec|A')).toEqual({ start: 9, end: 10 });
  });
  it('фолбэк: только heading без target — весь inner', () => {
    expect(wikilinkLabelRange('#H')).toEqual({ start: 0, end: 2 });
  });
});

describe('buildLivePreviewRanges (live-preview декорации — чистая)', () => {
  // Курсор вне ссылки (далеко): selFrom=selTo=-1 → ничего не раскрыто.
  const FAR = -1;

  it('простая `[[Note]]` — скрывает `[[` и `]]`, виден лейбл', () => {
    const text = 'a [[Note]] b';
    // `[[` на 2..4, `Note` 4..8, `]]` 8..10
    const r = buildLivePreviewRanges(text, FAR, FAR);
    expect(r).toEqual([
      { from: 2, to: 4 }, // `[[`
      { from: 8, to: 10 }, // `]]`
    ]);
  });

  it('алиас `[[Note|Alias]]` — скрывает `[[Note|` и `]]`, виден `Alias`', () => {
    const text = '[[Note|Alias]]';
    // `[[Note|` = 0..7, `Alias` 7..12, `]]` 12..14
    const r = buildLivePreviewRanges(text, FAR, FAR);
    expect(r).toEqual([
      { from: 0, to: 7 },
      { from: 12, to: 14 },
    ]);
  });

  it('heading `[[Note#Sec]]` — скрывает `[[` и `#Sec]]`, виден `Note`', () => {
    const text = '[[Note#Sec]]';
    // `[[` 0..2, `Note` 2..6, `#Sec]]` 6..12
    const r = buildLivePreviewRanges(text, FAR, FAR);
    expect(r).toEqual([
      { from: 0, to: 2 },
      { from: 6, to: 12 },
    ]);
  });

  it('РАСКРЫТИЕ под курсором: выделение внутри ссылки → ничего не прячем', () => {
    const text = '[[Note]]'; // 0..8
    // Курсор на позиции 4 (внутри) — раскрыто.
    expect(buildLivePreviewRanges(text, 4, 4)).toEqual([]);
    // Курсор ровно на краю `]]` (8) — тоже раскрыто (края включительно — печать у конца).
    expect(buildLivePreviewRanges(text, 8, 8)).toEqual([]);
    // Курсор ровно на старте `[[` (0) — раскрыто.
    expect(buildLivePreviewRanges(text, 0, 0)).toEqual([]);
  });

  it('две ссылки: курсор в одной → раскрыта только она, вторая скрыта', () => {
    const text = '[[One]] mid [[Two]]';
    // [[One]] 0..7, [[Two]] 12..19. Курсор на 3 (внутри One).
    const r = buildLivePreviewRanges(text, 3, 3);
    // One раскрыта, Two скрыта: `[[` 12..14, `]]` 17..19
    expect(r).toEqual([
      { from: 12, to: 14 },
      { from: 17, to: 19 },
    ]);
  });

  it('эмбед `![[...]]` ПРОПУСКАЕТСЯ (не скрываем)', () => {
    const text = '![[Embed]]';
    expect(buildLivePreviewRanges(text, FAR, FAR)).toEqual([]);
  });

  it('эмбед рядом с обычной — обычная скрыта, эмбед цел', () => {
    const text = '![[Pic]] [[Note]]';
    // ![[Pic]] 0..8 (skip); [[Note]] 9..17: `[[` 9..11, `]]` 15..17
    const r = buildLivePreviewRanges(text, FAR, FAR);
    expect(r).toEqual([
      { from: 9, to: 11 },
      { from: 15, to: 17 },
    ]);
  });

  it('offset смещает координаты (построение по visibleRanges кусками)', () => {
    const text = '[[Note]]';
    const r = buildLivePreviewRanges(text, FAR, FAR, 100);
    expect(r).toEqual([
      { from: 100, to: 102 },
      { from: 106, to: 108 },
    ]);
    // Раскрытие учитывает offset: курсор на 104 (внутри сдвинутой ссылки) → пусто.
    expect(buildLivePreviewRanges(text, 104, 104, 100)).toEqual([]);
  });
});

describe('wikilinkLivePreview (интеграция в EditorView)', () => {
  const views: EditorView[] = [];
  function mount(doc: string, anchor = doc.length): EditorView {
    const parent = document.createElement('div');
    document.body.appendChild(parent);
    const view = new EditorView({
      state: EditorState.create({
        doc,
        selection: { anchor },
        extensions: [wikilinkLivePreview],
      }),
      parent,
    });
    views.push(view);
    return view;
  }
  afterEach(() => {
    while (views.length) views.pop()!.destroy();
  });

  it('скрывает `[[ ]]` — в DOM видно только имя (документ цел)', () => {
    const view = mount('start [[Note]] end');
    expect(view.dom.textContent).toBe('start Note end');
    // Исходный документ НЕ изменён — это дисплей-декорация.
    expect(view.state.doc.toString()).toBe('start [[Note]] end');
  });

  it('алиас `[[Note|Alias]]` → в DOM виден `Alias`', () => {
    // Паддинг по краям + курсор на 0 (вне ссылки), иначе край-правило раскрыло бы единственную ссылку.
    const view = mount('xx [[Note|Alias]] yy', 0);
    expect(view.dom.textContent).toBe('xx Alias yy');
    expect(view.state.doc.toString()).toBe('xx [[Note|Alias]] yy');
  });

  it('раскрытие под курсором: курсор внутри → виден сырой `[[Note]]`', () => {
    const view = mount('aaaa [[Note]] bbbb', 0); // курсор на 0 — вне ссылки (2..13)
    expect(view.dom.textContent).toBe('aaaa Note bbbb');
    // Двигаем курсор внутрь ссылки (offset 7) → раскрытие, виден сырой синтаксис (печать не вслепую).
    view.dispatch({ selection: { anchor: 7 } });
    expect(view.dom.textContent).toBe('aaaa [[Note]] bbbb');
    // Уводим курсор обратно наружу → снова скрыто (реакция на selectionSet).
    view.dispatch({ selection: { anchor: 0 } });
    expect(view.dom.textContent).toBe('aaaa Note bbbb');
  });

  it('эмбед `![[...]]` НЕ скрывается (виден целиком)', () => {
    const view = mount('![[Pic]]', 0);
    expect(view.dom.textContent).toBe('![[Pic]]');
  });

  it('скрытый синтаксис атомарен (курсор не «застревает» внутри скрытых `[[`)', () => {
    // `ab [[Note]]` — ссылка на 3..11, курсор на 0 (вне ссылки → скрыто). Скрытые диапазоны:
    // `[[` 3..5, `]]` 9..11. Проверяем, что эти диапазоны зарегистрированы как АТОМАРНЫЕ
    // (EditorView.atomicRanges) → курсор перепрыгивает их как единое целое (UX live-preview).
    const view = mount('ab [[Note]]', 0);
    const atomic = view.state.facet(EditorView.atomicRanges);
    // Внутри `[[` (offset 4) должна найтись атомарная область 3..5.
    let found: { from: number; to: number } | null = null;
    for (const set of atomic) {
      set(view).between(4, 4, (from, to) => {
        found = { from, to };
        return false;
      });
    }
    expect(found).toEqual({ from: 3, to: 5 });
  });
});

// РЕГРЕССИЯ (репорт владельца): LP в source-режиме РЕАЛЬНОГО app монтируется НЕ изолированно, а вместе
// со ВСЕМ набором `nexusExtensions` (там `decorationPlugin` ставит `Decoration.mark .cm-wikilink` поверх
// ВСЕГО `[[…]]`, а LP — атомарный `Decoration.replace` на скобках). Прежние интеграционные тесты выше
// монтировали ТОЛЬКО `[wikilinkLivePreview]` → не ловили бы конфликт mark+replace, пустую `[[ ]]`
// (zero-length-replace → краш RangeSetBuilder → весь плагин падает → скобки видны) и реальные daily-строки.
// Подтверждено идентично в реальном WebKit (WKWebView, движок Tauri).
describe('wikilinkLivePreview в ПОЛНОМ наборе nexusExtensions (mark + replace, реальный app)', () => {
  const views: EditorView[] = [];
  function mountFull(doc: string, anchor: number): EditorView {
    const parent = document.createElement('div');
    document.body.appendChild(parent);
    const lp = new Compartment();
    const view = new EditorView({
      state: EditorState.create({
        doc,
        selection: { anchor },
        extensions: [
          ...nexusExtensions({
            fetchNotes: () => Promise.resolve([]),
            fetchTags: () => Promise.resolve([]),
            getOpenLink: () => undefined,
          }),
          lp.of(wikilinkLivePreview),
        ],
      }),
      parent,
    });
    views.push(view);
    return view;
  }
  afterEach(() => {
    while (views.length) views.pop()!.destroy();
  });

  it('mark (.cm-wikilink) + LP-replace вместе: скобки скрыты, подсветка стоит', () => {
    const view = mountFull('start [[Note]] end', 0); // курсор вне ссылки
    // LP-replace всё равно прячет `[[`/`]]` несмотря на mark поверх всей ссылки (нет конфликта precedence).
    expect(view.dom.textContent).toBe('start Note end');
    // mark-подсветка `.cm-wikilink` присутствует (обе декорации сосуществуют).
    expect(view.dom.querySelector('.cm-wikilink')).not.toBeNull();
    expect(view.state.doc.toString()).toBe('start [[Note]] end'); // документ цел
  });

  it('реальные daily-строки: пустая `[[ ]]` + датированная — плагин не падает, скобки скрыты', () => {
    // Строки 38/41 реальной Daily/2026-03-01.md: пустая `[[ ]]` (inner=пробел) и `[[2026-02-24 - 2026-03-02]]`.
    const doc = '## Кандидаты\n- [[ ]]\n## Связи\n\n- [[2026-02-24 - 2026-03-02]]\nend';
    let threw: unknown = null;
    let view: EditorView | null = null;
    try {
      view = mountFull(doc, doc.length); // курсор в конце — вне всех ссылок
    } catch (e) {
      threw = e;
    }
    expect(threw).toBeNull(); // пустая `[[ ]]` НЕ роняет RangeSetBuilder/плагин
    // Скобки скрыты у обеих ссылок (документ цел): датированная превратилась в чистый лейбл.
    expect(view!.dom.textContent).not.toContain('[[');
    expect(view!.dom.textContent).not.toContain(']]');
    expect(view!.dom.textContent).toContain('2026-02-24 - 2026-03-02');
    expect(view!.state.doc.toString()).toBe(doc); // исходный текст НЕ изменён (дисплей-декорация)
  });
});
