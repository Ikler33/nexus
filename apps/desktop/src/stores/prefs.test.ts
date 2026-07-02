import { beforeEach, describe, expect, it } from 'vitest';
import { usePrefsStore } from './prefs';

describe('prefs — wikilinkLivePreview (тоггл «Чистые ссылки»)', () => {
  beforeEach(() => {
    try {
      localStorage.clear();
    } catch {
      /* node-localStorage может быть нефункционален */
    }
  });

  it('дефолт ВКЛ', () => {
    // Стор инициализируется при импорте модуля (до возможной очистки) — дефолт true.
    expect(usePrefsStore.getState().wikilinkLivePreview).toBe(true);
  });

  it('сеттер меняет стейт и персистит в localStorage', () => {
    usePrefsStore.getState().setWikilinkLivePreview(false);
    expect(usePrefsStore.getState().wikilinkLivePreview).toBe(false);
    expect(localStorage.getItem('nexus.editor.wikilinkLivePreview')).toBe('false');

    usePrefsStore.getState().setWikilinkLivePreview(true);
    expect(usePrefsStore.getState().wikilinkLivePreview).toBe(true);
    expect(localStorage.getItem('nexus.editor.wikilinkLivePreview')).toBe('true');
  });
});

describe('prefs — noteMode (EDFIX-4 F4: персист режима source/preview)', () => {
  it('дефолт source; сеттер меняет стейт и персистит по ключу nexus.editor.noteMode', () => {
    // Стор инициализирован при импорте модуля (localStorage пуст) — дефолт 'source'.
    usePrefsStore.setState({ noteMode: 'source' }); // изоляция от порядка тестов
    usePrefsStore.getState().setNoteMode('preview');
    expect(usePrefsStore.getState().noteMode).toBe('preview');
    expect(localStorage.getItem('nexus.editor.noteMode')).toBe('preview');

    usePrefsStore.getState().setNoteMode('source');
    expect(usePrefsStore.getState().noteMode).toBe('source');
    expect(localStorage.getItem('nexus.editor.noteMode')).toBe('source');
  });
});
