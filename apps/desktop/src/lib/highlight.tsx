import { type ReactNode } from 'react';

/**
 * Подсветка терминов запроса в тексте через React-узлы `<mark>` — CSP-safe (без innerHTML/dangerouslySet).
 * Термины короче 2 символов игнорируются; спецсимволы regex экранируются. `markClass` — класс для `<mark>`
 * (CSS-модули скоупят имена, поэтому передаём его явно). Общий для палитры команд и сайдбар-поиска.
 */
export function highlightTerms(text: string, query: string, markClass: string): ReactNode {
  const terms = query
    .toLowerCase()
    .split(/\s+/)
    .filter((t) => t.length >= 2);
  if (!terms.length) return text;
  const escaped = terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  const re = new RegExp(`(${escaped.join('|')})`, 'gi');
  const termSet = new Set(terms);
  return text.split(re).map((part, i) =>
    termSet.has(part.toLowerCase()) ? (
      <mark key={i} className={markClass}>
        {part}
      </mark>
    ) : (
      part
    ),
  );
}
