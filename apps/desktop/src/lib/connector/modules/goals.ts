/**
 * F-10b — «Цели» (#35) как вырезанный модуль через overlays-реестр (F-8c). Шаблон оверлей-модуля
 * (см. docs/dev/connector.md «F-10b: вырез оверлея в модуль»): весь фронт-вклад «Целей» идёт через
 * `ctx`, ядро (App/Titlebar/core-overlays) больше НЕ импортирует `components/goals`.
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт видимости `goalsOpen` + действия `openGoals/closeGoals/toggleGoals`
 * + Esc-прецедент ОСТАЮТСЯ ядром (ui-стор, как `mainView`) — модуль даёт КОМПОНЕНТ + `isOpen`-селектор
 * поверх ядрового флага + команду палитры. grep-инвариант «ядро не импортирует components/goals»
 * достигается переносом ИМПОРТА панели сюда (стейт ≠ импорт компонента). AI-меню-пункт «Цели» Titlebar
 * остаётся ядро-chrome (titlebar-menu-реестра нет по решению F-8c) — он зовёт `toggleGoals()` ui-стора.
 */
import { GoalsPanel } from '../../../components/goals/GoalsPanel';
import { useGoalsStore } from '../../../stores/goals';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Цели» (#35, vision). Оверлей + команда палитры + живой пересчёт по индексатору. */
export const goalsModule: NexusModule = {
  id: 'goals',
  activate(ctx) {
    // Оверлей: order=10 (прежний DOM-порядок App.tsx, стекинг) — перенесён из core-overlays КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'goals',
      titleKey: 'commands.view.goals',
      order: 10,
      isOpen: (s) => s.goalsOpen,
      component: GoalsPanel,
    });

    // Команда палитры (прежняя commands-core `view.goals`). `ctx.commands` префиксует id →
    // `goals:view.goals`, source=plugin. `toggleGoals` — прежняя тоггл-семантика (без vault-guard).
    ctx.commands.register({
      id: 'view.goals',
      title: 'Goals',
      titleKey: 'commands.view.goals',
      run: () => useUIStore.getState().toggleGoals(),
    });

    // Живой пересчёт «Целей» по событию индексатора (ADR-007 S8, AC-GP-3) — перенос фича-эффекта App.tsx
    // (F-8b миграция). Только когда панель открыта; дебаунс 800ms (событий может быть пачка). Таймер
    // снимается при dispose модуля (тесты/HMR) — вклад в `subscriptions`.
    let timer: ReturnType<typeof setTimeout> | undefined;
    ctx.events.on('vault:changed', () => {
      if (!useUIStore.getState().goalsOpen) return;
      clearTimeout(timer);
      timer = setTimeout(() => void useGoalsStore.getState().load(), 800);
    });
    ctx.subscriptions.push({ dispose: () => clearTimeout(timer) });
  },
};
