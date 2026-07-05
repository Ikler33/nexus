import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { goalsModule } from './goals';

/**
 * F-10b (оверлей-модуль): «Цели» подключаются в ядро ТОЛЬКО через `ctx` — модуль регистрирует оверлей
 * (компонент + `isOpen`-селектор) и команду палитры, а `disposeAll` снимает их скопом. Стейт видимости
 * `goalsOpen` остаётся ядром (ui-стор) — модуль лишь читает его селектором.
 */

afterEach(() => modules._reset());

describe('goalsModule (F-10b)', () => {
  it('activate регистрирует оверлей (order/titleKey/isOpen) и команду палитры через ctx', () => {
    modules.register(goalsModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('goals');
    expect(overlay?.titleKey).toBe('commands.view.goals');
    expect(overlay?.order).toBe(10);
    // isOpen — селектор поверх ядрового флага `goalsOpen`.
    expect(overlay?.isOpen({ goalsOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ goalsOpen: false } as UIState)).toBe(false);

    // Команда палитры — id префиксован модулем (`goals:view.goals`), source=plugin.
    const cmd = commands.get('goals:view.goals');
    expect(cmd?.titleKey).toBe('commands.view.goals');
    expect(cmd?.source).toBe('plugin');
    // Непрефиксованного id `view.goals` в реестре больше НЕТ (ядро его не регистрирует).
    expect(commands.get('view.goals')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(goalsModule);
    modules.activateAll();
    expect(overlayRegistry.get('goals')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('goals')).toBeUndefined();
    expect(commands.get('goals:view.goals')).toBeUndefined();
  });
});
