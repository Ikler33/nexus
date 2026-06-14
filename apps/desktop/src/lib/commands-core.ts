import { commands, type Disposable } from './commands';
import { openOrCreateDaily } from './daily';
import { getActiveEditorView } from './editor/activeView';
import { insertLink, toggleTask, toggleWrap } from './editor/format';
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

/** EDIT-1: тоггл markdown-обрамления выделения в активном редакторе. Нет редактора — no-op. */
function formatActiveEditor(marker: string): void {
  const view = getActiveEditorView();
  if (view) toggleWrap(view, marker);
}

/** EDIT-2: тоггл таска на строке(ах) активного редактора. Нет редактора — no-op. */
function toggleTaskInActiveEditor(): void {
  const view = getActiveEditorView();
  if (view) toggleTask(view);
}

/** EDIT-4: вставка markdown-ссылки на выделении активного редактора. Нет редактора — no-op. */
function insertLinkInActiveEditor(): void {
  const view = getActiveEditorView();
  if (view) insertLink(view);
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
      // NAV-2: quick-switcher — палитра с секцией «Недавние» на пустом запросе (мускульная память ⌘O).
      id: 'recents.open',
      title: 'Go to recent…',
      titleKey: 'commands.recents.open',
      source: 'core',
      defaultKey: 'mod+o',
      run: () => useUIStore.getState().openPalette(),
    }),
    commands.register({
      // NAV-3: назад по истории навигации (браузерная модель). ⌘[ освобождён от indentLess в редакторе.
      id: 'nav.back',
      title: 'Back',
      titleKey: 'commands.nav.back',
      source: 'core',
      defaultKey: 'mod+[',
      run: () => useWorkspaceStore.getState().navBack(),
    }),
    commands.register({
      id: 'nav.forward',
      title: 'Forward',
      titleKey: 'commands.nav.forward',
      source: 'core',
      defaultKey: 'mod+]',
      run: () => useWorkspaceStore.getState().navForward(),
    }),
    commands.register({
      id: 'vault.open',
      title: 'Open vault…',
      titleKey: 'commands.vault.open',
      source: 'core',
      defaultKey: 'mod+shift+o', // NAV-2: уступил ⌘O quick-switcher'у (открытие vault — редкое)
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
      id: 'note.daily',
      title: 'Daily note',
      titleKey: 'commands.note.daily',
      source: 'core',
      defaultKey: 'mod+shift+d',
      run: async () => {
        if (!useVaultStore.getState().info) return; // нет открытого vault — некуда писать
        await openOrCreateDaily();
      },
    }),
    commands.register({
      id: 'capture.quick',
      title: 'Quick capture',
      titleKey: 'commands.capture.quick',
      source: 'core',
      defaultKey: 'mod+shift+n',
      run: () => {
        if (!useVaultStore.getState().info) return;
        useUIStore.getState().openCapture();
      },
    }),
    commands.register({
      id: 'note.fromTemplate',
      title: 'New note from template',
      titleKey: 'commands.note.fromTemplate',
      source: 'core',
      defaultKey: 'mod+shift+t',
      run: () => {
        if (!useVaultStore.getState().info) return; // нет открытого vault — некуда создавать
        useUIStore.getState().openTemplates();
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
      // EDIT-1: жирный ⌘B (тоггл **…**). CM6 не биндит Mod-b → ловит глобальный useKeymap.
      id: 'editor.format.bold',
      title: 'Bold',
      titleKey: 'commands.format.bold',
      source: 'core',
      defaultKey: 'mod+b',
      run: () => formatActiveEditor('**'),
    }),
    commands.register({
      // EDIT-1: курсив ⌘⇧I (тоггл *…*). ⌘I занят inline-LLM (IL-2), поэтому ⌘⇧I.
      id: 'editor.format.italic',
      title: 'Italic',
      titleKey: 'commands.format.italic',
      source: 'core',
      defaultKey: 'mod+shift+i',
      run: () => formatActiveEditor('*'),
    }),
    commands.register({
      // EDIT-4: вставка ссылки ⌘K (универсальный «вставить ссылку»; Mod-k свободен в реестре и CM6).
      id: 'editor.format.link',
      title: 'Insert link',
      titleKey: 'commands.format.link',
      source: 'core',
      defaultKey: 'mod+k',
      run: () => insertLinkInActiveEditor(),
    }),
    commands.register({
      // EDIT-2: чекбокс/таск ⌘L (тоггл - [ ]↔- [x]; обычная строка → таск). Mod-l свободен.
      id: 'editor.task.toggle',
      title: 'Toggle task / checkbox',
      titleKey: 'commands.format.task',
      source: 'core',
      defaultKey: 'mod+l',
      run: () => toggleTaskInActiveEditor(),
    }),
    commands.register({
      id: 'editor.toggleMode',
      title: 'Edit / Preview',
      titleKey: 'commands.editor.toggleMode',
      source: 'core',
      defaultKey: 'mod+e',
      run: () => useWorkspaceStore.getState().toggleMode(),
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
      id: 'versions.open',
      title: 'Version history',
      titleKey: 'commands.versions.open',
      source: 'core',
      run: () => useUIStore.getState().openVersions(),
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
      // TASK-1: панель «Задачи» (сводка всех `- [ ]` vault). Mod-Shift-K свободен.
      id: 'view.tasks',
      title: 'Tasks',
      titleKey: 'commands.view.tasks',
      source: 'core',
      defaultKey: 'mod+shift+k',
      run: () => {
        if (!useVaultStore.getState().info) return; // нет vault — нечего сканировать
        useUIStore.getState().toggleTasks();
      },
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
      id: 'view.news',
      title: 'News feed',
      titleKey: 'commands.view.news',
      source: 'core',
      run: () => useUIStore.getState().toggleNews(),
    }),
    commands.register({
      id: 'view.home',
      title: 'Home',
      titleKey: 'commands.view.home',
      source: 'core',
      run: () => useUIStore.getState().toggleHome(),
    }),
    commands.register({
      id: 'vault.rescan',
      title: 'Reindex vault',
      titleKey: 'commands.vault.rescan',
      source: 'core',
      run: () => {
        if (!useVaultStore.getState().info) return; // нет vault — нечего индексировать
        return tauriApi.vault.rescan();
      },
    }),
    commands.register({
      id: 'theme.toggle',
      title: 'Toggle theme (light/dark)',
      titleKey: 'commands.theme.toggle',
      source: 'core',
      run: () => useThemeStore.getState().toggle(),
    }),
    commands.register({
      id: 'view.toggleSidebar',
      title: 'Toggle sidebar',
      titleKey: 'commands.view.toggleSidebar',
      source: 'core',
      run: () => useUIStore.getState().toggleSidebar(),
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
