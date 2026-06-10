import { lazy, Suspense, useEffect } from 'react';
import { registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { tauriApi } from './lib/tauri-api';
import { useChatStore } from './stores/chat';
import { useContradictionsStore } from './stores/contradictions';
import { useDigestStore } from './stores/digest';
import { useGoalsStore } from './stores/goals';
import { useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { Titlebar } from './components/chrome/Titlebar';
import { StatusBar } from './components/chrome/StatusBar';
import { Sidebar } from './components/sidebar/Sidebar';
import { EditorArea } from './components/workspace/EditorArea';
import { HomeView } from './components/home/HomeView';
import { NewsView } from './components/news/NewsView';
import { AiPanel } from './components/chat/AiPanel';
import { CommandPalette } from './components/command/CommandPalette';
import { Onboarding } from './components/onboarding/Onboarding';
import { SettingsView } from './components/settings/SettingsView';
import { GoalsPanel } from './components/goals/GoalsPanel';
import { DigestPanel } from './components/digest/DigestPanel';
import { ContradictionsPanel } from './components/contradictions/ContradictionsPanel';
import { InlineAria } from './components/editor/InlineAria';
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
  const goalsOpen = useUIStore((s) => s.goalsOpen);
  const digestOpen = useUIStore((s) => s.digestOpen);
  const contradictionsOpen = useUIStore((s) => s.contradictionsOpen);
  const newsOpen = useUIStore((s) => s.newsOpen);
  const homeOpen = useUIStore((s) => s.homeOpen);
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

  // Живой пересчёт зависимых от индекса вьюх по событию индексатора (ADR-007 S8, AC-GP-3):
  // сейчас — «Цели» (#35), и только когда панель открыта. Дебаунс — событий может быть пачка.
  useEffect(() => {
    let unlisten = () => {};
    let timer: ReturnType<typeof setTimeout> | undefined;
    void tauriApi.events
      .onVaultChanged(() => {
        if (!useUIStore.getState().goalsOpen) return;
        clearTimeout(timer);
        timer = setTimeout(() => void useGoalsStore.getState().load(), 800);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      clearTimeout(timer);
      unlisten();
    };
  }, []);

  // Готовые результаты фоновых джоб прилетают по `jobs:changed` → refetch открытой панели (без поллинга,
  // ADR-007 slice 4/5). Дайджест и «Поиск противоречий» — обе LLM-фичи планировщика.
  useEffect(() => {
    let unlisten = () => {};
    void tauriApi.events
      .onJobsChanged(() => {
        const ui = useUIStore.getState();
        if (ui.digestOpen) void useDigestStore.getState().load();
        if (ui.contradictionsOpen) void useContradictionsStore.getState().load();
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten();
  }, []);

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
          {homeOpen ? <HomeView /> : newsOpen ? <NewsView /> : <EditorArea />}
        </main>
        {chatOpen && !reading && <AiPanel />}
      </div>
      <InlineAria />
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
      {goalsOpen && <GoalsPanel />}
      {digestOpen && <DigestPanel />}
      {contradictionsOpen && <ContradictionsPanel />}
    </div>
  );
}
