import { afterEach, beforeAll, describe, expect, it } from 'vitest';
import { commands, eventToCombo, normalizeCombo, spellCombo } from './commands';
import { registerCoreCommands } from './commands-core';
import { useWorkspaceStore } from '../stores/workspace';

describe('spellCombo (a11y: произносимая метка хоткея)', () => {
  it('разворачивает модификаторы в слова для скринридера', () => {
    expect(spellCombo('mod+shift+p')).toBe('Mod Shift P');
    expect(spellCombo('meta+/')).toBe('Cmd /');
    expect(spellCombo('ctrl+alt+k')).toBe('Ctrl Alt K');
  });
});

// jsdom под node 25 не отдаёт рабочий localStorage (нативный экспериментальный global мешает),
// а реестр хоткеев в него персистит. In-memory localStorage для теста персиста (стартует пустым).
beforeAll(() => {
  const store = new Map<string, string>();
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (k: string) => (store.has(k) ? (store.get(k) as string) : null),
      setItem: (k: string, v: string) => void store.set(k, String(v)),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
    },
  });
});

afterEach(() => commands._reset());

describe('command registry (Ф0-8)', () => {
  it('register / list / run / dispose', async () => {
    let ran = 0;
    const d = commands.register({ id: 't.a', title: 'A', run: () => { ran += 1; } });
    expect(commands.list().map((c) => c.id)).toContain('t.a');
    await commands.run('t.a');
    expect(ran).toBe(1);
    d.dispose();
    expect(commands.list().map((c) => c.id)).not.toContain('t.a');
  });

  it('normalizeCombo: фикс. порядок модификаторов, mod→ctrl (не-mac)', () => {
    expect(normalizeCombo('mod+p')).toBe('ctrl+p');
    expect(normalizeCombo('Shift+Ctrl+K')).toBe('ctrl+shift+k');
  });

  it('eventToCombo из события', () => {
    const e = { ctrlKey: true, metaKey: false, altKey: false, shiftKey: false, key: 'P' } as KeyboardEvent;
    expect(eventToCombo(e)).toBe('ctrl+p');
  });

  it('resolve: приоритет пользователь > плагин > ядро', () => {
    commands.register({ id: 'core.x', title: 'core', source: 'core', defaultKey: 'mod+k', run: () => {} });
    commands.register({ id: 'plugin.x', title: 'plugin', source: 'plugin', defaultKey: 'mod+k', run: () => {} });
    expect(commands.resolve('mod+k')).toBe('plugin.x'); // плагин > ядро
    commands.setUserKey('mod+k', 'core.x');
    expect(commands.resolve('mod+k')).toBe('core.x'); // пользователь перекрывает
  });

  it('remap / effectiveKey / userKeyFor / resetKey (слайс 4)', () => {
    commands.register({ id: 'c.g', title: 'g', source: 'core', defaultKey: 'mod+g', run: () => {} });
    // Дефолт (jsdom = не-mac → mod→ctrl).
    expect(commands.effectiveKey('c.g')).toBe('ctrl+g');
    expect(commands.userKeyFor('c.g')).toBeUndefined();

    commands.remap('c.g', 'mod+shift+g');
    expect(commands.userKeyFor('c.g')).toBe('ctrl+shift+g');
    expect(commands.effectiveKey('c.g')).toBe('ctrl+shift+g');
    expect(commands.resolve('mod+shift+g')).toBe('c.g');

    // Повторный ремап снимает прежний пользовательский бинд (не остаётся двух комбо у одной команды).
    commands.remap('c.g', 'mod+alt+g');
    expect(commands.resolve('ctrl+shift+g')).toBeUndefined();
    expect(commands.effectiveKey('c.g')).toBe('ctrl+alt+g');

    commands.resetKey('c.g');
    expect(commands.userKeyFor('c.g')).toBeUndefined();
    expect(commands.effectiveKey('c.g')).toBe('ctrl+g'); // снова дефолт
  });

  it('ремап персистится в localStorage (слайс 4)', () => {
    commands.register({ id: 'c.k', title: 'k', source: 'core', defaultKey: 'mod+k', run: () => {} });
    commands.remap('c.k', 'mod+shift+k');
    const raw = localStorage.getItem('nexus.hotkeys.v1');
    expect(raw).toBeTruthy();
    expect(JSON.parse(raw as string)).toEqual({ 'ctrl+shift+k': 'c.k' });
  });
});

describe('editor.toggleMode (⌘E — source/preview, регресс)', () => {
  it('mod+e резолвится в команду и тогглит режим активной группы туда-обратно', async () => {
    const reg = registerCoreCommands();
    try {
      expect(commands.resolve(normalizeCombo('mod+e'))).toBe('editor.toggleMode');
      const gid = useWorkspaceStore.getState().activeGroupId;
      const before = useWorkspaceStore.getState().modes[gid] ?? 'source';
      await commands.run('editor.toggleMode');
      const after = useWorkspaceStore.getState().modes[gid] ?? 'source';
      expect(after).not.toBe(before);
      // повторный вызов возвращает обратно (это тоггл)
      await commands.run('editor.toggleMode');
      expect(useWorkspaceStore.getState().modes[gid] ?? 'source').toBe(before);
    } finally {
      reg.dispose();
    }
  });
});
