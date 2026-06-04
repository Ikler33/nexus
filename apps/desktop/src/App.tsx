import { lazy, Suspense, useEffect } from 'react';
import { registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { useChatStore } from './stores/chat';
import { useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { Titlebar } from './components/chrome/Titlebar';
import { StatusBar } from './components/chrome/StatusBar';
import { Sidebar } from './components/sidebar/Sidebar';
import { EditorArea } from './components/workspace/EditorArea';
import { AiPanel } from './components/chat/AiPanel';
import { CommandPalette } from './components/command/CommandPalette';
import { Onboarding } from './components/onboarding/Onboarding';
import { SettingsView } from './components/settings/SettingsView';
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
  const tweaksOpen = useUIStore((s) => s.tweaksOpen);
  const reading = useUIStore((s) => s.reading);

  useKeymap();

  useEffect(() => {
    const disposable = registerCoreCommands();
    return () => disposable.dispose();
  }, []);

  // История чата — на vault (#17): подгружаем сохранённую сессию при смене корня vault.
  const vaultRoot = info?.root ?? null;
  useEffect(() => {
    useChatStore.getState().hydrate(vaultRoot);
  }, [vaultRoot]);

  // Esc выходит из режима чтения (если поверх нет оверлея — у них свой Esc).
  useEffect(() => {
    if (!reading) return;
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      const s = useUIStore.getState();
      if (s.paletteOpen || s.graphOpen || s.pluginsOpen || s.syncOpen) return;
      s.closeReading();
    };
    window.addEventListener('keydown', onEsc);
    return () => window.removeEventListener('keydown', onEsc);
  }, [reading]);

  // Первый запуск (vault не открыт) — приветственный экран онбординга.
  if (!info) return <Onboarding />;

  return (
    <div className={styles.app}>
      <Titlebar />
      <div
        className={`${styles.appBody} ${
          reading ? styles.reading : chatOpen ? styles.withChat : ''
        }`}
      >
        {!reading && (
          <aside className={styles.sidebar}>
            <Sidebar />
          </aside>
        )}
        <main className={styles.main}>
          <EditorArea />
        </main>
        {chatOpen && !reading && <AiPanel />}
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
      {tweaksOpen && <SettingsView />}
    </div>
  );
}
