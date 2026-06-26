import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Editor } from './Editor';
import { getActiveEditorView } from '../../lib/editor/activeView';
import { usePrefsStore } from '../../stores/prefs';
import type { Buffer } from '../../stores/workspace';
import { useWorkspaceStore } from '../../stores/workspace';

const buf = (path: string, doc: string): Buffer => ({ path, doc, dirty: false, baseHash: '' });

describe('Editor (Ф0-5, контракт CM6↔React)', () => {
  // Эти тесты проверяют буфер/курсор-механику, не рендер вики-ссылок. Live-preview (дефолт ON) скрыл
  // бы `[[…]]`-скобки и мешал ассёртам на сырой текст — выключаем его на время этих тестов.
  beforeEach(() => usePrefsStore.getState().setWikilinkLivePreview(false));
  afterEach(() => usePrefsStore.getState().setWikilinkLivePreview(true));

  it('рендерит документ и заменяет его при смене файла (без пересоздания)', async () => {
    const { rerender } = render(<Editor groupId="g" path="A.md" initialDoc="Alpha content here" />);
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('Alpha'));

    rerender(<Editor groupId="g" path="B.md" initialDoc="Bravo content here" />);
    await waitFor(() => expect(host.textContent).toContain('Bravo'));
    expect(host.textContent).not.toContain('Alpha');
  });

  it('сообщает об изменениях документа через onChange', async () => {
    let captured = '';
    render(
      <Editor groupId="g" path="A.md" initialDoc="start" onChange={(d) => { captured = d; }} />,
    );
    await waitFor(() => expect(screen.getByTestId('editor').textContent).toContain('start'));
    // onChange зовётся только при правках; стартовая загрузка не считается изменением.
    expect(captured).toBe('');
  });

  it('смена файла не считается правкой (регресс: externalSync, нет ложного dirty)', async () => {
    let changes = 0;
    const { rerender } = render(
      <Editor groupId="g" path="A.md" initialDoc="aaa" onChange={() => { changes += 1; }} />,
    );
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('aaa'));
    rerender(<Editor groupId="g" path="B.md" initialDoc="bbb" onChange={() => { changes += 1; }} />);
    await waitFor(() => expect(host.textContent).toContain('bbb'));
    expect(changes).toBe(0);
  });

  it('внешнее изменение того же файла синкается в редактор без ложного dirty (Ф1-9 accept / watcher)', async () => {
    let changes = 0;
    const { rerender } = render(
      <Editor groupId="g" path="A.md" initialDoc="hello" onChange={() => { changes += 1; }} />,
    );
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('hello'));

    // Тот же path, новый doc (как accept дописал [[wikilink]]) → отражается, onChange НЕ зовётся.
    rerender(<Editor groupId="g" path="A.md" initialDoc="hello [[B]]" onChange={() => { changes += 1; }} />);
    await waitFor(() => expect(host.textContent).toContain('[[B]]'));
    expect(changes).toBe(0);
  });

  // NAV-4: позиция курсора сохраняется при уходе и восстанавливается при возврате.
  it('восстанавливает позицию курсора при возврате к заметке', async () => {
    useWorkspaceStore.getState().reset();
    useWorkspaceStore.setState({
      buffers: { 'A.md': buf('A.md', 'Alpha content here'), 'B.md': buf('B.md', 'Bravo content here') },
    });
    const { rerender } = render(<Editor groupId="g" path="A.md" initialDoc="Alpha content here" />);
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('Alpha'));

    // Ставим курсор на offset 7 в A.md.
    const view = getActiveEditorView();
    expect(view).not.toBeNull();
    view!.dispatch({ selection: { anchor: 7 } });

    // Уходим в B.md (сохранит курсор A=7), затем возвращаемся в A.md (восстановит).
    rerender(<Editor groupId="g" path="B.md" initialDoc="Bravo content here" />);
    await waitFor(() => expect(host.textContent).toContain('Bravo'));
    expect(useWorkspaceStore.getState().buffers['A.md'].cursor).toBe(7);

    rerender(<Editor groupId="g" path="A.md" initialDoc="Alpha content here" />);
    await waitFor(() => expect(host.textContent).toContain('Alpha'));
    expect(getActiveEditorView()!.state.selection.main.head).toBe(7);
  });

  it('кламп курсора при усохшем документе (внешняя правка укоротила файл)', async () => {
    useWorkspaceStore.getState().reset();
    useWorkspaceStore.setState({
      buffers: { 'A.md': buf('A.md', 'long original text'), 'B.md': buf('B.md', 'other') },
    });
    const { rerender } = render(<Editor groupId="g" path="A.md" initialDoc="long original text" />);
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('long'));
    getActiveEditorView()!.dispatch({ selection: { anchor: 15 } }); // ближе к концу

    rerender(<Editor groupId="g" path="B.md" initialDoc="other" />);
    await waitFor(() => expect(host.textContent).toContain('other'));
    // Возврат к укороченной версии A.md (len 3) — курсор клампится в пределы, без краша.
    rerender(<Editor groupId="g" path="A.md" initialDoc="abc" />);
    await waitFor(() => expect(host.textContent).toContain('abc'));
    expect(getActiveEditorView()!.state.selection.main.head).toBe(3);
  });
});

describe('Editor — live-preview вики-ссылок под Compartment (тоггл «Чистые ссылки»)', () => {
  afterEach(() => usePrefsStore.getState().setWikilinkLivePreview(true));

  it('ON (дефолт): скобки скрыты, виден лейбл; OFF: скобки видны (reconfigure без пересоздания)', async () => {
    usePrefsStore.getState().setWikilinkLivePreview(true);
    render(<Editor groupId="g" path="A.md" initialDoc="see [[Note]] here" />);
    const host = screen.getByTestId('editor');
    // ON → `[[`/`]]` скрыты, в DOM только имя.
    await waitFor(() => expect(host.textContent).toBe('see Note here'));
    const view = getActiveEditorView();
    expect(view!.state.doc.toString()).toBe('see [[Note]] here'); // документ цел

    // Выключаем тоггл — Compartment reconfigure: скобки снова видны (view НЕ пересоздан).
    usePrefsStore.getState().setWikilinkLivePreview(false);
    await waitFor(() => expect(host.textContent).toBe('see [[Note]] here'));
    expect(getActiveEditorView()).toBe(view); // тот же инстанс view

    // Снова включаем — скобки опять скрыты.
    usePrefsStore.getState().setWikilinkLivePreview(true);
    await waitFor(() => expect(host.textContent).toBe('see Note here'));
  });

  it('ON: курсор внутри ссылки раскрывает сырой синтаксис (реакция на selectionSet через view)', async () => {
    usePrefsStore.getState().setWikilinkLivePreview(true);
    render(<Editor groupId="g" path="A.md" initialDoc="pre [[Note]] post" />);
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toBe('pre Note post'));

    // Ставим курсор внутрь ссылки (offset 6 — между `[[` и `]]`) → раскрытие, виден сырой `[[Note]]`.
    getActiveEditorView()!.dispatch({ selection: { anchor: 6 } });
    await waitFor(() => expect(host.textContent).toBe('pre [[Note]] post'));

    // Уводим курсор наружу (offset 0) → снова скрыто.
    getActiveEditorView()!.dispatch({ selection: { anchor: 0 } });
    await waitFor(() => expect(host.textContent).toBe('pre Note post'));
  });
});
