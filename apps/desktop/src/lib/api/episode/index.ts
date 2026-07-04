import * as mockEpisode from '../../mock/episode';
import { bridge } from '../bridge';
import type { EpisodeRow } from './types';

/**
 * Episode-домен (F-2d): эпизодическая память (EP-3) — панель эпизодов (список/обратимое скрытие/
 * необратимое удаление) + тоггл. Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/episode`);
 * потребители ходят сюда по-прежнему через `tauriApi.episode` (barrel-реэкспорт в `lib/tauri-api.ts`).
 * Вне Tauri — in-memory мок.
 */
export const episode = {
  /** Все эпизоды для панели (обратная хронология, со скрытыми). */
  list: (): Promise<EpisodeRow[]> =>
    bridge<EpisodeRow[]>('episode_list', undefined, () => mockEpisode.list()),
  /** Скрыть эпизод (обратимо — убирает из ретривала, строка/вектор живы). */
  dismiss: (id: number): Promise<void> =>
    bridge<void>('episode_dismiss', { id }, () => mockEpisode.dismiss(id)),
  /** Восстановить скрытый эпизод. */
  restore: (id: number): Promise<void> =>
    bridge<void>('episode_restore', { id }, () => mockEpisode.restore(id)),
  /** Удалить эпизод НАВСЕГДА (строка + вектор). Необратимо; первоисточник-сессия цел. */
  purge: (id: number): Promise<void> =>
    bridge<void>('episode_purge', { id }, () => mockEpisode.purge(id)),
  /** Текущее состояние тоггла эпизодической памяти (persisted). */
  getEnabled: (): Promise<boolean> =>
    bridge<boolean>('episode_get_enabled', undefined, () => mockEpisode.getEnabled()),
  /** Переключить эпизодическую память; ВКЛ enqueue'ит kick-генерацию (контракт MAJOR-2). */
  setEnabled: (on: boolean): Promise<void> =>
    bridge<void>('episode_set_enabled', { on }, () => mockEpisode.setEnabled(on)),
};
