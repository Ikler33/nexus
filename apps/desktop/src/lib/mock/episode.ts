// Браузер-мок эпизодической памяти (EP-3) — вне Tauri (превью/тесты). Зеркалит КОНТРАКТ команд
// `episode_*` (урок mock-must-match-backend): purge РЕАЛЬНО удаляет строку; dismiss/restore только
// тогглят флаг (строку не трогают); setEnabled персистит тоггл. Сид — пара эпизодов для превью панели.

import type { EpisodeRow } from '../tauri-api';

let episodes: EpisodeRow[] = [
  {
    id: 1,
    sessionId: 11,
    sessionTitle: 'Настройка SearXNG на VPS',
    summary:
      'Обсуждали, как поднять SearXNG в Docker на VPS: проброс порта 8888, включение JSON-формата. Договорились не открывать firewall для локального тоннеля.',
    topics: ['SearXNG', 'Docker', 'VPS'],
    startedAt: 1_718_700_000,
    endedAt: 1_718_703_600,
    generatedAt: 1_718_710_000,
    dismissed: false,
  },
  {
    id: 2,
    sessionId: 12,
    sessionTitle: 'Граф связей заметок',
    summary:
      'Разбирались с физикой графа: разлёт узлов, warmup-тики, cool-to-stop. Решили вынести параметры в настройки и добавить halo текущей ноты.',
    topics: ['граф', 'физика', 'настройки'],
    startedAt: 1_718_600_000,
    endedAt: 1_718_603_600,
    generatedAt: 1_718_610_000,
    dismissed: false,
  },
];
let enabled = false;

/** Обратная хронология по endedAt (как бэкенд). */
export function list(): Promise<EpisodeRow[]> {
  return Promise.resolve([...episodes].sort((a, b) => b.endedAt - a.endedAt));
}

export function dismiss(id: number): Promise<void> {
  episodes = episodes.map((e) => (e.id === id ? { ...e, dismissed: true } : e));
  return Promise.resolve();
}

export function restore(id: number): Promise<void> {
  episodes = episodes.map((e) => (e.id === id ? { ...e, dismissed: false } : e));
  return Promise.resolve();
}

/** Жёсткое удаление — РЕАЛЬНО убирает строку (как бэкенд DELETE + вектор), необратимо. */
export function purge(id: number): Promise<void> {
  episodes = episodes.filter((e) => e.id !== id);
  return Promise.resolve();
}

export function getEnabled(): Promise<boolean> {
  return Promise.resolve(enabled);
}

export function setEnabled(on: boolean): Promise<void> {
  enabled = on;
  return Promise.resolve();
}

/** Тест-хелпер: сброс к исходному состоянию. */
export function __reset(): void {
  enabled = false;
  episodes = [
    {
      id: 1,
      sessionId: 11,
      sessionTitle: 'Настройка SearXNG на VPS',
      summary: 'Обсуждали, как поднять SearXNG в Docker на VPS.',
      topics: ['SearXNG', 'Docker'],
      startedAt: 1_718_700_000,
      endedAt: 1_718_703_600,
      generatedAt: 1_718_710_000,
      dismissed: false,
    },
    {
      id: 2,
      sessionId: 12,
      sessionTitle: 'Граф связей заметок',
      summary: 'Разбирались с физикой графа.',
      topics: ['граф'],
      startedAt: 1_718_600_000,
      endedAt: 1_718_603_600,
      generatedAt: 1_718_610_000,
      dismissed: false,
    },
  ];
}
