/**
 * F-10b — «Входящие» (INBOX-1, GTD-разбор Inbox.md) как оверлей-модуль через overlays-реестр (F-8c).
 * Ядро больше НЕ импортирует `components/inbox`.
 *
 * ПАТТЕРН оверлей-модуля (v0): стейт `inboxOpen` + `closeInbox/toggleInbox` остаются ядром (ui-стор);
 * модуль даёт КОМПОНЕНТ + `isOpen`-селектор + команду палитры (без хоткея — вход из ActivityBar/палитры).
 * Кнопка «Входящие» в ActivityBar + действие «Сегодня»-вью — ядро-chrome, зовут `toggleInbox` (НЕ
 * импорт `components/inbox`).
 */
import { InboxPanel } from '../../../components/inbox/InboxPanel';
import { useUIStore } from '../../../stores/ui';
import { useVaultStore } from '../../../stores/vault';
import type { NexusModule } from '../types';

/** Модуль «Входящие» (INBOX-1). */
export const inboxModule: NexusModule = {
  id: 'inbox',
  activate(ctx) {
    // Оверлей: order=50 (прежний DOM-порядок App.tsx) — перенос из core-overlays КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'inbox',
      titleKey: 'commands.view.inbox',
      order: 50,
      isOpen: (s) => s.inboxOpen,
      component: InboxPanel,
    });

    // Команда палитры (прежняя commands-core `view.inbox`): id → `inbox:view.inbox`, source=plugin.
    // Vault-guard сохранён КАК ЕСТЬ.
    ctx.commands.register({
      id: 'view.inbox',
      title: 'Inbox',
      titleKey: 'commands.view.inbox',
      run: () => {
        if (!useVaultStore.getState().info) return;
        useUIStore.getState().toggleInbox();
      },
    });
  },
};
