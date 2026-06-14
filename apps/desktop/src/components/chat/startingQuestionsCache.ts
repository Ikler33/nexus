/**
 * AIP-SQ: session-кэш контекстных стартовых вопросов чата по пути активной заметки. Вопросы дёшевы и
 * не обязаны переживать рестарт (решение design-анализа: без персиста/миграции). Намеренно НЕ
 * инвалидируется при правке заметки в той же сессии (стартовые вопросы — лёгкий прайминг, деградируют
 * на статику). Сбрасывается при смене vault (`clearStartingQuestionsCache` из `vault.openVault`) —
 * иначе одноимённая заметка в другом хранилище отдала бы вопросы по чужому содержимому.
 *
 * Вынесен из компонента отдельным модулем: синглтон-стейт не должен жить в файле React-компонента
 * (иначе ломается fast-refresh, react-refresh/only-export-components).
 */
const cache = new Map<string, string[]>();

/** Закэшированные вопросы для заметки или `undefined` (промах). */
export function getCachedQuestions(center: string): string[] | undefined {
  return cache.get(center);
}

/** Кэширует вопросы (в т.ч. пустой массив — чтобы не дёргать LLM повторно за сессию). */
export function setCachedQuestions(center: string, questions: string[]): void {
  cache.set(center, questions);
}

/** Сбрасывает кэш (смена vault; в тестах — изоляция кейсов). */
export function clearStartingQuestionsCache(): void {
  cache.clear();
}
