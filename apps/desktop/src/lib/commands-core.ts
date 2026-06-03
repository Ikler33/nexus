import { commands, type Disposable } from './commands';
import { isTauri, tauriApi } from './tauri-api';
import { useUIStore } from '../stores/ui';
import { useVaultStore } from '../stores/vault';
import { activeBuffer, useWorkspaceStore } from '../stores/workspace';

/** Открытие vault: нативный диалог в Tauri, мок в браузере; сбрасывает рабочее пространство. */
export async function openVaultFlow(): Promise<void> {
  if (isTauri()) {
    const dir = await tauriApi.vault.pickDirectory();
    if (!dir) return;
    await useVaultStore.getState().openVault(dir);
  } else {
    await useVaultStore.getState().openVault('');
  }
  useWorkspaceStore.getState().reset();
}

/** Регистрирует команды ядра. Возвращает Disposable для снятия (тесты/HMR). */
export function registerCoreCommands(): Disposable {
  const disposers = [
    commands.register({
      id: 'palette.open',
      title: 'Command palette',
      titleKey: 'commands.palette.open',
      source: 'core',
      defaultKey: 'mod+p',
      run: () => useUIStore.getState().openPalette(),
    }),
    commands.register({
      id: 'vault.open',
      title: 'Open vault…',
      titleKey: 'commands.vault.open',
      source: 'core',
      defaultKey: 'mod+o',
      run: () => openVaultFlow(),
    }),
    commands.register({
      id: 'file.save',
      title: 'Save file',
      titleKey: 'commands.file.save',
      source: 'core',
      run: () => {
        const buffer = activeBuffer(useWorkspaceStore.getState());
        if (buffer) return useWorkspaceStore.getState().saveBuffer(buffer.path);
      },
    }),
    commands.register({
      id: 'view.splitRight',
      title: 'Split right',
      titleKey: 'commands.view.splitRight',
      source: 'core',
      defaultKey: 'mod+\\',
      run: () => useWorkspaceStore.getState().splitRight(),
    }),
    commands.register({
      id: 'view.graph',
      title: 'Local graph',
      titleKey: 'commands.view.graph',
      source: 'core',
      defaultKey: 'mod+g',
      run: () => useUIStore.getState().toggleGraph(),
    }),
    commands.register({
      id: 'view.chat',
      title: 'AI chat',
      titleKey: 'commands.view.chat',
      source: 'core',
      defaultKey: 'mod+j',
      run: () => {
        useUIStore.getState().setAiTab('chat');
        useUIStore.getState().openChat();
      },
    }),
    commands.register({
      id: 'view.suggest',
      title: 'Link suggestions',
      titleKey: 'commands.view.suggest',
      source: 'core',
      run: () => {
        useUIStore.getState().setAiTab('suggest');
        useUIStore.getState().openChat();
      },
    }),
    commands.register({
      id: 'view.plugins',
      title: 'Plugins',
      titleKey: 'commands.view.plugins',
      source: 'core',
      run: () => useUIStore.getState().togglePlugins(),
    }),
    commands.register({
      id: 'view.sync',
      title: 'Sync (git)',
      titleKey: 'commands.view.sync',
      source: 'core',
      run: () => useUIStore.getState().toggleSync(),
    }),
  ];
  return { dispose: () => disposers.forEach((d) => d.dispose()) };
}
