import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { graphModule } from './graph';

/**
 * F-10d (оверлей-модуль): «Граф» подключается в ядро ТОЛЬКО через `ctx` — модуль регистрирует оверлей
 * GraphLayer (компонент + `isOpen`-селектор) с mount:'appBody' (слой внутри тела, не поверх хрома) и
 * команду палитры, а `disposeAll` снимает их скопом. Стейт видимости `graphOpen` остаётся ядром
 * (ui-стор) — модуль лишь читает его селектором.
 */

afterEach(() => modules._reset());

describe('graphModule (F-10d)', () => {
  it('activate регистрирует оверлей mount:appBody (order/titleKey/isOpen) и команду палитры через ctx', () => {
    modules.register(graphModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('graph');
    expect(overlay?.titleKey).toBe('commands.view.graph');
    expect(overlay?.order).toBe(90);
    // mount:'appBody' — ключевое отличие F-10d: слой садится ВНУТРЬ тела (appBody-инстанс OverlayOutlet),
    // а не на уровень .app (где 8 оверлеев F-10b/c без поля mount = default 'app'). Фикс владельца.
    expect(overlay?.mount).toBe('appBody');
    // isOpen — селектор поверх ядрового флага `graphOpen`.
    expect(overlay?.isOpen({ graphOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ graphOpen: false } as UIState)).toBe(false);

    // Команда палитры — id префиксован модулем (`graph:view.graph`), source=plugin, хоткей ⌘G сохранён.
    const cmd = commands.get('graph:view.graph');
    expect(cmd?.titleKey).toBe('commands.view.graph');
    expect(cmd?.source).toBe('plugin');
    expect(cmd?.defaultKey).toBe('mod+g');
    // Непрефиксованного id `view.graph` в реестре больше НЕТ (ядро/commands-core его не регистрирует).
    expect(commands.get('view.graph')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(graphModule);
    modules.activateAll();
    expect(overlayRegistry.get('graph')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('graph')).toBeUndefined();
    expect(commands.get('graph:view.graph')).toBeUndefined();
  });
});
