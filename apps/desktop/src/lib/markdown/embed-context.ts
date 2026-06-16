import { createContext } from 'react';

/**
 * Контекст рекурсивной транклюзии. `ancestors` — множество УЖЕ резолвнутых путей по цепочке вставок
 * (для гард-цикла A→B→A); `depth` — глубина вложенности (бэкстоп от рекурсии, даже если резолв путей
 * почему-то не поймал цикл). Значение по умолчанию (корень дерева превью) — пустой набор, глубина 0.
 */
export interface EmbedCtx {
  ancestors: ReadonlySet<string>;
  depth: number;
}

export const EMBED_DEFAULT: EmbedCtx = {
  ancestors: new Set<string>(),
  depth: 0,
};

/** Максимальная глубина вложенных вставок — дальше показываем заглушку, не рекурсируем. */
export const MAX_EMBED_DEPTH = 4;

export const EmbedContext = createContext<EmbedCtx>(EMBED_DEFAULT);
