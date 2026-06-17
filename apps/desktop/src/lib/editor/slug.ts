/**
 * Slug заголовков для якорей режима чтения (HEADANCHOR-1): GitHub/Obsidian-стиль — нижний регистр,
 * пробелы → `-`, выкидываем пунктуацию, схлопываем повторные дефисы. Unicode сохраняем (как Obsidian:
 * `## Раздел` → `раздел`). Чистая функция — тестируется отдельно.
 */
export function slugify(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, '') // оставляем только буквы/цифры/пробел/дефис (Unicode-aware)
    .replace(/\s+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-+|-+$/g, '');
}

/**
 * Дедупликатор slug в пределах ОДНОГО документа: повторные заголовки → `slug-1`, `slug-2` (как GitHub).
 * Возвращает функцию с замыканием на собственную Map — создавайте НОВЫЙ инстанс на каждый рендер, иначе
 * счётчики утекут между заметками. Пустой slug (заголовок из одной пунктуации) → `section`.
 */
export function makeSlugger(): (text: string) => string {
  const seen = new Map<string, number>();
  return (text: string) => {
    const base = slugify(text) || 'section';
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return n === 0 ? base : `${base}-${n}`;
  };
}
