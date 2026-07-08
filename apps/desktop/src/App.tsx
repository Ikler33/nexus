import { lazy, Suspense, useEffect } from 'react';
import './lib/connector/core-views'; // F-8: регистрирует ядровые main-вью в реестр `views` до рендера
import './lib/connector/modules'; // F-9..F-11: регистрирует+активирует модули-вклады (news/board/оверлеи/sync/graph/agent) до рендера
import { registerCoreCommands } from './lib/commands-core';
import { useKeymap } from './hooks/useKeymap';
import { tauriApi, isTauri } from './lib/tauri-api';
import { useAiFeaturesStore } from './stores/aiFeatures';
import { flushAllDirty } from './stores/autosave';
import { useChatStore } from './stores/chat';
import { usePrefsStore } from './stores/prefs';
import { selectMainView, selectReadingEscBlocked, useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { useWorkspaceStore } from './stores/workspace';
import { ActivityBar } from './components/chrome/ActivityBar';
import { Titlebar } from './components/chrome/Titlebar';
import { StatusBar } from './components/chrome/StatusBar';
import { SelfCheck } from './components/chrome/SelfCheck';
import { Sidebar } from './components/sidebar/Sidebar';
import { MainViewOutlet } from './components/workspace/MainViewOutlet';
import { OverlayOutlet } from './components/workspace/OverlayOutlet';
import { AiPanel } from './components/chat/AiPanel';
import { CommandPalette } from './components/command/CommandPalette';
import { QuickCapture } from './components/command/QuickCapture';
import { TemplatePicker } from './components/command/TemplatePicker';
import { HotkeysCheatsheet } from './components/command/HotkeysCheatsheet';
import { ToastViewport } from './components/chrome/ToastViewport';
import { Onboarding } from './components/onboarding/Onboarding';
import { SettingsView } from './components/settings/SettingsView';
import { InlineAria } from './components/editor/InlineAria';
import styles from './App.module.css';

// Панели грузятся лениво (плагины — iframe-демо).
// (F-11: ленивая AgentView живёт в модуле `connector/modules/agent` — main-вью резолвит MainViewOutlet.)
// (F-10c: SyncPanel вырезан в модуль `connector/modules/sync` — приходит через OverlayOutlet.)
// (F-10d: Граф — тяжёлый d3-force/louvain §10 — вырезан в модуль `connector/modules/graph`; ленивый
//  GraphView под Suspense живёт в graph-зоне `GraphLayer`, приходит через appBody-инстанс OverlayOutlet.)
const PluginsPanel = lazy(() =>
  import('./components/plugins/PluginsPanel').then((m) => ({ default: m.PluginsPanel })),
);
// ConflictResolver — ЯДРО (safe-flow git-merge, standalone из пилюли статусбара по conflictOpen, DP-14;
// + внутри SyncPanel). F-10c вынес его из зоны sync в `components/common` (он genuinely core: тянет
// только hooks/lib/stores, НЕ SyncPanel) — так App.tsx не импортит НИЧЕГО из вырезанной sync-зоны, и
// граница F-1b держится ПОЛНЫМ eslint-enforcement (без оговорок MODULE_BOUNDARY_EXCEPTIONS).
const ConflictResolver = lazy(() =>
  import('./components/common/ConflictResolver').then((m) => ({ default: m.ConflictResolver })),
);
const VersionHistory = lazy(() =>
  import('./components/editor/VersionHistory').then((m) => ({ default: m.VersionHistory })),
);

/**
 * Оболочка приложения (дизайн-система Hermes): titlebar (бренд / поиск / инструменты) + тело
 * (sidebar | редактор | AI-панель) + status bar. Вне Tauri открывается мок-vault. Хоткеи —
 * через keymap. i18n RU/EN.
 */
export function App() {
  const info = useVaultStore((s) => s.info);
  // F-10d: Граф (`graphOpen`) резолвит реестр `overlays` через appBody-инстанс <OverlayOutlet mount="appBody"/>
  // внутри `.appBody` — App больше не подписан на `graphOpen` и не рендерит слой напрямую.
  const chatOpen = useUIStore((s) => s.chatOpen);
  const pluginsOpen = useUIStore((s) => s.pluginsOpen);
  // F-10c: SyncPanel (`syncOpen`) резолвит реестр `overlays` через <OverlayOutlet/> — App больше не
  // подписан на `syncOpen` и не рендерит панель напрямую.
  const conflictOpen = useUIStore((s) => s.conflictOpen);
  const closeConflict = useUIStore((s) => s.closeConflict);
  const versionsOpen = useUIStore((s) => s.versionsOpen);
  const closeVersions = useUIStore((s) => s.closeVersions);
  // F-8c: 7 оверлеев (goals/memory/episodes/tasks/inbox/digest/contradictions) резолвит реестр
  // `overlays` через <OverlayOutlet/> (App больше не подписан на их `*Open`-були по отдельности).
  // F-4 (семейство 1): единый derived-селектор активной main-вью вместо 5 отдельных `*Open`-булей.
  const mainView = useUIStore(selectMainView);
  const onboardingActive = useUIStore((s) => s.onboardingActive);
  const tweaksOpen = useUIStore((s) => s.tweaksOpen);
  const sidebarOpen = useUIStore((s) => s.sidebarOpen);
  const reading = useUIStore((s) => s.reading);
  const closeChat = useUIStore((s) => s.closeChat);
  const aiLayout = usePrefsStore((s) => s.aiLayout);
  const aiPanelW = usePrefsStore((s) => s.aiPanelW);
  const aiPanelH = usePrefsStore((s) => s.aiPanelH);

  useKeymap();

  useEffect(() => {
    const disposable = registerCoreCommands();
    return () => disposable.dispose();
  }, []);

  // История чата — на vault (#17): подгружаем сохранённую сессию при смене корня vault.
  const vaultRoot = info?.root ?? null;
  useEffect(() => {
    useChatStore.getState().hydrate(vaultRoot);
    // EP-3 (ревью): `episodic.enabled` живёт в БД vault (едет с vault), это ИСТОЧНИК ИСТИНЫ. Синхронизируем
    // фронт-pref `aiEpisodicMemory` (= отображение тоггла + per-call флаг чата) от бэка при открытии vault.
    // Иначе на другой машине / после очистки localStorage тоггл показывал бы OFF, а фоновая генерация шла
    // (нарушение privacy-default). Best-effort: нет vault/ошибка → оставляем pref как есть.
    if (vaultRoot) {
      void tauriApi.episode
        .getEnabled()
        .then((on) => usePrefsStore.getState().setAiEpisodicMemory(on))
        .catch(() => {});
      // Тогглы «Инсайты»/«Поиск противоречий» — тоже persisted в БД vault (источник истины), грузим
      // от бэка при открытии (privacy-default, как эпизоды). Стор не лезет в localStorage.
      void useAiFeaturesStore.getState().sync();
    }
  }, [vaultRoot]);

  // F-10b: живой пересчёт «Целей» по `vault:changed` (ADR-007 S8, AC-GP-3) переехал в модуль
  // `connector/modules/goals` (через `ctx.events`) — фича-эффект живёт рядом со своим оверлеем.

  // SAFE-3: внешнее изменение конкретного файла (`vault:file-changed`) → судьба открытого буфера
  // решается в сторе (эхо своего сейва / тихий reload чистого / баннер guard'а грязного).
  useEffect(() => {
    let unlisten = () => {};
    void tauriApi.events
      .onFileChanged(({ path, hash }) => {
        void useWorkspaceStore.getState().onExternalFileChange(path, hash);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten();
  }, []);

  // SAFE-4: закрытие окна с несохранёнными правками → флашим ВСЁ перед закрытием (local-first, без
  // диалога). Ошибка записи → окно НЕ закрываем (правки целы, видны в статусбаре). Только Tauri.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten = () => {};
    let closing = false;
    void import('@tauri-apps/api/window').then(({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      void win
        .onCloseRequested(async (event) => {
          if (closing) return; // повторный close после успешного флаша — пропускаем
          const dirty = Object.values(useWorkspaceStore.getState().buffers).some((b) => b.dirty);
          if (!dirty) return; // нечего сохранять — обычное закрытие
          event.preventDefault();
          try {
            await flushAllDirty();
            closing = true;
            await win.close();
          } catch {
            /* запись упала → окно не закрываем, правки целы (мандат 4) */
          }
        })
        .then((fn) => {
          unlisten = fn;
        });
    });
    return () => unlisten();
  }, []);

  // F-10b: refetch открытых панелей по `jobs:changed` (ADR-007 slice 4/5) переехал в оверлей-модули
  // `connector/modules/{digest,contradictions}` (`ctx.events`) — каждая фича refetch'ит свой стор.

  // Esc выходит из режима чтения (если поверх нет оверлея — у них свой Esc).
  useEffect(() => {
    if (!reading) return;
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // P0-3-смоук (регресс reading-esc-precedence): оверлеи, закрывающие себя по Esc БЕЗ
      // stopPropagation (палитра/QuickCapture/TemplatePicker), обновляют стор СИНХРОННО — гейт
      // ниже видел уже-закрытое состояние, и один Esc гасил и оверлей, и режим чтения «сквозь»
      // него. Обработанный ближе к фокусу Esc не дублируем (паттерн useKeymap: defaultPrevented).
      if (e.defaultPrevented) return;
      const s = useUIStore.getState();
      // Любой оверлей поверх reading имеет приоритет на Esc (у него свой close) — иначе Esc закрыл бы
      // весь режим чтения «сквозь» открытую модалку (находки аудита reading-esc-precedence +
      // conflictresolver-esc). Модальные панели без собственного focus-trap (digest/contradictions/
      // settings/conflict) особенно зависят от этого гейта. F-4: набор кодифицирован в
      // selectReadingEscBlocked (union trap+floats+safe-flow) КАК ЕСТЬ — без изменения поведения.
      if (selectReadingEscBlocked(s)) return;
      s.closeReading();
    };
    window.addEventListener('keydown', onEsc);
    return () => window.removeEventListener('keydown', onEsc);
  }, [reading]);

  // Первый запуск (vault не открыт) — онбординг; активный многошаговый flow (DP-7)
  // держит экран и после открытия vault (шаги AI-проверки и индексации).
  if (!info || onboardingActive) return <Onboarding />;

  // DP-12 (макет): расположение AI-панели — side / bottom / overlay; панель живёт только
  // в workspace-вью (Home/News — без неё, как `view === "workspace"` макета).
  const aiVisible = chatOpen && !reading && mainView === 'editor';
  const aiSide = aiVisible && aiLayout === 'side';
  const aiBottom = aiVisible && aiLayout === 'bottom';
  const aiOverlay = aiVisible && aiLayout === 'overlay';

  return (
    <div className={styles.app}>
      <Titlebar />
      {/* DP-13 (макет app-shell): вертикальный activity-bar + тело. */}
      <div className={styles.appShell}>
        {!reading && <ActivityBar />}
        <div
          className={`${styles.appBody} ${
            reading ? styles.reading : aiSide ? styles.withChat : aiBottom ? styles.withChatBottom : ''
          } ${!reading && !sidebarOpen ? styles.sidebarCollapsed : ''}`}
          style={
            {
              '--ai-panel-w': `${aiPanelW}px`,
              '--ai-panel-h': `${aiPanelH}px`,
            } as React.CSSProperties
          }
        >
          {!reading && sidebarOpen && (
            <aside className={styles.sidebar}>
              <Sidebar />
            </aside>
          )}
          <main className={styles.main}>
            {/* F-8: активная main-вью резолвится реестром `views` (App-lookup), рендер — через
                per-contribution ErrorBoundary (падение вью → плашка, app жив). */}
            <MainViewOutlet />
          </main>
          {aiSide && <AiPanel />}
          {aiBottom && (
            <div className={styles.aiBottom}>
              <AiPanel variant="bottom" />
            </div>
          )}
          {aiOverlay && (
            <div
              className={styles.aiScrim}
              onMouseDown={(e) => {
                if (e.target === e.currentTarget) closeChat();
              }}
            >
              <AiPanel variant="overlay" />
            </div>
          )}
          {/* F-10d: оверлеи mount:'appBody' (единственный — Граф) — из реестра `overlays` через
              appBody-инстанс OverlayOutlet ВНУТРИ `.appBody`. Слой графа `.graph-layer` (absolute
              inset:0) остаётся В ГРАНИЦАХ тела, не поверх титлбара/статусбара (фикс владельца «хром
              торчал поверх графа»). Заменяет прежний хардкод `{graphOpen && <div.graphLayer><GraphView/></div>}`;
              позиция/поведение идентичны + per-contribution ErrorBoundary. */}
          <OverlayOutlet mount="appBody" />
        </div>
      </div>
      <InlineAria />
      <StatusBar />
      {/* W-21: dev self-check (пинг LLM + конфиг) — только в dev-сборке, аид разработки. */}
      {import.meta.env.DEV && <SelfCheck />}

      <CommandPalette />
      <QuickCapture />
      <TemplatePicker />
      <HotkeysCheatsheet />
      <ToastViewport />
      {pluginsOpen && (
        <Suspense fallback={null}>
          <PluginsPanel />
        </Suspense>
      )}
      {/* F-10c: SyncPanel (`syncOpen`) переехал в модуль `connector/modules/sync` — рендерится из
          реестра `overlays` через <OverlayOutlet/> ниже (per-contribution ErrorBoundary). */}
      {/* DP-14: конфликт-резолвер из пилюли статусбара (мимо SyncPanel, как onConflict макета). */}
      {conflictOpen && (
        <Suspense fallback={null}>
          <ConflictResolver onClose={closeConflict} />
        </Suspense>
      )}
      {/* SAFE-6: история версий активной заметки (палитра / кнопка в табах / «Сравнить» из guard-баннера). */}
      {versionsOpen && (
        <Suspense fallback={null}>
          <VersionHistory onClose={closeVersions} />
        </Suspense>
      )}
      {tweaksOpen && <SettingsView />}
      {/* F-8c: 7 оверлеев (goals/memory/episodes/tasks/inbox/digest/contradictions) — из реестра
          `overlays` (App-lookup), каждый через per-contribution ErrorBoundary. Заменяет прежние 7
          хардкод-строк `{xOpen && <Panel/>}`; поведение идентично (те же панели/условия + изоляция). */}
      <OverlayOutlet />
    </div>
  );
}
