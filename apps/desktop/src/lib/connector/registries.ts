/**
 * Синглтон-реестры вкладов коннектора (F-8): main-вью и секции настроек. Ключ — `id` вклада (Map →
 * идемпотентность: повторная регистрация того же id заменяет запись, напр. при HMR/повторном
 * импорте). `list()` детерминирован — сортировка по `order` (стабильна). Реестр команд отдельный —
 * это существующий `commands-core` (легализуется тонкой обёрткой в module-manager).
 */
import type {
  OverlayContribution,
  OverlaysRegistry,
  PanelContribution,
  PanelsRegistry,
  SettingsContribution,
  SettingsRegistry,
  ViewContribution,
  ViewsRegistry,
  Disposable,
} from './types';

class ViewRegistryImpl implements ViewsRegistry {
  private map = new Map<string, ViewContribution>();

  register(view: ViewContribution): Disposable {
    this.map.set(view.id, view);
    return { dispose: () => this.map.delete(view.id) };
  }

  get(id: string): ViewContribution | undefined {
    return this.map.get(id);
  }

  /** Все вью, отсортированные по `order` (детерминированный порядок ActivityBar/реестра). */
  list(): ViewContribution[] {
    return [...this.map.values()].sort((a, b) => a.order - b.order);
  }

  /** Только для тестов: полный сброс. */
  _reset(): void {
    this.map.clear();
  }
}

class SettingsRegistryImpl implements SettingsRegistry {
  private map = new Map<string, SettingsContribution>();

  register(section: SettingsContribution): Disposable {
    this.map.set(section.id, section);
    return { dispose: () => this.map.delete(section.id) };
  }

  /** Все секции, отсортированные по `order`. */
  list(): SettingsContribution[] {
    return [...this.map.values()].sort((a, b) => a.order - b.order);
  }

  /** Только для тестов: полный сброс. */
  _reset(): void {
    this.map.clear();
  }
}

class OverlayRegistryImpl implements OverlaysRegistry {
  private map = new Map<string, OverlayContribution>();

  register(overlay: OverlayContribution): Disposable {
    this.map.set(overlay.id, overlay);
    return { dispose: () => this.map.delete(overlay.id) };
  }

  get(id: string): OverlayContribution | undefined {
    return this.map.get(id);
  }

  /** Все оверлеи, отсортированные по `order` (детерминированный DOM-порядок OverlayOutlet). */
  list(): OverlayContribution[] {
    return [...this.map.values()].sort((a, b) => a.order - b.order);
  }

  /** Только для тестов: полный сброс. */
  _reset(): void {
    this.map.clear();
  }
}

class PanelRegistryImpl implements PanelsRegistry {
  private map = new Map<string, PanelContribution>();

  register(panel: PanelContribution): Disposable {
    this.map.set(panel.id, panel);
    return { dispose: () => this.map.delete(panel.id) };
  }

  get(id: string): PanelContribution | undefined {
    return this.map.get(id);
  }

  /** Все панели детерминированно (по вставке — сортировка не нужна, в проде один вклад). */
  list(): PanelContribution[] {
    return [...this.map.values()];
  }

  /** Только для тестов: полный сброс. */
  _reset(): void {
    this.map.clear();
  }
}

/** Глобальный реестр main-вью (питает MainViewOutlet + ActivityBar). */
export const viewRegistry = new ViewRegistryImpl();

/** Глобальный реестр секций настроек (питает SettingsView). */
export const settingsRegistry = new SettingsRegistryImpl();

/** Глобальный реестр оверлеев (F-8c — питает OverlayOutlet: goals/memory/…/contradictions). */
export const overlayRegistry = new OverlayRegistryImpl();

/** Глобальный реестр workspace-панелей (F-12 — питает AiPanelOutlet; в проде один вклад: chat). */
export const panelRegistry = new PanelRegistryImpl();
