import * as mockVault from '../../mock/vault';
import { bridge } from '../bridge';
// `NoteRef` (vault-домен) — результат `search.searchVault`; импорт type-only (в рантайме стирается).
import type { NoteRef } from '../vault/types';
import type { SearchHit } from './types';

/**
 * Search-домен (F-2d): поиск по метаданным (Ф0) и гибридный поиск по телу (вектор + FTS5 (+граф) →
 * RRF, §6.2). Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/vault`); потребители ходят сюда
 * по-прежнему через `tauriApi.search` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const search = {
  /** Поиск по title/path/tags (метаданные, Ф0). */
  searchVault: (query: string): Promise<NoteRef[]> =>
    bridge<NoteRef[]>('search_vault', { query }, () => mockVault.searchVault(query)),

  /**
   * Гибридный поиск по ТЕЛУ (вектор + FTS5 (+граф) → RRF, §6.2). `limit` по умолчанию 10.
   * `folder`/`tag` — префильтр по метаданным ДО KNN; `center` — открытый файл (граф-ранг).
   */
  searchContent: (
    query: string,
    opts?: { limit?: number; folder?: string; tag?: string; center?: string },
  ): Promise<SearchHit[]> =>
    bridge<SearchHit[]>(
      'search_content',
      {
        query,
        limit: opts?.limit,
        folder: opts?.folder,
        tag: opts?.tag,
        center: opts?.center,
      },
      () => mockVault.searchContent(query, opts),
    ),
};
