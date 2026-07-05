/**
 * F-10b — «Поиск противоречий» (#vision) как оверлей-модуль через overlays-реестр (F-8c). Плавающий
 * float-оверлей (НЕ focus-trap), как «Дайджест». Ядро больше НЕ импортирует `components/contradictions`.
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт `contradictionsOpen` + `closeContradictions/toggleContradictions`
 * остаются ядром (ui-стор); модуль даёт КОМПОНЕНТ + `isOpen`-селектор + команду + refetch по
 * `jobs:changed`. Пункт «Поиск противоречий» в меню «AI-инсайты» Titlebar остаётся ядро-chrome
 * (titlebar-menu-реестра нет по решению F-8c) → зовёт `toggleContradictions()` ui-стора. Тоггл
 * «Поиск противоречий» (contradictions) в ai-секции настроек — ТОЖЕ ядро (тоггл фичи в общей ai-секции,
 * НЕ отдельная секция настроек; живёт в `useAiFeaturesStore`, не импортирует `components/contradictions`).
 */
import { ContradictionsPanel } from '../../../components/contradictions/ContradictionsPanel';
import { useContradictionsStore } from '../../../stores/contradictions';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Поиск противоречий» (#vision). */
export const contradictionsModule: NexusModule = {
  id: 'contradictions',
  activate(ctx) {
    // Оверлей: order=70 (прежний DOM-порядок App.tsx — float поверх trap-оверлеев) КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'contradictions',
      titleKey: 'commands.view.contradictions',
      order: 70,
      isOpen: (s) => s.contradictionsOpen,
      component: ContradictionsPanel,
    });

    // Команда палитры (прежняя commands-core `view.contradictions`): id → `contradictions:view.contradictions`.
    ctx.commands.register({
      id: 'view.contradictions',
      title: 'Contradiction finder',
      titleKey: 'commands.view.contradictions',
      run: () => useUIStore.getState().toggleContradictions(),
    });

    // Refetch открытой панели по готовности фоновой джобы (ADR-007 slice 4/5) — перенос последней части
    // combined-эффекта App.tsx (`jobs:changed`) сюда: свой стор, только когда панель открыта.
    ctx.events.on('jobs:changed', () => {
      if (useUIStore.getState().contradictionsOpen) void useContradictionsStore.getState().load();
    });
  },
};
