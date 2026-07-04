/**
 * Реестр модулей коннектора (F-8): статическая регистрация `modules.register(m)` в одном месте,
 * ДЕТЕРМИНИРОВАННЫЙ порядок активации (= порядок регистрации), скоупированный dispose. Каждому
 * модулю выдаётся `ModuleContext` — единственный вход в ядро; всё зарегистрированное авто-трекается
 * в `subscriptions` и снимается скопом при dispose.
 *
 * YAGNI v0: НИ ОДНОГО реального модуля ядро не регистрирует (каркас); news останется в ядре до F-9.
 * Динамической загрузки/манифестов/deactivate НЕТ (см. docs/dev/connector.md).
 */
import { commands } from '../commands';
import { tauriApi } from '../tauri-api';
import { onCoreEvent } from './events';
import { settingsRegistry, viewRegistry } from './registries';
import type {
  Disposable,
  ModuleContext,
  NexusModule,
  SettingsContribution,
  ViewContribution,
} from './types';
import type { Command } from '../commands';

/** Регистрирует Disposable и одновременно трекает его в `subs` (снятие скопом при dispose модуля). */
function track(subs: Disposable[], d: Disposable): Disposable {
  subs.push(d);
  return d;
}

/** Собирает `ModuleContext` поверх глобальных реестров: команды префиксуются `${moduleId}:`. */
function buildContext(moduleId: string, subs: Disposable[]): ModuleContext {
  return {
    moduleId,
    commands: {
      register: (cmd: Command) =>
        // Легализация commands-core: тот же реестр, id в namespace модуля, source=plugin (приоритет
        // хоткеев пользователь > плагин > ядро уже реализован в CommandRegistry.resolve).
        track(subs, commands.register({ ...cmd, id: `${moduleId}:${cmd.id}`, source: 'plugin' })),
    },
    views: {
      register: (view: ViewContribution) => track(subs, viewRegistry.register(view)),
      list: () => viewRegistry.list(),
      get: (id: string) => viewRegistry.get(id),
    },
    settings: {
      register: (section: SettingsContribution) => track(subs, settingsRegistry.register(section)),
      list: () => settingsRegistry.list(),
    },
    events: {
      on: (event, cb) => track(subs, onCoreEvent(event, cb)),
    },
    api: tauriApi,
    subscriptions: subs,
  };
}

class ModuleManager {
  private registered: NexusModule[] = [];
  private active = new Map<string, Disposable[]>();

  /** Статическая регистрация модуля (до `activateAll`). Дубликат id — no-op (идемпотентность). */
  register(module: NexusModule): void {
    if (this.registered.some((m) => m.id === module.id)) return;
    this.registered.push(module);
  }

  /** Активирует все зарегистрированные модули в порядке регистрации (детерминированно). */
  activateAll(): void {
    for (const module of this.registered) this.activate(module);
  }

  private activate(module: NexusModule): void {
    if (this.active.has(module.id)) return; // уже активен
    const subs: Disposable[] = [];
    module.activate(buildContext(module.id, subs));
    this.active.set(module.id, subs);
  }

  /** Снимает все вклады всех активных модулей (обратный порядок — LIFO). */
  disposeAll(): void {
    for (const subs of this.active.values()) {
      for (let i = subs.length - 1; i >= 0; i--) subs[i].dispose();
    }
    this.active.clear();
  }

  /** Список зарегистрированных модулей (диагностика/тесты). */
  list(): readonly NexusModule[] {
    return this.registered;
  }

  /** Только для тестов: снять всё и очистить реестр модулей. */
  _reset(): void {
    this.disposeAll();
    this.registered = [];
  }
}

/** Глобальный менеджер модулей коннектора (v0 — реальных модулей ноль). */
export const modules = new ModuleManager();
