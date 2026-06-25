import { create } from 'zustand';
import {
  tauriApi,
  type FullGraph,
  type HomeActivity,
  type HomeData,
  type OpenQuestion,
  type StaleNote,
  type Widget,
} from '../lib/tauri-api';
import i18n from '../i18n/setup';
import { logUi } from '../lib/debug-log';
import { useToastStore } from './toast';

/** Сколько узлов берём в мини-граф карточки (визуальная виньетка, не полноценный граф). */
const MINI_GRAPH_NODES = 48;

/** Префикс kind LLM-виджетов в планировщике (зеркалит Rust `home::widgets::KIND_PREFIX`): по нему
 *  активную джобу `home_widget:open_questions` сопоставляем ключу виджета `open_questions` (AIP-5). */
const WIDGET_KIND_PREFIX = 'home_widget:';

/**
 * Состояние HOME-дашборда (DP-1, макет `home.jsx`): статика H1 (stats/recent/goals) +
 * активность H6 + LLM-виджеты H3/H5 (brief/questions/drift из кэша; refresh — фоновая джоба,
 * готовность по `home:widget-updated` — refetch в HomeView) + stale radar H4 + мини-граф.
 */
interface HomeState {
  data: HomeData | null;
  activity: HomeActivity | null;
  brief: Widget | null;
  questions: OpenQuestion[];
  drift: string | null;
  stale: StaleNote[];
  graph: FullGraph | null;
  loading: boolean;
  /** Ключи виджетов, по которым крутится фоновая генерация («thinking» на карточке). */
  generating: Record<string, boolean>;
  /** P0-4: ТОЛЬКО фатальный провал `load()` (весь дашборд не загрузился) → глобальный баннер. Провал
   *  пер-виджетного reload/refresh сюда НЕ пишется (показывается тостом) — иначе ошибка одного виджета
   *  вешала бы баннер поверх рабочего дашборда. Успешный `load()`/`reloadWidget` сбрасывает в null. */
  error: string | null;
  load: () => Promise<void>;
  /** Перечитать один виджет (по событию `home:widget-updated`). */
  reloadWidget: (key: string) => Promise<void>;
  refreshWidget: (key: string) => Promise<void>;
  /** AIP-5: подтянуть «генерируется» из активных джоб планировщика (`home_widget:*`) — чтобы карточка,
   *  которую планировщик сидит проактивно на открытии vault, показывала «генерирю…», а не «нажми
   *  обновить». Только ДОБАВЛЯЕТ флаги (снятие — по `home:widget-updated` → reloadWidget). */
  syncGenerating: () => Promise<void>;
}

export const useHomeStore = create<HomeState>((set, get) => ({
  data: null,
  activity: null,
  brief: null,
  questions: [],
  drift: null,
  stale: [],
  graph: null,
  loading: true,
  generating: {},
  error: null,

  load: async () => {
    try {
      const [data, activity, brief, questions, drift, stale, graph] = await Promise.all([
        tauriApi.home.data(),
        tauriApi.home.activity(),
        tauriApi.home.widget('daily_brief'),
        tauriApi.home.openQuestions(),
        tauriApi.home.contextDrift(),
        tauriApi.home.staleRadar(),
        tauriApi.graph.getFullGraph(MINI_GRAPH_NODES),
      ]);
      set({ data, activity, brief, questions, drift, stale, graph, loading: false, error: null });
      void get().syncGenerating(); // карточки, что планировщик сидит на открытии, → «генерирю…»
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  syncGenerating: async () => {
    try {
      const jobs = await tauriApi.scheduler.activeJobs();
      const nowSec = Date.now() / 1000;
      const active: Record<string, boolean> = {};
      for (const j of jobs) {
        // Будущие recurring-джобы (runAt в будущем) НЕ «генерируются сейчас»: после прогона хендлер
        // переармливает future-pending джобу, и без этого фильтра она переустанавливала бы флаг сразу
        // после снятия → спиннер залипал бы (класс бага #63 «ready vs future», adversarial-ревью;
        // лечит сразу и open_questions/context_drift, и stale). Считаем только running/готовые.
        if (j.state !== 'running' && j.runAt > nowSec) continue;
        if (j.kind.startsWith(WIDGET_KIND_PREFIX)) {
          active[j.kind.slice(WIDGET_KIND_PREFIX.length)] = true;
        } else if (j.kind === 'stale_radar') {
          // Stale radar — отдельный kind (не `home_widget:*`), но тот же индикатор «обогащаю…»
          // (AIP-хвост: слой 2 теперь сидится проактивно на открытии).
          active.stale_radar = true;
        }
      }
      if (Object.keys(active).length) {
        set((s) => ({ generating: { ...s.generating, ...active } }));
      }
    } catch {
      // вне Tauri / планировщик недоступен — no-op (генерирующихся джоб нет)
    }
  },

  reloadWidget: async (key) => {
    try {
      if (key === 'daily_brief') {
        const brief = await tauriApi.home.widget('daily_brief');
        set((s) => ({ brief, generating: { ...s.generating, daily_brief: false } }));
      } else if (key === 'open_questions') {
        const questions = await tauriApi.home.openQuestions();
        set((s) => ({ questions, generating: { ...s.generating, open_questions: false } }));
      } else if (key === 'context_drift') {
        const drift = await tauriApi.home.contextDrift();
        set((s) => ({ drift, generating: { ...s.generating, context_drift: false } }));
      } else if (key === 'stale_radar') {
        const stale = await tauriApi.home.staleRadar();
        set((s) => ({ stale, generating: { ...s.generating, stale_radar: false } }));
      }
      // P0-4: успешное обновление виджета снимает прежний фатальный баннер (дашборд снова в порядке).
      if (get().error !== null) set({ error: null });
    } catch (e) {
      // P0-4: пер-виджетный провал — НЕ глобальный баннер (иначе ошибка одного виджета перекрывает
      // рабочий дашборд). Гасим спиннер «генерирую…» (иначе висел бы вечно, обещая результат, которого
      // не будет — audit honesty/stuck-state) и показываем ЛОКАЛЬНЫЙ тост.
      set((s) => ({ generating: { ...s.generating, [key]: false } }));
      useToastStore.getState().addToast(i18n.t('home.widgetError'), { kind: 'error' });
      logUi('home:widget-error', `${key}: ${String(e).slice(0, 200)}`);
    }
  },

  refreshWidget: async (key) => {
    if (get().generating[key]) return;
    set((s) => ({ generating: { ...s.generating, [key]: true } }));
    try {
      if (key === 'stale_radar') await tauriApi.home.staleRefresh();
      else await tauriApi.home.refresh(key);
      // Вне Tauri события не прилетают — мок «завершает» refresh отложенным refetch'ом.
      if (!isTauriEnv()) setTimeout(() => void get().reloadWidget(key), 900);
    } catch (e) {
      // P0-4: провал запуска обновления виджета — локальный тост, НЕ глобальный баннер (как reloadWidget).
      set((s) => ({ generating: { ...s.generating, [key]: false } }));
      useToastStore.getState().addToast(i18n.t('home.widgetError'), { kind: 'error' });
      logUi('home:widget-error', `${key}: ${String(e).slice(0, 200)}`);
    }
  },
}));

function isTauriEnv(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
