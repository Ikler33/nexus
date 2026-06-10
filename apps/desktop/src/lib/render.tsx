import type { ReactNode } from 'react';

/**
 * `**жирные**` фрагменты LLM-текста → <strong> (макет: серифные AI-тексты с акцентами).
 * Не markdown-движок — только bold, как в `home.jsx`/`insights.jsx`.
 */
export function renderBold(text: string): ReactNode[] {
  return text
    .split(/\*\*(.+?)\*\*/g)
    .map((part, i) => (i % 2 === 1 ? <strong key={`${i}-${part.slice(0, 12)}`}>{part}</strong> : part));
}
