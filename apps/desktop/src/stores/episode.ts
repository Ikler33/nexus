import { create } from 'zustand';

import { tauriApi, type EpisodeRow } from '../lib/tauri-api';

interface EpisodeState {
  /** Все эпизоды (обратная хронология — порядок с бэкенда), включая скрытые. */
  episodes: EpisodeRow[];
  loading: boolean;
  /** Текущее состояние тоггла эпизодической памяти (persisted на бэке). */
  enabled: boolean;
  /** Загрузить список. Ошибка → пустой (без throw — панель не падает). Монотонный токен против гонок. */
  load: () => Promise<void>;
  /** Подтянуть persisted-состояние тоггла. */
  loadEnabled: () => Promise<void>;
  /** Переключить эпизодическую память (ВКЛ enqueue'ит kick-генерацию на бэке). */
  setEnabled: (on: boolean) => Promise<void>;
  /** Скрыть эпизод (обратимо). Перечитывает. */
  dismiss: (id: number) => Promise<void>;
  /** Восстановить скрытый. Перечитывает. */
  restore: (id: number) => Promise<void>;
  /** Удалить навсегда. Перечитывает. */
  purge: (id: number) => Promise<void>;
}

/** Стор панели «Эпизоды» (EP-3): CRUD поверх tauri-команд `episode_*`. Мутаторы перечитывают список
 *  (память мала). `load()` защищён монотонным токеном — применяем только САМЫЙ свежий ответ. */
export const useEpisodeStore = create<EpisodeState>((set, get) => {
  let loadSeq = 0;
  return {
    episodes: [],
    loading: false,
    enabled: false,
    async load() {
      const seq = ++loadSeq;
      set({ loading: true });
      try {
        const episodes = await tauriApi.episode.list();
        if (seq === loadSeq) set({ episodes });
      } catch {
        if (seq === loadSeq) set({ episodes: [] });
      } finally {
        if (seq === loadSeq) set({ loading: false });
      }
    },
    async loadEnabled() {
      try {
        set({ enabled: await tauriApi.episode.getEnabled() });
      } catch {
        /* недоступно — оставляем текущее */
      }
    },
    async setEnabled(on) {
      await tauriApi.episode.setEnabled(on);
      set({ enabled: on });
    },
    async dismiss(id) {
      await tauriApi.episode.dismiss(id);
      await get().load();
    },
    async restore(id) {
      await tauriApi.episode.restore(id);
      await get().load();
    },
    async purge(id) {
      await tauriApi.episode.purge(id);
      await get().load();
    },
  };
});
