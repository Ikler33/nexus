/**
 * F-10c — «Синхронизация (git)» как вырезанный модуль через overlays-реестр (F-8c). Шаблон
 * оверлей-модуля (см. docs/dev/connector.md «F-10b: вырез оверлея в модуль»): фронт-вклад SyncPanel
 * идёт через `ctx`, ядро (App.tsx) больше НЕ рендерит `{syncOpen && <SyncPanel/>}` напрямую — панель
 * приходит из реестра `overlays` в `OverlayOutlet` (per-contribution ErrorBoundary).
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт видимости `syncOpen` + `openSync/closeSync/toggleSync` ОСТАЮТСЯ
 * ядром (ui-стор) — модуль даёт КОМПОНЕНТ + `isOpen`-селектор + команду палитры. Кнопка «Синхронизация»
 * в ActivityBar остаётся ядро-chrome (зовёт `toggleSync()` ui-стора) — как AI-меню «Цели» у goals.
 *
 * ГРАНИЦА (F-1b): зона `components/sync` содержит ДВА компонента — SyncPanel (этот оверлей) и
 * ConflictResolver (git-merge-резолвер). ConflictResolver ОСТАЁТСЯ ядром (safe-flow, вызывается
 * standalone из пилюли статусбара по `conflictOpen`, DP-14) и внутри SyncPanel. Поэтому App.tsx честно
 * импортирует ConflictResolver из зоны sync — задокументировано в MODULE_BOUNDARY_EXCEPTIONS
 * (eslint.config.js). Сам вырезаемый оверлей — только SyncPanel.
 */
import { SyncPanel } from '../../../components/sync/SyncPanel';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Синхронизация (git)». Оверлей SyncPanel + команда палитры (F-10c). */
export const syncModule: NexusModule = {
  id: 'sync',
  activate(ctx) {
    // Оверлей: order=80 (после 7 оверлеев F-10b, orders 10..70). Стекинг решает z-index панели
    // (SyncPanel z-index:50), поэтому DOM-порядок среди position:fixed-оверлеев косметичен —
    // behavior-preserving. isOpen читает ядровой флаг `syncOpen`.
    ctx.overlays.register({
      id: 'sync',
      titleKey: 'commands.view.sync',
      order: 80,
      isOpen: (s) => s.syncOpen,
      component: SyncPanel,
    });

    // Команда палитры (прежняя commands-core `view.sync`). `ctx.commands` префиксует id →
    // `sync:view.sync`, source=plugin. `toggleSync` — прежняя тоггл-семантика (без defaultKey).
    ctx.commands.register({
      id: 'view.sync',
      title: 'Sync (git)',
      titleKey: 'commands.view.sync',
      run: () => useUIStore.getState().toggleSync(),
    });
  },
};
