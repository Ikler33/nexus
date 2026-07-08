import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import { modules } from '../module-manager';
import { viewRegistry } from '../registries';
import { agentModule } from './agent';

/**
 * F-11 (вырез САМОЙ связанной фичи): agent подключается в ядро ТОЛЬКО через `ctx` — модуль
 * регистрирует main-вью (lazy AgentView + кнопка ActivityBar) и команду палитры, а `disposeAll`
 * снимает их скопом. Зеркало news.test/board.test — доказывает, что вклады agent появляются в
 * реестрах коннектора (а не через прямые правки ядра) и корректно снимаются.
 */

afterEach(() => modules._reset());

describe('agentModule (F-11)', () => {
  it('activate регистрирует main-вью и команду через ctx (behavior-preserving)', () => {
    modules.register(agentModule);
    modules.activateAll();

    // Main-вью «Агент» — с прежними order/titleKey/activityBar/suspense (behavior-preserving).
    const view = viewRegistry.get('agent');
    expect(view?.titleKey).toBe('commands.view.agent');
    expect(view?.order).toBe(50);
    expect(view?.isActive('agent')).toBe(true);
    expect(view?.isActive('news')).toBe(false);
    expect(view?.activityBar).toBe(true);
    expect(view?.suspense).toBe(true);

    // Команда палитры — id префиксован модулем (`agent:view.agent`), source=plugin.
    const cmd = commands.get('agent:view.agent');
    expect(cmd?.titleKey).toBe('commands.view.agent');
    expect(cmd?.source).toBe('plugin');
    // Непрефиксованного id `view.agent` в реестре больше НЕТ (ядро его не регистрирует).
    expect(commands.get('view.agent')).toBeUndefined();
  });

  it('disposeAll снимает все вклады agent скопом (вью + команда)', () => {
    modules.register(agentModule);
    modules.activateAll();
    expect(viewRegistry.get('agent')).toBeDefined();

    modules.disposeAll();
    expect(viewRegistry.get('agent')).toBeUndefined();
    expect(commands.get('agent:view.agent')).toBeUndefined();
  });
});
