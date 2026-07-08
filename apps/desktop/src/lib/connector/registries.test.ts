import { describe, expect, it } from 'vitest';
import './core-views'; // сайд-эффект: регистрирует ядровые вью (для проверки легализации)
import { panelRegistry, settingsRegistry, viewRegistry } from './registries';
import type { PanelContribution, SettingsContribution, ViewContribution } from './types';

/**
 * Реестры вкладов коннектора (F-8): register/get/list-детерминизм/dispose/идемпотентность. Ядровые
 * вью (home/…/editor) зарегистрированы сайд-эффектом core-views. Тесты добавляют/снимают СВОИ вклады
 * (id с префиксом `t:`) и не трогают ядровые.
 */

const noop = () => {};
const nullComp = () => null;

function view(id: string, order: number): ViewContribution {
  return {
    id,
    titleKey: `t.${id}`,
    icon: nullComp,
    order,
    component: nullComp,
    activate: noop,
    isActive: (v) => v === id,
  };
}

function section(id: string, order: number): SettingsContribution {
  return { id, titleKey: `t.${id}`, icon: nullComp, order, component: nullComp };
}

function panel(id: string): PanelContribution {
  return { id, titleKey: `t.${id}`, component: nullComp };
}

describe('viewRegistry (F-8)', () => {
  it('register → get возвращает вклад; dispose удаляет', () => {
    const d = viewRegistry.register(view('t:one', 5));
    expect(viewRegistry.get('t:one')?.titleKey).toBe('t.t:one');
    d.dispose();
    expect(viewRegistry.get('t:one')).toBeUndefined();
  });

  it('list() детерминирован — сортировка по order (независимо от порядка регистрации)', () => {
    const d3 = viewRegistry.register(view('t:c', 300));
    const d1 = viewRegistry.register(view('t:a', 100));
    const d2 = viewRegistry.register(view('t:b', 200));
    const ids = viewRegistry
      .list()
      .filter((v) => v.id.startsWith('t:'))
      .map((v) => v.id);
    expect(ids).toEqual(['t:a', 't:b', 't:c']);
    d1.dispose();
    d2.dispose();
    d3.dispose();
  });

  it('идемпотентность: повторная регистрация того же id заменяет, не дублирует', () => {
    const d1 = viewRegistry.register(view('t:dup', 10));
    const d2 = viewRegistry.register({ ...view('t:dup', 10), titleKey: 't.replaced' });
    const dups = viewRegistry.list().filter((v) => v.id === 't:dup');
    expect(dups).toHaveLength(1);
    expect(dups[0].titleKey).toBe('t.replaced');
    d1.dispose();
    d2.dispose();
    expect(viewRegistry.get('t:dup')).toBeUndefined();
  });

  it('ядровые вью легализованы: home/today/editor присутствуют', () => {
    // news (F-9), board (F-10c) и agent (F-11) вырезаны в модули — регистрируются через ctx при
    // активации модуля, не в core-views; здесь проверяем ТОЛЬКО ядровые вью. Регистрацию их вью
    // покрывают modules/news.test.ts, modules/board.test.ts и modules/agent.test.ts.
    for (const id of ['home', 'today', 'editor']) {
      expect(viewRegistry.get(id), id).toBeDefined();
    }
    expect(viewRegistry.get('news'), 'news — модуль, не ядровая вью').toBeUndefined();
    expect(viewRegistry.get('board'), 'board — модуль, не ядровая вью').toBeUndefined();
    expect(viewRegistry.get('agent'), 'agent — модуль, не ядровая вью').toBeUndefined();
    // Editor — дефолт-вью, не в ActivityBar; остальные — в ActivityBar.
    expect(viewRegistry.get('editor')?.activityBar).toBe(false);
    expect(viewRegistry.get('home')?.activityBar).toBe(true);
  });
});

describe('panelRegistry (F-12)', () => {
  it('register → get возвращает вклад; list содержит его; dispose удаляет', () => {
    const d = panelRegistry.register(panel('t:chat'));
    expect(panelRegistry.get('t:chat')?.titleKey).toBe('t.t:chat');
    expect(panelRegistry.list().map((p) => p.id)).toContain('t:chat');
    d.dispose();
    expect(panelRegistry.get('t:chat')).toBeUndefined();
    expect(panelRegistry.list().some((p) => p.id === 't:chat')).toBe(false);
  });

  it('идемпотентность: повторная регистрация того же id заменяет, не дублирует', () => {
    const d1 = panelRegistry.register(panel('t:dup'));
    const d2 = panelRegistry.register({ ...panel('t:dup'), titleKey: 't.replaced' });
    const dups = panelRegistry.list().filter((p) => p.id === 't:dup');
    expect(dups).toHaveLength(1);
    expect(dups[0].titleKey).toBe('t.replaced');
    d1.dispose();
    d2.dispose();
    expect(panelRegistry.get('t:dup')).toBeUndefined();
  });
});

describe('settingsRegistry (F-8)', () => {
  it('register/get/list по order + dispose', () => {
    const d2 = settingsRegistry.register(section('t:s2', 220));
    const d1 = settingsRegistry.register(section('t:s1', 210));
    const ids = settingsRegistry
      .list()
      .filter((s) => s.id.startsWith('t:'))
      .map((s) => s.id);
    expect(ids).toEqual(['t:s1', 't:s2']);
    d1.dispose();
    d2.dispose();
    expect(settingsRegistry.list().some((s) => s.id.startsWith('t:'))).toBe(false);
  });
});
