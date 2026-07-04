import * as mockVault from '../../mock/vault';
import { bridge } from '../bridge';
import type { Digest } from './types';

/**
 * Digest-домен (F-2d): «Дайджест изменений» vault (ADR-007 slice 4) — последний сгенерированный
 * дайджест и постановка генерации в очередь. Все вызовы — через `bridge` (Tauri ↔ мок
 * `lib/mock/vault`); потребители ходят сюда по-прежнему через `tauriApi.digest` (barrel-реэкспорт в
 * `lib/tauri-api.ts`).
 */
export const digest = {
  /** Последний сгенерированный «Дайджест изменений» (или `null`). ADR-007 slice 4. Вне Tauri — мок. */
  latest: (): Promise<Digest | null> =>
    bridge<Digest | null>('get_latest_digest', undefined, () => mockVault.getDigest()),

  /**
   * Ставит генерацию дайджеста в очередь (воркер выполнит на ближайшем тике). Требует
   * сконфигурированного chat (иначе backend вернёт ошибку). Завершение — по событию `jobs:changed`.
   */
  generate: (): Promise<void> =>
    bridge<void>('generate_digest', undefined, () => mockVault.generateDigest()),
};
