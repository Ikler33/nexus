import { promoteToBoard } from './board-promote';
import { commands, type Disposable } from './commands';
import { openOrCreateDaily } from './daily';
import { getActiveEditorView } from './editor/activeView';
import { insertLink, toggleTask, toggleWrap } from './editor/format';
import { printActiveNote } from './print';
import { isTauri, tauriApi, type InlineMode } from './tauri-api';
import i18n from '../i18n/setup';
import { useInlineStore } from '../stores/inline';
import { useThemeStore } from '../stores/theme';
import { useToastStore } from '../stores/toast';
import { useUIStore } from '../stores/ui';
import { useVaultStore } from '../stores/vault';
import { activeBuffer, activePath, useWorkspaceStore } from '../stores/workspace';

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

/** MEM-3 (D1): ЯВНО сохранить выделенный текст активного редактора в память агента (`source='explicit'`).
 *  Нет редактора / пустое выделение → честный toast-хинт. Toast об успехе/сбое (i18n-синглтон — команда
 *  вне React-дерева). */
async function saveSelectionToMemory(): Promise<void> {
  const view = getActiveEditorView();
  const sel = view?.state.selection.main;
  const text = view && sel ? view.state.sliceDoc(sel.from, sel.to).trim() : '';
  if (!text) {
    useToastStore.getState().addToast(i18n.t('commands.memory.noSelection'), { kind: 'error' });
    return;
  }
  try {
    await tauriApi.memory.add(text, 'explicit');
    useToastStore.getState().addToast(i18n.t('chat.memorySaved'), { kind: 'success' });
  } catch {
    useToastStore.getState().addToast(i18n.t('chat.memorySaveFailed'), { kind: 'error' });
  }
}

/** AI-1 (A1): продвинуть заметку `path` на доску (`board-promote`) + тост исхода + открыть доску. Сбой
 *  записи (флаш грязного буфера/диск) → тост-ошибка. Экспортируется для контекст-меню дерева (FileTree) —
 *  единый orchestration с командой палитры. Локализация колонки: `board.col.<id>` с фолбэком на raw-статус. */
