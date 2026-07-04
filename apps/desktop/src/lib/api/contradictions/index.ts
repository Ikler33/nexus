import * as mockVault from '../../mock/vault';
import { bridge } from '../bridge';
import type { Contradiction } from './types';

/**
 * Contradictions-домен (F-2d): поиск противоречий по vault (#vision, спека
 * `docs/specs/contradictions.md`) — список найденного, постановка поиска в очередь, тоггл фичи. Все
 * вызовы — через `bridge` (Tauri ↔ мок `lib/mock/vault`); потребители ходят сюда по-прежнему через
 * `tauriApi.contradictions` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const contradictions = {
  /** Найденные противоречия (или `[]`). #vision, спека `docs/specs/contradictions.md`. Вне Tauri — мок. */
  list: (): Promise<Contradiction[]> =>
    bridge<Contradiction[]>('get_contradictions', undefined, () => mockVault.getContradictions()),

  /**
   * Ставит поиск противоречий в очередь (воркер выполнит фоном). Требует chat + эмбеддинги; дедуп
   * активной джобы. Завершение — по событию `jobs:changed`. Вне Tauri — no-op.
   */
  generate: (): Promise<void> =>
    bridge<void>('generate_contradictions', undefined, () => mockVault.generateContradictions()),

  /** Состояние тоггла «Поиск противоречий» (persisted, дефолт OFF). Вне Tauri — мок. */
  getEnabled: (): Promise<boolean> =>
    bridge<boolean>('contradictions_get_enabled', undefined, () =>
      mockVault.contradictionsGetEnabled(),
    ),

  /** Переключить «Поиск противоречий»; при включении бэкенд ставит kick-джобу. Вне Tauri — мок. */
  setEnabled: (on: boolean): Promise<void> =>
    bridge<void>('contradictions_set_enabled', { on }, () =>
      mockVault.contradictionsSetEnabled(on),
    ),
};
