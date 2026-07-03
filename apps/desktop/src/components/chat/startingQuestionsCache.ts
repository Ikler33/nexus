/**
 * AIP-SQ: session-кэш контекстных стартовых вопросов чата по пути активной заметки. Вопросы дёшевы и
 * не обязаны переживать рестарт (решение design-анализа: без персиста/миграции). Намеренно НЕ
 * инвалидируется при правке заметки в той же сессии (стартовые вопросы — лёгкий прайминг, деградируют
 * на статику). Сбрасывается при смене vault (window-событие `vault:switched` от `vault.openVault`,
 * подписка внизу файла) — иначе одноимённая заметка в другом хранилище отдала бы вопросы по чужому
 * содержимому.
 *
 * Вынесен из компонента отдельным модулем: синглтон-стейт не должен жить в файле React-компонента
 * (иначе ломается fast-refresh, react-refresh/only-export-components).
 */
import { VAULT_SWITCHED_EVENT } from '../../lib/app-events';
const cache = new Map<string, string[]>();
// Кап на размер кэша (audit B11): за длинную сессию с сотнями открытых заметок Map рос бы
// неограниченно. Map хранит порядок вставки → удаляем самую старую запись (FIFO/≈LRU) при переполнении.
const CACHE_CAP = 200;

/** Закэшированные вопросы для заметки или `undefined` (промах). */
export function getCachedQuestions(center: string): string[] | undefined {
  return cache.get(center);
}

/** Кэширует вопросы (в т.ч. пустой массив — чтобы не дёргать LLM повторно за сессию). */
export function setCachedQuestions(center: string, questions: string[]): void {
  cache.delete(center); // переустановка двигает запись в конец (свежесть для FIFO-эвикции)
  cache.set(center, questions);
  if (cache.size > CACHE_CAP) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
}

/** Сбрасывает кэш (смена vault; в тестах — изоляция кейсов). */
export function clearStartingQuestionsCache(): void {
  cache.clear();
}

// Смена vault → сброс. Раньше `stores/vault` импортировал этот модуль напрямую (инверсия
// stores→components); теперь vault эмитит window-событие, а кэш чистит себя сам (F-1).
// Паттерн — существующие window-подписки на уровне модуля (ср. `resize` в stores/theme.ts).
if (typeof window !== 'undefined') {
  window.addEventListener(VAULT_SWITCHED_EVENT, clearStartingQuestionsCache);
}
