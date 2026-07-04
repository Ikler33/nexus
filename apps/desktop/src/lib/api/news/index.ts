import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import * as mockNews from '../../mock/news';
import { bridge, isTauri, subscribe } from '../bridge';
// `LinkSuggestion` (suggest-домен) — результат `news.related`, принадлежит ЧУЖОМУ домену и до
// своего среза F-2d+ живёт в барреле. Импорт type-only — в рантайме стирается, цикла баррел ↔
// домен нет (тот же паттерн, что у `lib/mock/*` и `chat/types.ts`).
import type { LinkSuggestion } from '../../tauri-api';
import type {
  NewsArticle,
  NewsConfig,
  NewsEndpointHealth,
  NewsPage,
  NewsRun,
  NewsSource,
} from './types';

/**
 * News-домен (F-2c): лента AI-новостей (NF-3/NF-5, спека `docs/specs/news-feed.md`) — страница/
 * прочитано/в заметку, конфиг+источники+per-host consent, ридер (NF-6), прогоны/диагностика
 * (W-39/40), экспорт логов. Request/response-вызовы — через `bridge` (Tauri ↔ мок `lib/mock/news`);
 * потребители ходят сюда по-прежнему через `tauriApi.news`/`tauriApi.events.onNewsProgress`
 * (barrel-реэкспорт в `lib/tauri-api.ts`).
 */

/** Лента AI-новостей (NF-3/NF-5). Прогон гоняет планировщик (kind `newsfeed`); готовность —
 * событие `jobs:changed`. Вне Tauri — стейтфул-мок. */
export const news = {
  /** Страница ленты: записи (свежие сверху) + чипы тем + последний прогон. */
  page: (opts?: { topic?: string; unreadOnly?: boolean; page?: number }): Promise<NewsPage> =>
    bridge<NewsPage>(
      'get_news',
      { topic: opts?.topic, unreadOnly: opts?.unreadOnly, page: opts?.page },
      () => mockNews.page(opts),
    ),

  /** Отметка прочитано/непрочитано (AC-NF-9). */
  markRead: (id: number, read: boolean): Promise<void> =>
    bridge<void>('news_mark_read', { id, read }, () => mockNews.markRead(id, read)),

  /** «В заметку» (AC-NF-11): создаёт `News/<дата> <заголовок>.md`, возвращает путь заметки. */
  toNote: (id: number): Promise<string> =>
    bridge<string>('news_to_note', { id }, () => mockNews.toNote(id)),

  /** Ручной прогон «Обновить» (AC-NF-6): ставит джобу с дедупом; `false` — уже в очереди. */
  refresh: (): Promise<boolean> =>
    bridge<boolean>('refresh_news', undefined, () => mockNews.refresh()),

  /** Конфиг `news.json` (consent + источники + ключи). */
  getConfig: (): Promise<NewsConfig> =>
    bridge<NewsConfig>('get_news_config', undefined, () => mockNews.getConfig()),

  /** Разрешить хост статьи (per-host consent из Denied-баннера ридера). Возвращает конфиг. */
  allowHost: (host: string): Promise<NewsConfig> =>
    bridge<NewsConfig>('news_allow_host', { host }, () => mockNews.allowHost(host)),

  /** Снять разрешение с хоста (gear-меню ленты). Возвращает конфиг. */
  disallowHost: (host: string): Promise<NewsConfig> =>
    bridge<NewsConfig>('news_disallow_host', { host }, () => mockNews.disallowHost(host)),

  /** Сохраняет конфиг и мгновенно синхронизирует политику эгресса (NF-4, AC-NF-7). */
  setConfig: (config: NewsConfig): Promise<NewsConfig> =>
    bridge<NewsConfig>('set_news_config', { config }, () => mockNews.setConfig(config)),

  /** Реестр источников v1 с действующими флагами — consent показывает, куда пойдут запросы. */
  sources: (): Promise<NewsSource[]> =>
    bridge<NewsSource[]>('news_sources', undefined, () => mockNews.sources()),

  /** Полный текст статьи для reader (NF-6): кэш → guarded-фетч → RU-перевод. Долгий вызов. */
  article: (id: number): Promise<NewsArticle> =>
    bridge<NewsArticle>('news_article', { id }, () => mockNews.article(id)),

  /** «Сократить» (NF-6): 3–6 RU-тезисов по тексту статьи. */
  summarize: (id: number): Promise<string[]> =>
    bridge<string[]>('news_summarize', { id }, () => mockNews.summarize(id)),

  /** FLOW: заметки vault, релевантные новости (RAG по заголовку+резюме). Заметка, созданная из
   *  этой же новости (frontmatter `source`==url), отфильтрована. Пусто, если RAG/индекс недоступны. */
  related: (id: number, limit?: number): Promise<LinkSuggestion[]> =>
    bridge<LinkSuggestion[]>('news_related', { id, limit }, () => mockNews.related(id, limit)),

  /** W-39 «Диагностика»: история последних прогонов (свежие сверху, до `limit`). */
  runs: (limit: number): Promise<NewsRun[]> =>
    bridge<NewsRun[]>('get_news_runs', { limit }, () => mockNews.runs(limit)),

  /** W-39: пинг провайдера новостей (анализатор `ai.fast`→`ai.chat`) через политику эгресса. */
  testEndpoint: (): Promise<NewsEndpointHealth> =>
    bridge<NewsEndpointHealth>('news_test_endpoint', undefined, () => mockNews.testEndpoint()),

  /** W-39: экспорт самого свежего лог-файла в файл через save-диалог. Путь файла, либо null
   *  если отменили. fs — в доверенном бэкенде (как backup W-9); путь выбирает пользователь.
   *  Bridge-исключение (см. `../bridge.ts`): путь с OS-диалогом — сначала диалог, потом invoke. */
  exportLogs: async (): Promise<string | null> => {
    if (!isTauri()) return mockNews.exportLogs();
    const path = await saveDialog({
      defaultPath: 'nexus-news.log',
      filters: [{ name: 'Log', extensions: ['log'] }],
    });
    if (!path) return null;
    await invoke<void>('export_news_logs', { path });
    return path;
  },
};

/** Событийные подписки news-домена. Вне Tauri — no-op (мок-бэкенд событий не эмитит). */
export const newsEvents = {
  /** Этапный прогресс прогона ленты (`news:progress`): sources → llm → digest → save. */
  onNewsProgress: (
    cb: (p: { stage: string; done: number; total: number }) => void,
  ): Promise<() => void> =>
    subscribe<{ stage: string; done: number; total: number }>('news:progress', cb),
};
