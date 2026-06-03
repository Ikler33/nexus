import { lazy, Suspense, useEffect } from 'react';
import { isTauri } from './lib/tauri-api';
import { openVaultFlow, registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { Titlebar } from './components/chrome/Titlebar';
import { StatusBar } from './components/chrome/StatusBar';
import { Sidebar } from './components/sidebar/Sidebar';
import { EditorArea } from './components/workspace/EditorArea';
import { AiPanel } from './components/chat/AiPanel';
import { CommandPalette } from './components/command/CommandPalette';
import styles from './App.module.css';

// Граф и панели грузятся лениво (граф — тяжёлый sigma.js §10; плагины — iframe-демо).
const GraphView = lazy(() => import('./components/graph/GraphView'));
const PluginsPanel = lazy(() =>
  import('./components/plugins/PluginsPanel').then((m) => ({ default: m.PluginsPanel })),
);
const SyncPanel = lazy(() =>
  import('./components/sync/SyncPanel').then((m) => ({ default: m.SyncPanel })),
);

/**
 * Оболочка приложения (дизайн-система Hermes): titlebar (бренд / поиск / инструменты) + тело
 * (sidebar | редактор | AI-панель) + status bar. Вне Tauri открывается мок-vault. Хоткеи —
 * через keymap. i18n RU/EN.
 */
export function App() {
  const info = useVaultStore((s) => s.info);
  const graphOpen = useUIStore((s) => s.graphOpen);
  const chatOpen = useUIStore((s) => s.chatOpen);
  const pluginsOpen = useUIStore((s) => s.pluginsOpen);
  const syncOpen = useUIStore((s) => s.syncOpen);

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
    <div className={styles.app}>
      <Titlebar />
      <div className={`${styles.appBody} ${chatOpen ? styles.withChat : ''}`}>
        <aside className={styles.sidebar}>
          <Sidebar />
        </aside>
        <main className={styles.main}>
          <EditorArea />
        </main>
        {chatOpen && <AiPanel />}
      </div>
      <StatusBar />

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
