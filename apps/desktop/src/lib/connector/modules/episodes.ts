/**
 * «Эпизоды» (EP-3, саммари прошлых сессий) как оверлей-модуль через overlays-реестр (F-8c, F-10b).
 * Ядро больше НЕ импортирует `components/episodes`.
 *
 * Вклад: оверлей + подписка на `vault:opened` (episodic-sync, F-8b). Вход в панель — кнопка «Эпизоды…»
 * в секции настроек AI/Модели (`SettingsView`), которая зовёт `openEpisodes()` ui-стора (ядро-chrome, НЕ
 * импорт `components/episodes`). ПАТТЕРН оверлей-модуля: стейт `episodesOpen` +
 * `openEpisodes/closeEpisodes/toggleEpisodes` остаются ядром (ui-стор); модуль даёт КОМПОНЕНТ +
 * `isOpen`-селектор. Тоггл «Эпизодическая память» в ai-секции — тоже ядро (pref `aiEpisodicMemory`).
 *
 * F-8b (хвост коннектора): episodic-sync перенесён из App.tsx сюда. При открытии vault фронт-pref
 * `aiEpisodicMemory` (= отображение тоггла + per-call флаг чата) синхронизируется от БД vault (ИСТОЧНИК
 * ИСТИНЫ) — иначе на другой машине / после очистки localStorage тоггл показывал бы OFF, а фоновая
 * генерация шла (нарушение privacy-default). `vault:opened` = window-событие `vault:switched` от
 * `stores/vault.openVault` (диспатчится УЖЕ ПОСЛЕ переключения бэка) — фетч НЕ зависит от фронт-root, а
 * pref фронтовый; тайминг чуть раньше прежнего App-эффекта `[vaultRoot]` (событие до `set({info})`), но
 * это behavior-preserving. Best-effort: ошибка → pref как есть.
 */
import { EpisodesPanel } from '../../../components/episodes/EpisodesPanel';
import { usePrefsStore } from '../../../stores/prefs';
import type { NexusModule } from '../types';

/** Модуль «Эпизоды» (EP-3): оверлей + episodic-sync по `vault:opened` (F-8b). */
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

    // episodic-sync (F-8b): при открытии vault тянем persisted `episodic.enabled` из БД vault и отражаем
    // во фронт-pref `aiEpisodicMemory`. Перенос эффекта App.tsx `[vaultRoot]` на `ctx.events` (событие
    // зовётся из openVault ПОСЛЕ переключения бэка; фетч не нуждается в фронт-root). Best-effort.
    ctx.events.on('vault:opened', () => {
      void ctx.api.episode
        .getEnabled()
        .then((on) => usePrefsStore.getState().setAiEpisodicMemory(on))
        .catch(() => {});
    });
  },
};
