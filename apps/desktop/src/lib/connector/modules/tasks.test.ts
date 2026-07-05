import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { tasksModule } from './tasks';

/**
 * F-10b (оверлей-модуль): «Задачи» через `ctx` — оверлей + команда с прежним хоткеем ⌘⇧K. Стейт
 * `tasksOpen` остаётся ядром; `disposeAll` снимает вклады скопом.
 */

afterEach(() => modules._reset());

describe('tasksModule (F-10b)', () => {
  it('activate регистрирует оверлей и команду (хоткей ⌘⇧K сохранён) через ctx', () => {
    modules.register(tasksModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('tasks');
    expect(overlay?.titleKey).toBe('commands.view.tasks');
    expect(overlay?.order).toBe(40);
    expect(overlay?.isOpen({ tasksOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ tasksOpen: false } as UIState)).toBe(false);

    const cmd = commands.get('tasks:view.tasks');
    expect(cmd?.titleKey).toBe('commands.view.tasks');
    expect(cmd?.source).toBe('plugin');
    expect(cmd?.defaultKey).toBe('mod+shift+k');
    expect(commands.get('view.tasks')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(tasksModule);
    modules.activateAll();
    expect(overlayRegistry.get('tasks')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('tasks')).toBeUndefined();
    expect(commands.get('tasks:view.tasks')).toBeUndefined();
  });
});
