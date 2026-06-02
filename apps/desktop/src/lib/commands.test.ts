import { afterEach, describe, expect, it } from 'vitest';
import { commands, eventToCombo, normalizeCombo } from './commands';

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
});
