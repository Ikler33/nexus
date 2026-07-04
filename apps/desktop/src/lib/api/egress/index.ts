import * as mockEgress from '../../mock/egress';
import { bridge } from '../bridge';
import type { EgressFeatureId, EgressState } from './types';

/**
 * Egress-домен (F-2d): политика эгресса ядра (срез 2 net.md) — тоггл «офлайн» (E2) + per-feature
 * opt-in (E6). Изменения применяются мгновенно и переживают рестарт (E5, OS config-dir). Все вызовы —
 * через `bridge` (Tauri ↔ мок `lib/mock/egress`); потребители ходят сюда по-прежнему через
 * `tauriApi.egress` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const egress = {
  getState: (): Promise<EgressState> =>
    bridge<EgressState>('get_egress_state', undefined, () => mockEgress.getState()),

  /** Включение дорезает активный chat-стрим (E10); LAN/loopback-модели продолжают работать. */
  setOffline: (offline: boolean): Promise<EgressState> =>
    bridge<EgressState>('set_egress_offline', { offline }, () => mockEgress.setOffline(offline)),

  setFeature: (feature: EgressFeatureId, enabled: boolean): Promise<EgressState> =>
    bridge<EgressState>('set_egress_feature', { feature, enabled }, () =>
      mockEgress.setFeature(feature, enabled),
    ),
};
