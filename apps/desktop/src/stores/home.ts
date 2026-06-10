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

/** Сколько узлов берём в мини-граф карточки (визуальная виньетка, не полноценный граф). */
const MINI_GRAPH_NODES = 48;

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
  error: string | null;
  load: () => Promise<void>;
  /** Перечитать один виджет (по событию `home:widget-updated`). */
  reloadWidget: (key: string) => Promise<void>;
  refreshWidget: (key: string) => Promise<void>;
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
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  reloadWidget: async (key) => {
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
      set((s) => ({ error: String(e), generating: { ...s.generating, [key]: false } }));
    }
  },
}));

function isTauriEnv(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
