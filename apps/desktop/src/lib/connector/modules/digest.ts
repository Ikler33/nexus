/**
 * F-10b — «Дайджест изменений» (#35, ADR-007 slice 4) как оверлей-модуль через overlays-реестр
 * (F-8c). Плавающий float-оверлей (НЕ focus-trap): может стоять поверх trap-оверлеев. Ядро больше НЕ
 * импортирует `components/digest`.
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт `digestOpen` + `closeDigest/toggleDigest` остаются ядром (ui-стор);
 * модуль даёт КОМПОНЕНТ + `isOpen`-селектор + команду + refetch по `jobs:changed`. Пункт «Дайджест
 * изменений» в меню «AI-инсайты» Titlebar остаётся ядро-chrome (titlebar-menu-реестра нет по решению
 * F-8c — задокументировано в connector.md) → зовёт `toggleDigest()` ui-стора.
 */
import { DigestPanel } from '../../../components/digest/DigestPanel';
import { useDigestStore } from '../../../stores/digest';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Дайджест изменений» (#35). */
export const digestModule: NexusModule = {
  id: 'digest',
  activate(ctx) {
    // Оверлей: order=60 (прежний DOM-порядок App.tsx — float поверх trap-оверлеев) КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'digest',
      titleKey: 'commands.view.digest',
      order: 60,
      isOpen: (s) => s.digestOpen,
      component: DigestPanel,
    });

    // Команда палитры (прежняя commands-core `view.digest`): id → `digest:view.digest`, source=plugin.
    ctx.commands.register({
      id: 'view.digest',
      title: 'Changes digest',
      titleKey: 'commands.view.digest',
      run: () => useUIStore.getState().toggleDigest(),
    });

    // Refetch открытой панели по готовности фоновой джобы (ADR-007 slice 4/5) — перенос части
    // combined-эффекта App.tsx (`jobs:changed`) сюда: дайджест refetch'ит СВОЙ стор, только когда открыт.
    ctx.events.on('jobs:changed', () => {
      if (useUIStore.getState().digestOpen) void useDigestStore.getState().load();
    });
  },
};
