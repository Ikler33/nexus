import { lazy, Suspense, useEffect } from 'react';
import {
  FolderOpen,
  GitBranch,
  Languages,
  MessageSquare,
  Moon,
  Puzzle,
  Share2,
  Sun,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { isTauri } from './lib/tauri-api';
import { openVaultFlow, registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { changeLocale } from './i18n/setup';
import { useThemeStore } from './stores/theme';
import { useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { Sidebar } from './components/sidebar/Sidebar';
import { EditorArea } from './components/workspace/EditorArea';
import { AiPanel } from './components/chat/AiPanel';
import { CommandPalette } from './components/command/CommandPalette';
import styles from './App.module.css';

// Граф и панель плагинов грузятся лениво (граф — тяжёлый sigma.js §10; плагины — iframe-демо).
const GraphView = lazy(() => import('./components/graph/GraphView'));
const PluginsPanel = lazy(() =>
  import('./components/plugins/PluginsPanel').then((m) => ({ default: m.PluginsPanel })),
);
const SyncPanel = lazy(() =>
  import('./components/sync/SyncPanel').then((m) => ({ default: m.SyncPanel })),
);

/**
 * Оболочка приложения: sidebar (поиск + дерево) + область редактора со вкладками/сплитами
 * (Б12) + Command Palette. Вне Tauri открывается мок-vault. Хоткеи — через keymap. i18n RU/EN.
 */
export function App() {
  const info = useVaultStore((s) => s.info);
  const graphOpen = useUIStore((s) => s.graphOpen);
  const toggleGraph = useUIStore((s) => s.toggleGraph);
  const chatOpen = useUIStore((s) => s.chatOpen);
  const toggleChat = useUIStore((s) => s.toggleChat);
  const pluginsOpen = useUIStore((s) => s.pluginsOpen);
  const togglePlugins = useUIStore((s) => s.togglePlugins);
  const syncOpen = useUIStore((s) => s.syncOpen);
  const toggleSync = useUIStore((s) => s.toggleSync);
  const theme = useThemeStore((s) => s.theme);
  const toggleTheme = useThemeStore((s) => s.toggle);
  const { t, i18n } = useTranslation();

  useKeymap();

  useEffect(() => {
    const disposable = registerCoreCommands();
    return () => disposable.dispose();
  }, []);

  useEffect(() => {
    if (!isTauri() && !info) {
      void openVaultFlow();
    }
  }, [info]);

  return (
    <div className={`${styles.layout} ${chatOpen ? styles.withChat : ''}`}>
      <aside className={styles.sidebar}>
        <header className={styles.sidebarHeader}>
          <span className={styles.vaultName} title={info?.root}>
            {info?.name ?? t('app.name')}
          </span>
          <div className={styles.headerActions}>
            <button
              className={styles.openBtn}
              onClick={() => toggleChat()}
              title={t('commands.view.chat')}
              aria-label={t('commands.view.chat')}
              aria-pressed={chatOpen}
            >
              <MessageSquare size={16} aria-hidden />
            </button>
            <button
              className={styles.openBtn}
              onClick={() => toggleGraph()}
              title={t('commands.view.graph')}
              aria-label={t('commands.view.graph')}
            >
              <Share2 size={16} aria-hidden />
            </button>
            <button
              className={styles.openBtn}
              onClick={() => togglePlugins()}
              title={t('commands.view.plugins')}
              aria-label={t('commands.view.plugins')}
              aria-pressed={pluginsOpen}
            >
              <Puzzle size={16} aria-hidden />
            </button>
            <button
              className={styles.openBtn}
              onClick={() => toggleSync()}
              title={t('commands.view.sync')}
              aria-label={t('commands.view.sync')}
              aria-pressed={syncOpen}
            >
              <GitBranch size={16} aria-hidden />
            </button>
            <button
              className={styles.openBtn}
              onClick={() => toggleTheme()}
              title={t('commands.theme.toggle')}
              aria-label={t('commands.theme.toggle')}
            >
              {theme === 'dark' ? <Sun size={16} aria-hidden /> : <Moon size={16} aria-hidden />}
            </button>
            <button
              className={styles.openBtn}
              onClick={() => changeLocale(i18n.language === 'ru' ? 'en' : 'ru')}
              title="Язык / Language"
              aria-label="Язык / Language"
            >
              <Languages size={16} aria-hidden />
            </button>
            <button
              className={styles.openBtn}
              onClick={() => void openVaultFlow()}
              title={t('app.openVault')}
              aria-label={t('app.openVault')}
            >
              <FolderOpen size={16} aria-hidden />
            </button>
          </div>
        </header>
        <Sidebar />
      </aside>

      <main className={styles.main}>
        <EditorArea />
      </main>

      {chatOpen && <AiPanel />}

      <CommandPalette />
      {graphOpen && (
        <Suspense fallback={null}>
          <GraphView />
        </Suspense>
      )}
      {pluginsOpen && (
        <Suspense fallback={null}>
          <PluginsPanel />
        </Suspense>
      )}
      {syncOpen && (
        <Suspense fallback={null}>
          <SyncPanel />
        </Suspense>
      )}
    </div>
  );
}