export async function promoteNoteToBoard(path: string): Promise<void> {
  const toast = useToastStore.getState().addToast;
  try {
    const r = await promoteToBoard(path);
    const column = i18n.t(`board.col.${r.column}`, { defaultValue: r.column });
    if (r.kind === 'already') {
      toast(i18n.t('board.promote.already', { column }), { kind: 'info' });
    } else if (!r.inScope) {
      // Статус проставлен, но scope доски сужен и не совпал → карточка не на доске (честно, ревью AI-1).
      toast(i18n.t('board.promote.outOfScope', { column }), { kind: 'info' });
    } else {
      toast(i18n.t('board.promote.done', { column }), { kind: 'success' });
    }
    useUIStore.getState().openBoard();
  } catch {
    toast(i18n.t('board.promote.failed'), { kind: 'error' });
  }
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
      id: 'file.reveal',
      title: 'Reveal active file in tree',
      titleKey: 'commands.file.reveal',
      source: 'core',
      // REVEAL-ACTIVE-FILE: раскрываем предков активного файла и просим дерево проскроллить к нему
      // (открыв заметку через ⌘O/палитру/ссылку/граф, в дереве она раньше не подсвечивалась).
      run: async () => {
        const path = activePath(useWorkspaceStore.getState());
        if (!path) return; // нет активной заметки
        await useVaultStore.getState().revealPath(path);
        useUIStore.getState().requestReveal(path);
      },
    }),
    commands.register({
      id: 'file.rename',
      title: 'Rename active file',
      titleKey: 'commands.file.rename',
      source: 'core',
      defaultKey: 'f2',
      // FILE-RENAME-COMMAND: раскрываем предков активного файла и запускаем инлайн-переименование в
      // дереве. Сам rename (commitRename → vault.renameFile) флашит грязные буферы — несохранённое не
      // теряется. Папки переименовываются через контекст-меню (у команды — только активный файл).
      run: async () => {
        const path = activePath(useWorkspaceStore.getState());
        if (!path) return; // нет активной заметки
        await useVaultStore.getState().revealPath(path);
        useUIStore.getState().requestRename(path);
      },
    }),
    commands.register({
      id: 'file.copyMarkdown',
      title: 'Copy note as Markdown',
      titleKey: 'commands.file.copyMarkdown',
      source: 'core',
      // COPY-AS-MARKDOWN: копирует ИСХОДНЫЙ markdown активной заметки в буфер обмена (полезно в режиме
      // чтения, где нет редактора для select-all; и одним действием на весь документ). Берём `doc` из
      // буфера (живой текст с несохранёнными правками), не читаем диск.
      run: async () => {
        const buf = activeBuffer(useWorkspaceStore.getState());
        const toast = useToastStore.getState().addToast;
        if (!buf) {
          toast(i18n.t('file.noActiveNote'), { kind: 'error' });
          return;
        }
        if (!navigator.clipboard) {
          toast(i18n.t('file.copyFailed'), { kind: 'error' });
          return;
        }
        try {
          await navigator.clipboard.writeText(buf.doc);
          toast(i18n.t('file.copied'), { kind: 'success' });
        } catch {
          toast(i18n.t('file.copyFailed'), { kind: 'error' });
        }
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
      // AI-1 (A1): «На доску» — активная заметка → задача канбана (первая колонка дефолт-доски).
      id: 'board.promote',
      title: 'Add note to board',
      titleKey: 'commands.board.promote',
      source: 'core',
      run: () => {
        if (!useVaultStore.getState().info) return; // нет vault — нечего продвигать
        const buffer = activeBuffer(useWorkspaceStore.getState());
        if (!buffer) {
          useToastStore.getState().addToast(i18n.t('board.promote.noNote'), { kind: 'error' });
          return;
        }
        return promoteNoteToBoard(buffer.path);
      },
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
      // MEM-3 (D1): явная команда «в память» — сохранить выделение редактора как факт памяти агента.
      id: 'memory.saveSelection',
      title: 'Save selection to AI memory',
      titleKey: 'commands.memory.saveSelection',
      source: 'core',
      run: () => void saveSelectionToMemory(),
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
      // INBOX-1: панель «Входящие» (GTD-разбор Inbox.md). Без хоткея — ActivityBar/палитра.
      id: 'view.inbox',
      title: 'Inbox',
      titleKey: 'commands.view.inbox',
      source: 'core',
      run: () => {
        if (!useVaultStore.getState().info) return;
        useUIStore.getState().toggleInbox();
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
      // MEM-4: панель «Память ИИ» — управление явными фактами памяти агента.
      id: 'view.memory',
      title: 'AI memory',
      titleKey: 'commands.view.memory',
      source: 'core',
      run: () => {
        if (!useVaultStore.getState().info) return;
        useUIStore.getState().toggleMemory();
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
      id: 'view.today',
      title: 'Today',
      titleKey: 'commands.view.today',
      source: 'core',
      run: () => useUIStore.getState().toggleToday(),
    }),
    commands.register({
      id: 'view.agent',
      title: 'Agent',
      titleKey: 'commands.view.agent',
      source: 'core',
      run: () => useUIStore.getState().toggleAgent(),
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
    commands.register({
      id: 'help.cheatsheet',
      title: 'Keyboard shortcuts',
      titleKey: 'commands.help.cheatsheet',
      source: 'core',
      // ⌘/ — конвенция «показать сочетания» (Slack/Linear/GitHub); `Mod-/` вырезан из keymap
      // редактора (как nav ⌘[/⌘]), чтобы глобально открывал шпаргалку, а не toggleComment CM6.
      defaultKey: 'mod+/',
      run: () => useUIStore.getState().toggleCheatsheet(),
    }),
  ];
  return { dispose: () => disposers.forEach((d) => d.dispose()) };
}
