/**
 * Мок ленты новостей (NF-5) для браузерного dev/vitest: in-memory состояние с той же
 * семантикой, что бэкенд-команды NF-3 (страница/прочитано/в заметку/конфиг). Контент
 * зеркалит мок дизайн-прототипа (`news.jsx` хендоффа) — превью сверяется с макетом 1:1.
 */
import type {
  LinkSuggestion,
  NewsArticle,
  NewsConfig,
  NewsEndpointHealth,
  NewsItem,
  NewsPage,
  NewsRun,
  NewsSource,
} from '../tauri-api';

const NOW = Math.floor(Date.now() / 1000);
const H = 3600;

let config: NewsConfig = {
  enabled: true,
  sources: {},
  keywords: null,
  extraHosts: [],
  modelPref: null,
};

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
    commentsUrl: null,
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
    commentsUrl: null,
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
    commentsUrl: null,
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
    commentsUrl: 'https://news.ycombinator.com/item?id=40000004',
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
    commentsUrl: null,
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
    commentsUrl: null,
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
  llmDown: null, // B12: зеркалим контракт бэкенда (здоровый LLM → null)
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

/**
 * P1-8: зеркалит бэкенд `news_allow_host` (commands/news.rs:345) — идемпотентный push хоста в
 * `extra_hosts` (`extraHosts` в TS). Реально мутирует module-level config, чтобы превью/тесты
 * «Разрешить хост» отражали добавление, а не врали возвратом нетронутого конфига. getConfig()
 * отдаёт копию, так что наружу module-level состояние не утекает.
 */
export async function allowHost(host: string): Promise<NewsConfig> {
  if (!config.extraHosts.includes(host)) config.extraHosts.push(host);
  return getConfig();
}

/**
 * P1-8: зеркалит бэкенд `news_disallow_host` (commands/news.rs:365) — retain-remove хоста из
 * `extra_hosts` (идемпотентно: нет хоста → no-op). Возвращает применённый конфиг.
 */
export async function disallowHost(host: string): Promise<NewsConfig> {
  config.extraHosts = config.extraHosts.filter((h) => h !== host);
  return getConfig();
}

