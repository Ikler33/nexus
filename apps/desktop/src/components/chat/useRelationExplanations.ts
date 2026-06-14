import { useEffect, useReducer, useRef } from 'react';

import { tauriApi, type LinkSuggestion } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';

/**
 * AIP-10: ленивая подгрузка LLM-«причины связи» для карточек «Связи»/«Похожие». Сниппет (`item.reason`)
 * показывается мгновенно как плейсхолдер и финальный фолбэк; объяснение бесшовно подменяет его по
 * готовности. Архитектура (по итогам design+adversarial):
 *  - ГЛОБАЛЬНЫЙ (на модуль) кэш результатов по ключу пары (`pairKey`) — шарится между обеими вкладками
 *    И сам исключает «чужую карточку»: хук читает кэш ТОЛЬКО для текущих пар, устаревшие записи (другой
 *    activePath) просто игнорируются (epoch не нужен).
 *  - ГЛОБАЛЬНЫЙ семафор `CONCURRENCY=2` + FIFO-очередь: при N видимых карточках в полёте всегда ≤2
 *    IPC-вызова (chat_util — мелкая модель, `--parallel 1` на сервере). Дедуп по ключу.
 *  - Бэк кэширует на диске → повторы/возвраты к заметке мгновенны. Ошибка/нет модели → '' → сниппет.
 * (Видимостный IntersectionObserver не вводим: списки коротки ≤12, очередь и так сериализует; при росте
 *  списков — задел на будущее.)
 */

const CONCURRENCY = 2;
/** Ключ пары. Разделитель `\n` НЕ встречается в путях файлов (в отличие от пробела) → нет коллизий
 *  вида ('a','b c') vs ('a b','c'). Единая функция — чтобы запись и чтение кэша не разъехались. */
const pairKey = (activePath: string, candidatePath: string): string => `${activePath}\n${candidatePath}`;
/** Готовые объяснения по ключу пары. Пустые НЕ кэшируем (даём ретрай при возврате к заметке). */
const resultCache = new Map<string, string>();
/** Ключи в полёте/очереди — дедуп (один IPC на пару, даже если её просят обе вкладки). */
const inFlight = new Set<string>();
const queue: Array<{ activePath: string; candidatePath: string; key: string }> = [];
let running = 0;
/** Подписчики (хук-инстансы) — дёргаются по готовности задачи, чтобы перечитать кэш. */
const listeners = new Set<() => void>();
/** Защита от разрастания за длинную сессию: бэк — источник истины, фронт-кэш пересоберётся по запросу. */
const CACHE_CAP = 5000;

function notify() {
  for (const l of listeners) l();
}

function pump() {
  while (running < CONCURRENCY && queue.length > 0) {
    const task = queue.shift()!;
    running += 1;
    tauriApi.suggest
      .explainRelation(task.activePath, task.candidatePath)
      .catch(() => '') // ошибка IPC/LLM → '' (фолбэк на сниппет), без reject/toast-спама
      .then((expl) => {
        if (expl) {
          if (resultCache.size > CACHE_CAP) resultCache.clear();
          resultCache.set(task.key, expl);
        }
      })
      .finally(() => {
        running -= 1;
        inFlight.delete(task.key);
        notify();
        pump();
      });
  }
}

function enqueue(activePath: string, candidatePath: string) {
  const key = pairKey(activePath, candidatePath);
  if (resultCache.has(key) || inFlight.has(key)) return; // уже готово/в полёте
  inFlight.add(key);
  queue.push({ activePath, candidatePath, key });
  pump();
}

const EMPTY: Record<string, string> = {};

/** Возвращает `candidatePath → объяснение` для текущей активной заметки. Нет объяснения → ключа нет
 *  (вызывающий показывает `item.reason`). Тумблер `aiExplainRelations` ВЫКЛ → `{}` и без IPC. */
export function useRelationExplanations(
  activePath: string | null,
  items: LinkSuggestion[],
): Record<string, string> {
  const enabled = usePrefsStore((s) => s.aiExplainRelations);
  const [, force] = useReducer((x: number) => x + 1, 0);
  const itemsRef = useRef(items);
  itemsRef.current = items;
  // Стабильный ключ списка: эффект перезапускается только при смене НАБОРА путей (не на каждый рендер).
  const itemsKey = items.map((i) => i.path).join('|');

  // Подписка на готовность задач (перечитать кэш → ре-рендер с объяснениями).
  useEffect(() => {
    listeners.add(force);
    return () => {
      listeners.delete(force);
    };
  }, []);

  // Постановка пар текущей заметки в очередь (по смене активной заметки/списка/тумблера).
  useEffect(() => {
    if (!enabled || !activePath || itemsRef.current.length === 0) return;
    for (const item of itemsRef.current) {
      if (item.path !== activePath) enqueue(activePath, item.path);
    }
  }, [activePath, enabled, itemsKey]);

  if (!enabled || !activePath) return EMPTY;
  // Собираем объяснения ТОЛЬКО для текущих пар (устаревшие записи кэша других заметок игнорируются).
  const out: Record<string, string> = {};
  for (const item of items) {
    const v = resultCache.get(pairKey(activePath, item.path));
    if (v) out[item.path] = v;
  }
  return out;
}

/** Тест-хук: очистить глобальное состояние между тестами (кэш/очередь/инфлайт). */
export function __resetRelationExplanationsForTest(): void {
  resultCache.clear();
  inFlight.clear();
  queue.length = 0;
  running = 0;
  listeners.clear();
}
