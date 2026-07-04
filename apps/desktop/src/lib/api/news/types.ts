/**
 * DTO-типы news-домена (F-2c): записи/страница ленты AI-новостей (NF-3/NF-5), конфиг+источники
 * (consent AC-NF-7), прогоны/диагностика (W-39/40), статья ридера (NF-6). Зеркала Rust-структур
 * (`news` / `commands::news`) — контракт провода `invoke`. Потребители импортируют их по-прежнему
 * из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Запись ленты новостей (зеркалит Rust `news::NewsItem`, NF-3). Время — Unix-секунды. */
export interface NewsItem {
  id: number;
  sourceId: string;
  url: string;
  titleRu: string;
  summaryRu: string;
  topic: string;
  /** Источник русскоязычный (резюме без пометки «перевод»). */
  langRu: boolean;
  publishedAt: number;
  read: boolean;
  /** Ссылка на обсуждение на HN (если `url` — внешняя, напр. github у Show HN); иначе `null`. */
  commentsUrl: string | null;
}

/** B12: структурный сигнал «LLM-анализатор недоступен» (зеркалит Rust `news::LlmDownInfo`) —
 *  замена RU-префикс-протокола в `errors[]`, который FE сниффил регексом. */
export interface LlmDownInfo {
  /** URL эндпоинта оценки, который был недоступен; `null` — эндпоинт ИИ не задан. */
  endpoint: string | null;
  /** `true` — часть батчей прошла (лента обновлена частично, баннер не нужен);
   *  `false` — недоступен весь прогон (тотально → баннер). */
  partial: boolean;
}

/** Итог последнего прогона ленты (зеркалит Rust `news::NewsRun`): шапка-сводка дня. */
export interface NewsRun {
  runAt: number;
  digestRu: string;
  itemsNew: number;
  sourcesOk: number;
  sourcesTotal: number;
  llmFailed: number;
  /** Видимые ошибки источников («источник: причина») — no silent caps (AC-NF-1). */
  errors: string[];
  /** B12: сбой ВЫЗОВА LLM-оценки в прогоне; `null`/отсутствует — вызовы живы (или запись сделана
   *  версией до миграции 027 — тогда действует legacy-сниффер строки в `errors`). */
  llmDown?: LlmDownInfo | null;
}

/** Здоровье эндпоинта анализатора новостей (W-39, зеркалит Rust `commands::news::NewsEndpointHealth`):
 *  результат кнопки «Проверить связь» в панели «Диагностика». */
export interface NewsEndpointHealth {
  /** Эндпоинт ответил (любой HTTP-статус) — провайдер достижим. */
  ok: boolean;
  /** Человеко-читаемое сообщение (RU): «доступен» / причина недоступности. */
  message: string;
  /** Базовый URL пингованного эндпоинта (тот, что реально использует пайплайн новостей). */
  endpoint: string;
  /** Латентность пинга в миллисекундах. */
  latencyMs: number;
}

/** Страница ленты (зеркалит Rust `commands::news::NewsPageDto`). */
export interface NewsPage {
  items: NewsItem[];
  topics: string[];
  run: NewsRun | null;
}

/** Конфиг ленты `news.json` (зеркалит Rust `news::NewsConfig`); `enabled` = consent (AC-NF-7). */
export interface NewsConfig {
  enabled: boolean;
  /** Переопределения вкл/выкл источников реестра: id → bool. */
  sources: Record<string, boolean>;
  /** Ключевые слова фильтра; `null` — пресет по умолчанию. */
  keywords: string[] | null;
  /** Доп. хосты статей, разрешённые по клику из ридера (per-host consent, ревизия NF-6). */
  extraHosts: string[];
  /** W-40: модель пайплайна новостей — `'fast'` (`ai.fast`, дефолт) | `'main'` (`ai.chat`);
   *  `null`/неизвестное → как `'fast'` (0 регрессии). */
  modelPref: 'fast' | 'main' | null;
}

/** Источник реестра v1 (зеркалит Rust `commands::news::NewsSourceDto`) — для consent-строки. */
export interface NewsSource {
  id: string;
  title: string;
  enabled: boolean;
  langRu: boolean;
}

/** Статья reader'а (зеркалит Rust `commands::news::NewsArticleDto`, NF-6). `denied` — хост вне
 * политики эгресса (HN-домены/офлайн): fail-closed, UI отдаёт резюме + ссылку на оригинал. */
export type NewsArticle =
  | { status: 'ready'; paras: string[]; translated: boolean; truncated: boolean }
  | { status: 'denied'; message: string };
