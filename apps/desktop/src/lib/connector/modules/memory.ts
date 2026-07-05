/**
 * F-10b — «Память ИИ» (MEM-4) как оверлей-модуль через overlays-реестр (F-8c). Весь фронт-вклад
 * идёт через `ctx`; ядро (App/core-overlays) больше НЕ импортирует `components/memory`.
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт `memoryOpen` + действия `openMemory/closeMemory/toggleMemory`
 * ОСТАЮТСЯ ядром (ui-стор) — модуль даёт КОМПОНЕНТ + `isOpen`-селектор + команду. Кнопка «Память ИИ…»
 * в секции настроек AI/Модели (`SettingsView`) — ядро-chrome, зовёт `openMemory()` ui-стора (это не
 * импорт `components/memory`). Тоггл «Память агента» (agentMemory) в той же ai-секции — тоже ядро
 * (тоггл фичи, НЕ отдельная секция настроек).
 */
import { MemoryPanel } from '../../../components/memory/MemoryPanel';
import { useUIStore } from '../../../stores/ui';
import { useVaultStore } from '../../../stores/vault';
import type { NexusModule } from '../types';

/** Модуль «Память ИИ» (MEM-4 — явные факты памяти агента). */
export const memoryModule: NexusModule = {
  id: 'memory',
  activate(ctx) {
    // Оверлей: order=20 (прежний DOM-порядок App.tsx) — перенесён из core-overlays КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'memory',
      titleKey: 'commands.view.memory',
      order: 20,
      isOpen: (s) => s.memoryOpen,
      component: MemoryPanel,
    });

    // Команда палитры (прежняя commands-core `view.memory`): id → `memory:view.memory`, source=plugin.
    // Vault-guard сохранён КАК ЕСТЬ (нет vault — нечего показывать).
    ctx.commands.register({
      id: 'view.memory',
      title: 'AI memory',
      titleKey: 'commands.view.memory',
      run: () => {
        if (!useVaultStore.getState().info) return;
        useUIStore.getState().toggleMemory();
      },
    });
  },
};
