import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import { modules } from '../module-manager';
import { panelRegistry } from '../registries';
import { chatModule } from './chat';

/**
 * F-12 (ФИНАЛ модуляризации фронта): chat/AiPanel подключается в ядро ТОЛЬКО через `ctx` — модуль
 * регистрирует workspace-панель (реестр `panels`) и команду палитры, а `disposeAll` снимает их скопом.
 * Зеркало agent.test/news.test — доказывает, что вклады chat приходят из реестров коннектора (а не
 * прямым импортом ядра, убранным из App.tsx) и корректно снимаются.
 */

afterEach(() => modules._reset());

describe('chatModule (F-12)', () => {
  it('activate регистрирует workspace-панель и команду через ctx (behavior-preserving)', () => {
    modules.register(chatModule);
    modules.activateAll();

    // Панель «AI-панель» — id `chat`, i18n-ключ имени (плашка ErrorBoundary), компонент из реестра.
    const panel = panelRegistry.get('chat');
    expect(panel?.titleKey).toBe('chrome.aiPanel');
    expect(panel?.component).toBeTypeOf('function');
    expect(panelRegistry.list().map((p) => p.id)).toContain('chat');

    // Команда палитры — id префиксован модулем (`chat:view.chat`), source=plugin, хоткей ⌘J сохранён.
    const cmd = commands.get('chat:view.chat');
    expect(cmd?.titleKey).toBe('commands.view.chat');
    expect(cmd?.source).toBe('plugin');
    expect(cmd?.defaultKey).toBe('mod+j');
    // Непрефиксованного id `view.chat` в реестре больше НЕТ (ядро его не регистрирует).
    expect(commands.get('view.chat')).toBeUndefined();
  });

  it('disposeAll снимает все вклады chat скопом (панель + команда)', () => {
    modules.register(chatModule);
    modules.activateAll();
    expect(panelRegistry.get('chat')).toBeDefined();

    modules.disposeAll();
    expect(panelRegistry.get('chat')).toBeUndefined();
    expect(commands.get('chat:view.chat')).toBeUndefined();
  });
});
