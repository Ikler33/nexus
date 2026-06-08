import { commands, type Disposable } from './commands';
import { getActiveEditorView } from './editor/activeView';
import { printActiveNote } from './print';
import { isTauri, tauriApi, type InlineMode } from './tauri-api';
import { useInlineStore } from '../stores/inline';
import { useThemeStore } from '../stores/theme';
import { useUIStore } from '../stores/ui';
import { useVaultStore } from '../stores/vault';
import { activeBuffer, useWorkspaceStore } from '../stores/workspace';

/** Запускает inline-генерацию в активном редакторе (IL-3, команда палитры). Нет редактора — no-op. */
function runInlineInActiveEditor(mode: InlineMode): void {
  const view = getActiveEditorView();
  if (!view) return;
  view.focus();
  useInlineStore.getState().runInline(view, mode);
}

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
      id: 'file.new',
      title: 'New note',
      titleKey: 'commands.file.new',
      source: 'core',
      defaultKey: 'mod+n',
      run: async () => {
        if (!useVaultStore.getState().info) return; // нет открытого vault — некуда писать
        const path = await useVaultStore.getState().createNote();
        await useWorkspaceStore.getState().openFile(path);
      },
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
      id: 'file.print',
      title: 'Print / Export PDF',
      titleKey: 'commands.file.print',
      source: 'core',
      run: () => printActiveNote(),
    }),
    commands.register({
      id: 'editor.inline.continue',
      title: 'Inline: continue',
      titleKey: 'commands.inline.continue',
      source: 'core',
      run: () => runInlineInActiveEditor('continue'),
    }),
    commands.register({
      id: 'editor.inline.rewrite',
      title: 'Inline: rewrite selection',
      titleKey: 'commands.inline.rewrite',
      source: 'core',
      run: () => runInlineInActiveEditor('rewrite'),
    }),
    commands.register({
      id: 'editor.inline.summarize',
      title: 'Inline: summarize selection',
      titleKey: 'commands.inline.summarize',
      source: 'core',
      run: () => runInlineInActiveEditor('summarize'),
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
    commands.register({
      id: 'view.goals',
      title: 'Goals',
      titleKey: 'commands.view.goals',
      source: 'core',
      run: () => useUIStore.getState().toggleGoals(),
    }),
    commands.register({
      id: 'view.digest',
      title: 'Changes digest',
      titleKey: 'commands.view.digest',
      source: 'core',
      run: () => useUIStore.getState().toggleDigest(),
    }),
    commands.register({
      id: 'view.contradictions',
      title: 'Contradiction finder',
      titleKey: 'commands.view.contradictions',
      source: 'core',
      run: () => useUIStore.getState().toggleContradictions(),
    }),
    commands.register({
      id: 'theme.toggle',
      title: 'Toggle theme (light/dark)',
      titleKey: 'commands.theme.toggle',
      source: 'core',
      run: () => useThemeStore.getState().toggle(),
    }),
    commands.register({
      id: 'view.reading',
      title: 'Reading mode',
      titleKey: 'commands.view.reading',
      source: 'core',
      defaultKey: 'mod+r',
      run: () => useUIStore.getState().toggleReading(),
    }),
    commands.register({
      id: 'view.settings',
      title: 'Settings',
      titleKey: 'commands.view.settings',
      source: 'core',
      defaultKey: 'mod+,',
      run: () => useUIStore.getState().openSettings(),
    }),
  ];
  return { dispose: () => disposers.forEach((d) => d.dispose()) };
}
