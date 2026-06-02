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
      title: 'Палитра команд',
      source: 'core',
      defaultKey: 'mod+p',
      run: () => useUIStore.getState().openPalette(),
    }),
    commands.register({
      id: 'vault.open',
      title: 'Открыть vault…',
      source: 'core',
      defaultKey: 'mod+o',
      run: () => openVaultFlow(),
    }),
    commands.register({
      id: 'file.save',
      title: 'Сохранить файл',
      source: 'core',
      run: () => {
        const buffer = activeBuffer(useWorkspaceStore.getState());
        if (buffer) return useWorkspaceStore.getState().saveBuffer(buffer.path);
      },
    }),
    commands.register({
      id: 'view.splitRight',
      title: 'Разделить вправо',
      source: 'core',
      defaultKey: 'mod+\\',
      run: () => useWorkspaceStore.getState().splitRight(),
    }),
  ];
  return { dispose: () => disposers.forEach((d) => d.dispose()) };
}
