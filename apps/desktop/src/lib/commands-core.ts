import { commands, type Disposable } from './commands';
import { isTauri, tauriApi } from './tauri-api';
import { useUIStore } from '../stores/ui';
import { useVaultStore } from '../stores/vault';

/** Открытие vault: нативный диалог в Tauri, мок в браузере. */
export async function openVaultFlow(): Promise<void> {
  if (!isTauri()) {
    await useVaultStore.getState().openVault('');
    return;
  }
  const dir = await tauriApi.vault.pickDirectory();
  if (dir) await useVaultStore.getState().openVault(dir);
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
        const { activeFile, saveActiveFile } = useVaultStore.getState();
        if (activeFile) return saveActiveFile(activeFile.content);
      },
    }),
  ];
  return { dispose: () => disposers.forEach((d) => d.dispose()) };
}
