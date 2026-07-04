import * as mockVault from '../../mock/vault';
import { bridge } from '../bridge';
import type { BacklinkEntry, FullGraph, GraphData, MentionEntry } from './types';

/**
 * Graph-домен (F-2d): граф связей vault — беклинки (ADR-004), незалинкованные упоминания (UNLINK-1),
 * локальный N-hop граф и единый граф всего vault (AC-DOD-Ф3). Все вызовы — через `bridge`
 * (Tauri ↔ мок `lib/mock/vault`); потребители ходят сюда по-прежнему через `tauriApi.graph`
 * (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const graph = {
  /** Беклинки файла (источник истины — SQLite, ADR-004). */
  getBacklinks: (path: string): Promise<BacklinkEntry[]> =>
    bridge<BacklinkEntry[]>('get_backlinks', { path }, () => mockVault.getBacklinks(path)),

  /** UNLINK-1: незалинкованные упоминания заголовка файла (FTS-фраза по телу, без уже-линкующих). */
  unlinkedMentions: (path: string): Promise<MentionEntry[]> =>
    bridge<MentionEntry[]>('get_unlinked_mentions', { path }, () =>
      mockVault.getUnlinkedMentions(path),
    ),

  /** Локальный N-hop граф вокруг файла (ADR-004). */
  getLocalGraph: (center: string, hops: number): Promise<GraphData> =>
    bridge<GraphData>('get_local_graph', { center, hops }, () =>
      mockVault.getLocalGraph(center, hops),
    ),

  /** Единый граф всего vault — топ-`limit` файлов по связности (AC-DOD-Ф3). */
  getFullGraph: (limit: number): Promise<FullGraph> =>
    bridge<FullGraph>('get_full_graph', { limit }, () => mockVault.getFullGraph(limit)),
};
