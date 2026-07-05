import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import { modules } from '../module-manager';
import { settingsRegistry, viewRegistry } from '../registries';
import { newsModule } from './news';

/**
 * F-9 (пилот вырезания): news подключается в ядро ТОЛЬКО через `ctx` — модуль регистрирует main-вью,
 * секцию настроек и команду палитры, а `disposeAll` снимает их скопом. Доказывает, что вклады news
 * появляются в реестрах коннектора (а не через прямые правки ядра) и корректно снимаются.
 */

afterEach(() => modules._reset());

describe('newsModule (F-9)', () => {
  it('activate регистрирует main-вью, секцию настроек и команду через ctx', () => {
    modules.register(newsModule);
    modules.activateAll();

    // Main-вью «Новости» — с прежними order/titleKey/activityBar (behavior-preserving).
    const view = viewRegistry.get('news');
    expect(view?.titleKey).toBe('commands.view.news');
    expect(view?.order).toBe(30);
    expect(view?.activityBar).toBe(true);

    // Секция настроек «Новости».
    const section = settingsRegistry.list().find((s) => s.id === 'news');
    expect(section?.titleKey).toBe('settings.news.title');
    expect(section?.order).toBe(50);

    // Команда палитры — id префиксован модулем (`news:view.news`), source=plugin.
    const cmd = commands.get('news:view.news');
    expect(cmd?.titleKey).toBe('commands.view.news');
    expect(cmd?.source).toBe('plugin');
    // Непрефиксованного id `view.news` в реестре больше НЕТ (ядро его не регистрирует).
    expect(commands.get('view.news')).toBeUndefined();
  });

  it('disposeAll снимает все вклады news скопом (вью + секция + команда)', () => {
    modules.register(newsModule);
    modules.activateAll();
    expect(viewRegistry.get('news')).toBeDefined();

    modules.disposeAll();
    expect(viewRegistry.get('news')).toBeUndefined();
    expect(settingsRegistry.list().some((s) => s.id === 'news')).toBe(false);
    expect(commands.get('news:view.news')).toBeUndefined();
  });
});
