/**
 * W-34: незакрытый ``` (нечётное число фенсов в частичном стрим-тексте) → дорисовать закрывающий
 * фенс, чтобы недописанный код-блок рендерился как `<pre><code>`, а не ломал остальную разметку.
 * Чистая утилита (вынесена из StreamingMarkdown, чтобы файл-компонент оставался component-only —
 * react-refresh).
 */
export function closeOpenFences(s: string): string {
  const fences = (s.match(/```/g) || []).length;
  return fences % 2 === 1 ? s + '\n```' : s;
}
