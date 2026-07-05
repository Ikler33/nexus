/**
 * F-10b — «Эпизоды» (EP-3, саммари прошлых сессий) как оверлей-модуль через overlays-реестр (F-8c).
 * Ядро больше НЕ импортирует `components/episodes`.
 *
 * У «Эпизодов» — САМЫЙ узкий вклад: ТОЛЬКО оверлей (у фичи НЕТ команды палитры и НЕТ фича-эффекта
 * App.tsx). Вход в панель — кнопка «Эпизоды…» в секции настроек AI/Модели (`SettingsView`), которая
 * зовёт `openEpisodes()` ui-стора (ядро-chrome, НЕ импорт `components/episodes`). ПАТТЕРН оверлей-
 * модуля: стейт `episodesOpen` + `openEpisodes/closeEpisodes/toggleEpisodes` остаются ядром (ui-стор);
 * модуль даёт КОМПОНЕНТ + `isOpen`-селектор. Тоггл «Эпизодическая память» в ai-секции — тоже ядро.
 */
import { EpisodesPanel } from '../../../components/episodes/EpisodesPanel';
import type { NexusModule } from '../types';

/** Модуль «Эпизоды» (EP-3). Только оверлей (ни команды, ни события). */
export const episodesModule: NexusModule = {
  id: 'episodes',
  activate(ctx) {
    // Оверлей: order=30 (прежний DOM-порядок App.tsx). titleKey='episode.title' — перенос КАК ЕСТЬ.
    ctx.overlays.register({
      id: 'episodes',
      titleKey: 'episode.title',
      order: 30,
      isOpen: (s) => s.episodesOpen,
      component: EpisodesPanel,
    });
  },
};