// Полные RU-тексты reader'а (контент мокапа `news.jsx`); id=6 эмулирует HN-кейс — статья на
// хосте вне доверенных источников → политика отвечает denied (fail-closed, как бэкенд NF-6).
const BODIES: Record<number, string[]> = {
  1: [
    'OpenAI выпустила обновление GPT-5.2, главным нововведением которого стал режим длинного контекста с окном до 2 миллионов токенов. Это позволяет загружать в модель целые кодовые базы, длинные юридические документы или книги без предварительной нарезки на фрагменты.',
    'По данным компании, стоимость обработки на длинных входах снижена примерно на 30% за счёт нового механизма кэширования внимания, который переиспользует уже вычисленные представления при повторных запросах к одному и тому же документу.',
    'Режим доступен в API уже сегодня; веб-интерфейс ChatGPT получит его в течение недели. Для корпоративных тарифов окно контекста включено по умолчанию, для остальных — как опция с отдельной тарификацией.',
    'Разработчики отмечают, что качество извлечения фактов из середины контекста — проблема «lost in the middle», от которой страдали предыдущие поколения, — заметно выросло. На внутреннем бенчмарке точность ответов по фактам из центральной трети документа поднялась с 71% до 89%.',
    'Независимые тесты сообщества пока ограничены, но первые замеры подтверждают: модель уверенно держит ссылки и определения, введённые в начале очень длинного ввода, и корректно использует их в конце.',
    'Критики указывают на риск чрезмерной зависимости от «грубой силы» контекстного окна вместо аккуратного retrieval: загрузка двух миллионов токенов в каждый запрос дороже и медленнее, чем точечный поиск нужных фрагментов. OpenAI отвечает, что два подхода дополняют друг друга.',
  ],
  2: [
    'Mistral представила Mistral Large 3 с открытыми весами под новой лицензией, которая разрешает коммерческое использование без отчислений. Это прямой вызов закрытым моделям среднего класса и заметный сдвиг в стратегии компании.',
    'На стандартных бенчмарках — MMLU, GSM8K, HumanEval — модель показывает результаты, сопоставимые с проприетарными решениями среднего сегмента, уступая лишь флагманам последнего поколения.',
    'Веса уже доступны на Hugging Face в форматах safetensors и GGUF. Опубликованы варианты на 8B, 24B и 70B параметров, что покрывает диапазон от ноутбука до серверной стойки.',
    'Сообщество локального инференса встретило релиз с энтузиазмом: квантизованная версия Q4 модели 24B запускается на одной потребительской видеокарте с 16 ГБ памяти и выдаёт около 40 токенов в секунду.',
    'Лицензия Apache 2.0 снимает юридические барьеры, из-за которых компании опасались строить продукты на открытых моделях.',
  ],
  4: [
    'Команда llama.cpp добавила механизм офлоада KV-cache на системную память, который позволяет держать модели уровня 70B в пределах 24 ГБ видеопамяти без катастрофической потери скорости.',
    'Ключевая идея — новый аллокатор, который размещает «холодные» слои кэша ключей-значений в обычной оперативной памяти и подгружает их в видеопамять по мере необходимости.',
    'На практике задержка вырастает умеренно — на 15–25% в зависимости от соотношения объёма контекста и пропускной способности шины PCIe.',
    'Изменение уже влито в main-ветку; сборки с поддержкой Metal (Apple Silicon) и CUDA доступны в разделе релизов.',
    'Автор патча подчёркивает, что это не замена покупке видеокарты с большим объёмом памяти, а способ сделать локальный запуск больших моделей реалистичным на имеющемся железе.',
  ],
  5: [
    'Саймон Уиллисон опубликовал подробный практический разбор запуска локального RAG-пайплайна на ноутбуке без единого облачного вызова — от индексации до генерации ответов.',
    'Стек получился компактным: эмбеддинги считает nomic-embed, векторное хранилище локальное, генерация идёт через llama.cpp. Всё работает на CPU и встроенной графике.',
    'Весь индекс по личной базе заметок собирается за несколько минут и занимает десятки мегабайт на диске. Обновление инкрементальное: при изменении заметки переиндексируется только она.',
    'Автор подчёркивает приватность подхода: данные не покидают устройство ни на одном этапе, а качество ответов на персональной базе зачастую выше, чем у универсальных облачных ассистентов.',
    'В конце статьи приведены готовые скрипты и оценка стоимости: разовая настройка занимает около часа, дальнейшее использование — бесплатно.',
  ],
  3: [
    'Hugging Face запустил публичный лидерборд локальных моделей, который ранжирует их по скорости инференса, измеренной в токенах в секунду, а не только по качеству ответов.',
    'Замеры проводятся на нескольких референсных конфигурациях — потребительских GPU разных классов и чипах Apple Silicon, — что даёт практический ориентир тем, кто запускает модели на своём железе.',
    'Рейтинг учитывает разные форматы квантизации (от FP16 до Q4) и длины контекста, поскольку и то, и другое сильно влияет на реальную пропускную способность.',
    'Инициатива закрывает давний пробел: большинство существующих лидербордов сравнивали только качество, оставляя вопрос «а с какой скоростью это работает на моём ноутбуке» без ответа.',
  ],
};

const SUMMARIES: Record<number, string[]> = {
  1: [
    'Окно контекста расширено до 2M токенов — целые кодовые базы и книги без нарезки.',
    'Длинные входы дешевле ≈на 30% благодаря новому кэшу внимания.',
    'Точность по фактам из середины контекста: 71% → 89%.',
    'В API сегодня, в ChatGPT — в течение недели.',
  ],
  2: [
    'Открытые веса под Apache 2.0 — коммерция без отчислений.',
    'Качество на уровне закрытых моделей среднего класса.',
    'Варианты 8B/24B/70B на Hugging Face (safetensors, GGUF).',
    'Q4-версия 24B идёт на одной 16 ГБ видеокарте ≈40 ток/с.',
  ],
  4: [
    'KV-cache можно офлоадить в RAM — модели 70B влезают в 24 ГБ VRAM.',
    'Задержка растёт умеренно (15–25%).',
    'Уже в main; сборки под Metal и CUDA, управляется флагом.',
    'Не замена большой видеокарте, но раскрывает имеющееся железо.',
  ],
  5: [
    'Локальный RAG на ноутбуке без облака: nomic-embed + llama.cpp.',
    'Индекс по заметкам собирается за минуты, занимает десятки МБ.',
    'Полная приватность — данные не покидают устройство.',
    'Готовые скрипты и оценка стоимости в статье.',
  ],
  3: [
    'Лидерборд ранжирует локальные модели по скорости (ток/с), а не только качеству.',
    'Замеры на потребительских GPU и Apple Silicon.',
    'Учтены форматы квантизации и длины контекста.',
    'Закрывает пробел «а как быстро это на моём железе».',
  ],
};

