/**
 * Мок HOME-дашборда (DP-1) для браузерного dev/vitest: контент в духе макета `home.jsx`
 * хендоффа — превью сверяется с дизайном. Heatmap детерминированный (без Math.random —
 * стабильные тесты/скриншоты).
 */
import type {
  GoalEntry,
  HomeActivity,
  HomeData,
  StaleNote,
  Widget,
} from '../tauri-api';

const NOW = Math.floor(Date.now() / 1000);
const H = 3600;
const DAY = 86_400;

const goals: GoalEntry[] = [
  { path: 'Projects/Nexus.md', title: 'Nexus MVP', progress: 72 },
  { path: 'Projects/Agents.md', title: 'Архитектура агентов', progress: 45 },
  { path: 'Projects/Eval.md', title: 'Eval-харнесс', progress: 88 },
];

export async function data(): Promise<HomeData> {
  return {
    stats: { notes: 847, tags: 42, links: 1923, words: 412_380 },
    recent: [
      { path: 'Research/RAG Pipeline.md', title: 'RAG Pipeline', updatedAt: NOW - 12 * 60, words: 1840 },
      { path: 'Projects/Agents.md', title: 'Архитектура агентов', updatedAt: NOW - 2 * H, words: 3250 },
      { path: 'Research/Embeddings.md', title: 'Embeddings', updatedAt: NOW - 5 * H, words: 920 },
      { path: 'Notes/Идеи.md', title: 'Идеи', updatedAt: NOW - 9 * H, words: 480 },
      { path: 'Inbox.md', title: 'Inbox', updatedAt: NOW - DAY, words: 210 },
    ],
    goals,
  };
}

export async function activity(): Promise<HomeActivity> {
  // Детерминированная «активность»: волна по синусу с пропусками.
  const heatmap = [];
  for (let ago = 0; ago < 17 * 7; ago++) {
    const v = Math.sin(ago * 0.7) + Math.sin(ago * 0.23) * 0.8;
    const count = v > 0.4 ? Math.round(v * 4) : ago % 9 === 3 ? 1 : 0;
    if (count > 0) heatmap.push({ daysAgo: ago, count });
  }
  return {
    heatmap,
    changesToday: 12,
    week: 38,
    prevWeek: 31,
    streakDays: 23,
    bestStreak: 31,
    orphans: 94,
    continue: {
      path: 'Research/RAG Pipeline.md',
      title: 'RAG Pipeline',
      updatedAt: NOW - 12 * 60,
      words: 1840,
      snippet:
        'Гибридный ретрив: векторный поиск дополняется FTS5, результаты сливаются RRF-ранкингом. Следующий шаг — замерить влияние чанк-перекрытия на точность…',
    },
  };
}

const widgets: Record<string, Widget> = {
  daily_brief: {
    key: 'daily_brief',
    content:
      'Активная работа над **архитектурой агентов** — 5 заметок за два дня, плотная связка с **eval-харнессом**. Кластер «RAG Pipeline» вырос на 3 заметки; появились первые записи о **смещении фокуса** в сторону инфраструктуры. Стоит вернуться к «Embeddings» — черновик не трогали 9 дней.',
    generatedAt: NOW - 40 * 60,
    sourceHash: 1,
    status: 'ready',
    stale: false,
  },
  open_questions: {
    key: 'open_questions',
    content: JSON.stringify([
      { question: 'Как чанк-перекрытие влияет на точность ретрива длинных заметок?', path: 'Research/RAG Pipeline.md' },
      { question: 'Достаточно ли bge-m3 для кода, или нужен отдельный эмбеддер?', path: 'Research/Embeddings.md' },
      { question: 'Где граница между агентом-планировщиком и сценарным пайплайном?', path: 'Projects/Agents.md' },
    ]),
    generatedAt: NOW - 2 * H,
    sourceHash: 1,
    status: 'ready',
    stale: false,
  },
  context_drift: {
    key: 'context_drift',
    content:
      'Заявленный фокус — Nexus MVP, но последние десять правок уходят в инфраструктуру: eval-харнесс, серверные конфиги, перенос дизайна. Цель «Архитектура агентов» не получала записей шесть дней — если это осознанная пауза, зафиксируйте её; иначе фокус размывается.',
    generatedAt: NOW - 3 * H,
    sourceHash: 1,
    status: 'ready',
    stale: false,
  },
};

export async function widget(key: string): Promise<Widget | null> {
  return widgets[key] ?? null;
}

export async function refresh(key: string): Promise<void> {
  // Эмуляция фоновой регенерации: чуть свежее время генерации.
  const w = widgets[key];
  if (w) widgets[key] = { ...w, generatedAt: Math.floor(Date.now() / 1000) };
}

// Тоггл «Инсайты» (зеркало бэкенд-сеттинга `insights.enabled`, дефолт OFF). setEnabled персистит флаг
// в памяти процесса (мок-бэкенд без БД); kick-джобы не эмулируем (виджеты уже сидированы выше).
let insightsEnabled = false;

export function insightsGetEnabled(): Promise<boolean> {
  return Promise.resolve(insightsEnabled);
}

export function insightsSetEnabled(on: boolean): Promise<void> {
  insightsEnabled = on;
  return Promise.resolve();
}

export async function staleRadar(): Promise<StaleNote[]> {
  const base = {
    isDraft: false,
    isWip: false,
    isOverdue: false,
    isOrphan: false,
    isEvergreen: false,
    reason: null,
    action: null,
    hint: null,
  };
  return [
    { ...base, path: 'Projects/Roadmap Q1.md', title: 'Roadmap Q1', score: 82, severity: 'red', ageDays: 94, isOverdue: true, action: 'update' },
    { ...base, path: 'Research/Vector DB сравнение.md', title: 'Vector DB сравнение', score: 71, severity: 'red', ageDays: 61, isOrphan: true, action: 'archive' },
    { ...base, path: 'Notes/Конспект курса.md', title: 'Конспект курса', score: 44, severity: 'orange', ageDays: 38, isDraft: true, action: 'split' },
    { ...base, path: 'Notes/Черновик статьи.md', title: 'Черновик статьи', score: 35, severity: 'orange', ageDays: 29, isWip: true, action: 'update' },
  ];
}
