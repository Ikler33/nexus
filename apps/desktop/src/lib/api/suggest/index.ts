import * as mockTags from '../../mock/tags';
import * as mockVault from '../../mock/vault';
import { bridge } from '../bridge';
import type { LinkSuggestion, TagSuggestion } from './types';

/**
 * Suggest-домен (F-2d): предложения по индексу vault — связи (Ф1-9 max-sim), похожие заметки (#35),
 * LLM-резюме заметки, объяснение связи пары (AIP-10), стартовые вопросы чата (AIP-SQ), closed-vocab
 * авто-теги (AI-2c). Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/*`); потребители ходят сюда
 * по-прежнему через `tauriApi.suggest` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const suggest = {
  /** Предложения связей для файла (режим 1 max-sim, Ф1-9). Вне Tauri — мок. */
  forFile: (path: string, limit?: number): Promise<LinkSuggestion[]> =>
    bridge<LinkSuggestion[]>('get_link_suggestions', { path, limit }, () =>
      mockVault.getLinkSuggestions(path, limit),
    ),

  /** «Похожие заметки» (#35, дискавери — включая уже связанные). Порог — на стороне UI. Вне Tauri — мок. */
  related: (path: string, limit?: number): Promise<LinkSuggestion[]> =>
    bridge<LinkSuggestion[]>('get_related_notes', { path, limit }, () =>
      mockVault.getRelatedNotes(path, limit),
    ),

  /** Inspector «Резюме»: краткое LLM-резюме текущего текста заметки (one-shot, не-стрим). `null` =
   *  нет утилитарной модели / пустой текст / пустой ответ → фронт показывает заглушку. Вне Tauri — мок. */
  noteSummary: (text: string): Promise<string | null> =>
    bridge<string | null>('get_note_summary', { text }, () => mockVault.noteSummary(text)),

  /** AIP-10: короткое LLM-объяснение связи пары заметок (вместо сырого сниппета; кэш на бэке).
   *  Пустая строка = нет утилитарной модели / ошибка / нет контента → фронт показывает сниппет.
   *  Вне Tauri — '' (естественный фолбэк на сниппет). */
  explainRelation: (pathA: string, pathB: string): Promise<string> =>
    bridge<string>('explain_relation', { pathA, pathB }, () => mockVault.explainRelation()),

  /** AIP-SQ: до 3 коротких стартовых вопросов по активной заметке `center` для пустого чата.
   *  Пустой список = нет утилитарной модели / нет контента / ошибка LLM → фронт показывает
   *  статические подсказки. Вне Tauri — [] (естественный фолбэк на статику). */
  startingQuestions: (center?: string): Promise<string[]> =>
    bridge<string[]>('get_starting_questions', { center }, () => mockVault.startingQuestions()),

  /** AI-2c: closed-vocab авто-тег — `chat_util` предлагает теги ТОЛЬКО из словаря vault. `tags` уже
   *  отфильтрованы по словарю; пустой список = нет утилитарной модели / нет контента / нет тегов → фронт
   *  показывает «нет предложений». НЕ пишет. Вне Tauri — мок (зеркалит контракт: vocab-фильтр + пусто). */
  suggestTags: (path: string): Promise<TagSuggestion> =>
    bridge<TagSuggestion>('suggest_tags', { path }, () => mockTags.suggestTags()),
};
