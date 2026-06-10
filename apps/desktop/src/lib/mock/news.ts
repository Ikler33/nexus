/**
 * Мок ленты новостей (NF-5) для браузерного dev/vitest: in-memory состояние с той же
 * семантикой, что бэкенд-команды NF-3 (страница/прочитано/в заметку/конфиг). Контент
 * зеркалит мок дизайн-прототипа (`news.jsx` хендоффа) — превью сверяется с макетом 1:1.
 */
import type { NewsConfig, NewsItem, NewsPage, NewsRun, NewsSource } from '../tauri-api';

const NOW = Math.floor(Date.now() / 1000);
const H = 3600;

let config: NewsConfig = { enabled: true, sources: {}, keywords: null };

const registry: NewsSource[] = [
  { id: 'openai', title: 'OpenAI', enabled: true, langRu: false },
  { id: 'deepmind', title: 'DeepMind', enabled: true, langRu: false },
  { id: 'mistral', title: 'Mistral', enabled: true, langRu: false },
  { id: 'hf-blog', title: 'Hugging Face', enabled: true, langRu: false },
  { id: 'llamacpp-releases', title: 'llama.cpp', enabled: true, langRu: false },
  { id: 'willison', title: 'Simon Willison', enabled: true, langRu: false },
];

let items: NewsItem[] = [
  {
    id: 1,
    sourceId: 'openai',
    url: 'https://openai.com/news/example',
    titleRu: 'GPT-5.2 получил режим длинного контекста до 2M токенов',
    summaryRu:
      'Обновление расширяет окно контекста и снижает стоимость на длинных документах; доступно в API с сегодняшнего дня.',
    topic: 'Модели',
    langRu: false,
    publishedAt: NOW - 2 * H,
    read: false,
  },
  {
    id: 2,
    sourceId: 'mistral',
    url: 'https://mistral.ai/news/example',
    titleRu: 'Mistral Large 3 — открытые веса для коммерческого использования',
    summaryRu:
      'Новая лицензия разрешает коммерцию без роялти; бенчмарки сопоставимы с закрытыми моделями среднего класса.',
    topic: 'Модели',
    langRu: false,
    publishedAt: NOW - 5 * H,
    read: false,
  },
  {
    id: 3,
    sourceId: 'hf-blog',
    url: 'https://huggingface.co/blog/example',
    titleRu: 'На HF появился рейтинг локальных моделей по скорости инференса',
    summaryRu: 'Лидерборд учитывает токены/сек на потребительских GPU и Apple Silicon.',
    topic: 'Модели',
    langRu: true,
    publishedAt: NOW - 9 * H,
    read: true,
  },
  {
    id: 4,
    sourceId: 'llamacpp-releases',
    url: 'https://github.com/ggml-org/llama.cpp/releases/example',
    titleRu: 'llama.cpp: офлоад KV-cache на CPU без потери скорости',
    summaryRu:
      'Новый аллокатор позволяет держать 70B-модели в 24 ГБ VRAM с приемлемой задержкой.',
    topic: 'Инференс / локальный стек',
    langRu: false,
    publishedAt: NOW - 4 * H,
    read: false,
  },
  {
    id: 5,
    sourceId: 'willison',
    url: 'https://simonwillison.net/example',
    titleRu: 'Запуск локального RAG на ноутбуке: практический разбор',
    summaryRu: 'Автор показывает пайплайн на nomic-embed и локальном векторном хранилище без облака.',
    topic: 'Инференс / локальный стек',
    langRu: false,
    publishedAt: NOW - 7 * H,
    read: false,
  },
  {
    id: 6,
    sourceId: 'deepmind',
    url: 'https://deepmind.google/discover/example',
    titleRu: 'Новый метод дистилляции снижает галлюцинации на 40%',
    summaryRu: '', // llm-fail кейс: резюме недоступно, показан заголовок (AC-NF-10)
    topic: 'Исследования',
    langRu: false,
    publishedAt: NOW - 11 * H,
    read: false,
  },
];

let run: NewsRun | null = {
  runAt: NOW - 12 * 60,
  digestRu:
    'Главное за сутки: GPT-5.2 расширил контекст до 2M токенов, Mistral Large 3 вышел с открытыми весами под коммерцию. В локальном стеке — офлоад KV-cache в llama.cpp снимает барьер 70B на 24 ГБ. В исследованиях — новый метод дистилляции против галлюцинаций.',
  itemsNew: 6,
  sourcesOk: 5,
  sourcesTotal: 6,
  llmFailed: 1,
  errors: ['Mistral блог: таймаут (15с), будет повтор в следующем прогоне'],
};

export async function page(opts?: {
  topic?: string;
  unreadOnly?: boolean;
  page?: number;
}): Promise<NewsPage> {
  const filtered = items
    .filter((it) => !opts?.topic || it.topic === opts.topic)
    .filter((it) => !opts?.unreadOnly || !it.read)
    .sort((a, b) => b.publishedAt - a.publishedAt);
  const topics = [...new Set(items.map((it) => it.topic))];
  return { items: filtered, topics, run: run ? { ...run } : null };
}

export async function markRead(id: number, read: boolean): Promise<void> {
  items = items.map((it) => (it.id === id ? { ...it, read } : it));
}

export async function toNote(id: number): Promise<string> {
  const it = items.find((x) => x.id === id);
  if (!it) throw new Error(`запись ленты не найдена: ${id}`);
  return `News/2026-06-10 ${it.titleRu.slice(0, 24)}.md`;
}

export async function refresh(): Promise<boolean> {
  // Эмулируем прогон: через секунду «обновился» runAt (без новых записей).
  setTimeout(() => {
    if (run) run = { ...run, runAt: Math.floor(Date.now() / 1000) };
  }, 1000);
  return true;
}

export async function getConfig(): Promise<NewsConfig> {
  return { ...config, sources: { ...config.sources } };
}

export async function setConfig(next: NewsConfig): Promise<NewsConfig> {
  config = { ...next, sources: { ...next.sources } };
  return getConfig();
}

export async function sources(): Promise<NewsSource[]> {
  return registry.map((s) => ({ ...s, enabled: config.sources[s.id] ?? s.enabled }));
}
