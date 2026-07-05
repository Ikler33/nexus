/**
 * F-10b — «Задачи» (TASK-1, сводка всех `- [ ]` vault) как оверлей-модуль через overlays-реестр
 * (F-8c). Ядро больше НЕ импортирует `components/tasks`.
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт `tasksOpen` + `closeTasks/toggleTasks` остаются ядром (ui-стор);
 * модуль даёт КОМПОНЕНТ + `isOpen`-селектор + команду палитры (с прежним хоткеем ⌘⇧K). Кнопка «Задачи»
 * в ActivityBar + пункт «Сегодня»-вью — ядро-chrome, зовут `toggleInbox`/`toggleTasks` ui-стора (НЕ
 * импорт `components/tasks`).
 */
import { TasksPanel } from '../../../components/tasks/TasksPanel';
import { useUIStore } from '../../../stores/ui';
import { useVaultStore } from '../../../stores/vault';
import type { NexusModule } from '../types';

/** Модуль «Задачи» (TASK-1). */
export const tasksModule: NexusModule = {
  id: 'tasks',
  activate(ctx) {
    // Оверлей: order=40 (прежний DOM-порядок App.tsx) — перенос из core-overlays КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'tasks',
      titleKey: 'commands.view.tasks',
      order: 40,
      isOpen: (s) => s.tasksOpen,
      component: TasksPanel,
    });

    // Команда палитры (прежняя commands-core `view.tasks`): id → `tasks:view.tasks`, source=plugin.
    // Хоткей ⌘⇧K и vault-guard сохранены КАК ЕСТЬ (resolve матчит defaultKey независимо от source).
    ctx.commands.register({
      id: 'view.tasks',
      title: 'Tasks',
      titleKey: 'commands.view.tasks',
      defaultKey: 'mod+shift+k',
      run: () => {
        if (!useVaultStore.getState().info) return; // нет vault — нечего сканировать
        useUIStore.getState().toggleTasks();
      },
    });
  },
};