export async function article(id: number): Promise<NewsArticle> {
  await new Promise((r) => setTimeout(r, 600)); // эмуляция фетча+перевода
  const paras = BODIES[id];
  if (!paras) {
    return { status: 'denied', message: 'хост не разрешён политикой эгресса ядра' };
  }
  const it = items.find((x) => x.id === id);
  return { status: 'ready', paras, translated: !(it?.langRu ?? false), truncated: false };
}

export async function summarize(id: number): Promise<string[]> {
  await new Promise((r) => setTimeout(r, 900)); // эмуляция LLM
  const it = items.find((x) => x.id === id);
  return SUMMARIES[id] ?? (it?.summaryRu ? [it.summaryRu] : []);
}

/** Связанные заметки vault (FLOW): мок отдаёт пару карточек для известных новостей, пусто — иначе.
 *  Пути указывают на РЕАЛЬНЫЕ заметки demo-vault (см. mock/vault.ts) — клик в превью открывает
 *  настоящий контент, а не плейсхолдер, чтобы дизайн-приёмка FLOW была честной. */
const RELATED: Record<number, LinkSuggestion[]> = {
  1: [
    {
      path: 'Projects/Roadmap.md',
      title: 'Roadmap',
      score: 0.031,
      reason: '…план проекта Alpha и приоритеты ближайших итераций…',
    },
    {
      path: 'Projects/Alpha/Spec.md',
      title: 'Alpha Spec',
      score: 0.021,
      reason: '…спецификация модуля и связи с дорожной картой…',
    },
  ],
  2: [
    {
      path: 'Notes/Idea.md',
      title: 'Idea',
      score: 0.027,
      reason: '…идея с тегом #idea и ссылкой на протокол встречи…',
    },
  ],
};

export async function related(id: number, limit?: number): Promise<LinkSuggestion[]> {
  await new Promise((r) => setTimeout(r, 400)); // эмуляция RAG-поиска
  return (RELATED[id] ?? []).slice(0, limit ?? 6);
}

// W-39 «Диагностика»: история прогонов (свежие сверху) — текущий прогон + два прошлых
// (один частичный с ошибкой источника, один чистый), чтобы превью панели было показательным.
const RUNS_HISTORY: NewsRun[] = [
  run ? { ...run } : ({} as NewsRun),
  {
    runAt: NOW - 26 * H,
    digestRu: 'Спокойный день: пара релизов инференса, без крупных анонсов моделей.',
    itemsNew: 3,
    sourcesOk: 6,
    sourcesTotal: 6,
    llmFailed: 0,
    errors: [],
    llmDown: null,
  },
  {
    runAt: NOW - 50 * H,
    digestRu: 'Анонс открытых весов и обзор локального RAG-стека.',
    itemsNew: 5,
    sourcesOk: 4,
    sourcesTotal: 6,
    llmFailed: 0,
    errors: ['DeepMind блог: 503, будет повтор', 'Simon Willison: таймаут (15с)'],
    llmDown: null,
  },
].filter((r) => r.runAt);

/** W-39: история последних прогонов (свежие сверху, до `limit`). */
export async function runs(limit: number): Promise<NewsRun[]> {
  return RUNS_HISTORY.slice(0, limit).map((r) => ({ ...r, errors: [...r.errors] }));
}

/** W-39: пинг провайдера новостей — мок отдаёт «доступен» с правдоподобной латентностью. */
export async function testEndpoint(): Promise<NewsEndpointHealth> {
  await new Promise((r) => setTimeout(r, 350)); // эмуляция пинга
  return {
    ok: true,
    message: 'анализатор новостей доступен',
    endpoint: 'http://192.168.0.28:8084',
    latencyMs: 42,
  };
}

/** W-39: экспорт логов — в браузер-превью fs нет, возвращаем фиктивный путь. */
export async function exportLogs(): Promise<string | null> {
  return 'nexus-news.log';
}
