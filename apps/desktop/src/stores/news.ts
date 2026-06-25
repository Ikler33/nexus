import { create } from 'zustand';
import {
  isTauri,
  tauriApi,
  type NewsConfig,
  type NewsItem,
  type NewsRun,
  type NewsSource,
} from '../lib/tauri-api';

/**
 * Состояние страницы «Новости» (NF-5, спека `docs/specs/news-feed.md`). Данные — целиком
 * бэкенд-команды NF-3 (страница/конфиг/реестр); фильтры тема/непрочитанные — серверные
 * (повторный `load`). Прогон асинхронный: `refresh()` ставит джобу, результат прилетает
 * по `jobs:changed` (refetch в NewsView). `config.enabled` = consent (AC-NF-7).
 */
interface NewsState {
  items: NewsItem[];
  topics: string[];
  run: NewsRun | null;
  config: NewsConfig | null;
  sources: NewsSource[];
  /** Активный чип темы (`null` — «Все»). */
  topic: string | null;
  unreadOnly: boolean;
  /** Первая загрузка страницы (до прихода данных). */
  loading: boolean;
  /** Джоба прогона в очереди/выполняется («Собираю…» на кнопке). */
  refreshing: boolean;
  error: string | null;
  /** Транзиентное уведомление («Создана заметка …»); самосбрасывается в NewsView. */
  notice: string | null;
  load: () => Promise<void>;
  /** Ручной прогон (AC-NF-6): дедуп на бэке; снимается по `jobs:changed`-refetch. */
  refresh: () => Promise<void>;
  markRead: (id: number, read: boolean) => Promise<void>;
  toNote: (id: number) => Promise<void>;
  /** Вкл/выкл ленты (consent): пишет news.json и мгновенно правит политику эгресса (NF-4). */
  setEnabled: (enabled: boolean) => Promise<void>;
  setTopic: (topic: string | null) => void;
  setUnreadOnly: (unreadOnly: boolean) => void;
  clearNotice: () => void;
}

// Epoch-счётчик загрузок (audit B13): быстрая смена темы/«непрочитанные» во время in-flight load
// могла применить устаревший ответ (темы A) уже после переключения на B → лента не совпадала с чипом.
let loadEpoch = 0;

export const useNewsStore = create<NewsState>((set, get) => ({
  items: [],
  topics: [],
  run: null,
  config: null,
  sources: [],
  topic: null,
  unreadOnly: false,
  loading: true,
  refreshing: false,
  error: null,
  notice: null,

  load: async () => {
    const epoch = ++loadEpoch;
    try {
      const { topic, unreadOnly } = get();
      const [config, sources, page] = await Promise.all([
        tauriApi.news.getConfig(),
        tauriApi.news.sources(),
        tauriApi.news.page({ topic: topic ?? undefined, unreadOnly }),
      ]);
      const stillRefreshing = await tauriApi.scheduler.jobActive('newsfeed');
      if (epoch !== loadEpoch) return; // фильтр сменился во время загрузки → этот ответ устарел
      set({
        config,
        sources,
        items: page.items,
        topics: page.topics,
        run: page.run,
        loading: false,
        refreshing: stillRefreshing,
        error: null,
      });
    } catch (e) {
      if (epoch !== loadEpoch) return; // устаревшая загрузка не показывает свою ошибку
      set({ loading: false, refreshing: false, error: String(e) });
    }
  },

  refresh: async () => {
    if (get().refreshing) return;
    set({ refreshing: true, error: null });
    try {
      await tauriApi.news.refresh();
      // Вне Tauri событий `jobs:changed` нет — мок «завершает прогон» отложенным refetch'ом.
      if (!isTauri()) {
        setTimeout(() => void get().load(), 1500);
        return;
      }
      // Вотчдог «Собираю…» (инцидент 2026-06-12: воркер планировщика умер → джоба стояла
      // pending вечно, спиннер крутился часами). Поллим очередь: завершилась → refetch;
      // pending дольше минуты без запуска → планировщик мёртв, честная ошибка 'stalled';
      // running дольше потолка → тоже 'stalled' (вотчдог тика на бэке оборвёт сам тик).
      const startedAt = Date.now();
      const RUNNING_CAP_MS = 20 * 60_000;
      const tick = async () => {
        if (!get().refreshing) return; // load() уже снял спиннер
        let jobs;
        try {
          jobs = await tauriApi.scheduler.activeJobs();
        } catch {
          setTimeout(() => void tick(), 5000);
          return;
        }
        const nf = jobs.find((j) => j.kind === 'newsfeed');
        if (!nf) {
          // Джобы нет → прогон завершился (или умер в dead — load покажет состояние/ошибки).
          void get().load();
          return;
        }
        const readyAgoMs = Date.now() - nf.runAt * 1000;
        if (nf.state === 'pending' && readyAgoMs > 60_000) {
          set({ refreshing: false, error: 'stalled' });
          return;
        }
        if (Date.now() - startedAt > RUNNING_CAP_MS) {
          set({ refreshing: false, error: 'stalled' });
          return;
        }
        setTimeout(() => void tick(), 5000);
      };
      setTimeout(() => void tick(), 5000);
    } catch (e) {
      set({ refreshing: false, error: String(e) });
    }
  },

  markRead: async (id, read) => {
    // Оптимистично: карточка тускнеет сразу, бэкенд догоняет (ошибка — откат через load).
    set((s) => ({ items: s.items.map((it) => (it.id === id ? { ...it, read } : it)) }));
    try {
      await tauriApi.news.markRead(id, read);
    } catch (e) {
      set({ error: String(e) });
      void get().load();
    }
  },

  toNote: async (id) => {
    try {
      const path = await tauriApi.news.toNote(id);
      set({ notice: path });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  setEnabled: async (enabled) => {
    const current = get().config ?? {
      enabled: false,
      sources: {},
      keywords: null,
      extraHosts: [],
      modelPref: null,
    };
    try {
      const config = await tauriApi.news.setConfig({ ...current, enabled });
      set({ config });
      if (enabled) {
        // Первый прогон сразу после consent — лента наполняется без ожидания суточного тика.
        await get().refresh();
        await get().load();
      }
    } catch (e) {
      set({ error: String(e) });
    }
  },

  setTopic: (topic) => {
    set({ topic });
    void get().load();
  },

  setUnreadOnly: (unreadOnly) => {
    set({ unreadOnly });
    void get().load();
  },

  clearNotice: () => set({ notice: null }),
}));
