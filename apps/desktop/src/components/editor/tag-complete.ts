// Чистый матчер контекста автокомплита тегов (PROP-4, §8/§14.5): определяет, печатает ли пользователь
// тег — инлайн `#tag` ИЛИ значение в frontmatter `tags:`-инлайн-списке — и возвращает уже набранный
// префикс. Без CodeMirror — юнит-тестируемо. Регекс-контекст (§14.5): `#` НЕ в начале заголовка
// (`# ` со ВПР отбрасывается — после `#` нужен tag-символ), не в инлайн-code-span (нечётные ``` `).

/** Tag-символы: буквы/цифры/`_`/`-`/`/` (вложенность `#a/b`); Unicode (кириллица — owner-critical). */
const TAG_CHARS = '[\\p{L}\\p{N}_/-]';
const INLINE_HASH = new RegExp(`(?:^|\\s)#(${TAG_CHARS}*)$`, 'u');
const FM_LIST = new RegExp(`^\\s*(?:tags|aliases):\\s*\\[[^\\]]*?(${TAG_CHARS}*)$`, 'u');

/**
 * Текст строки ДО курсора → набранный префикс тега, если это контекст автокомплита тегов; иначе `null`.
 * Инлайн `#тег` (но не заголовок `# ` и не внутри `` `code` ``) или `tags: [a, b|` в frontmatter.
 */
export function tagCompletionQuery(before: string): string | null {
  // Внутри инлайн-code-span (нечётное число одиночных бэктиков до курсора) — не автокомплитим.
  if (((before.match(/`/g) || []).length & 1) === 1) return null;

  const hash = INLINE_HASH.exec(before);
  if (hash) return hash[1];
  const fm = FM_LIST.exec(before);
  if (fm) return fm[1];
  return null;
}
