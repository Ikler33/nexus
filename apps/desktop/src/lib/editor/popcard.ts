/**
 * Hermes-8 S7 — чистые хелперы построения ховер-превью (`.popcard`). Без побочных эффектов/async →
 * тестируются изолированно. АНТИ-ФЕЙК (правило проекта): извлекаем ТОЛЬКО реальные данные заметки
 * (тип/статус из frontmatter, эксцерпт из тела); ничего не выдумываем — пустые поля опускаются вызывающим.
 */
import { basenameTitle } from './masthead';
import { extractFrontmatter, parseFrontmatterFields } from '../markdown/frontmatter';

/** Снимает ведущий frontmatter `---\n…\n---` — для эксцерпта ТЕЛА (не зеркалит строки, просто срез). */
export function stripFrontmatter(content: string): string {
  if (!content.startsWith('---\n') && !content.startsWith('---\r\n')) return content;
  const open = content.indexOf('\n') + 1;
  const lines = content.slice(open).split('\n');
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].replace(/\r$/, '') === '---') {
      return lines
        .slice(i + 1)
        .join('\n')
        .replace(/^\s*\n/, '');
    }
  }
  return content; // незакрытый блок — не угадываем
}

/** Значение поля frontmatter по имени ключа (регистронезависимо), первое значение или null. */
function fmField(content: string, key: string): string | null {
  const fm = extractFrontmatter(content);
  if (!fm) return null;
  const fields = parseFrontmatterFields(fm.raw);
  const f = fields.find((x) => x.key.toLowerCase() === key.toLowerCase());
  return f && f.values.length > 0 ? f.values[0] : null;
}

/** Тип заметки из frontmatter (`type`) для eyebrow, или null. */
export function noteType(content: string): string | null {
  return fmField(content, 'type');
}

/** Статус заметки из frontmatter (`status`) для меты, или null. */
export function noteStatus(content: string): string | null {
  return fmField(content, 'status');
}

/**
 * Заголовок превью: frontmatter `title` → первый H1 тела → basename пути. Никогда не пусто, если путь
 * задан (честный фолбэк — имя файла, а не выдуманное).
 */
export function noteTitle(content: string, path: string): string {
  const fmTitle = fmField(content, 'title');
  if (fmTitle) return fmTitle;
  const body = stripFrontmatter(content);
  const h1 = body.match(/^#\s+(.+?)\s*$/m);
  if (h1) return h1[1].trim();
  return basenameTitle(path);
}

/**
 * Эксцерпт тела для `.pc-body`: первые ~`max` символов чистого текста (без frontmatter, без ведущего H1,
 * markdown-разметка сглажена до читаемого превью), обрезка по слову + «…». Пусто → ''.
 */
export function bodyExcerpt(content: string, max = 200): string {
  let body = stripFrontmatter(content);
  // Убираем ведущий H1 (он уходит в заголовок карточки — не дублируем в теле).
  body = body.replace(/^#\s+.+?(\n|$)/, '');
  const plain = body
    .replace(/```[\s\S]*?```/g, ' ') // код-фенсы (закрытые пары)
    // НЕзакрытый фенс (нет закрывающего ```): срезаем хвост от него до конца тела — иначе сырое
    // содержимое блока (например `api_key: sk-…`) утекло бы в превью (ревью: утечка секретов).
    .replace(/```[\s\S]*$/, ' ')
    .replace(/!\[\[([^\]]+)\]\]/g, ' ') // embed-картинки
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ') // обычные картинки
    .replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_m, t, alias) => alias || t) // вики → текст
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1') // ссылки → текст
    .replace(/[*_~`>#-]/g, ' ') // markdown-символы
    .replace(/\s+/g, ' ')
    .trim();
  if (plain.length <= max) return plain;
  const slice = plain.slice(0, max);
  const lastSpace = slice.lastIndexOf(' ');
  const cut = lastSpace > max * 0.6 ? slice.slice(0, lastSpace) : slice;
  return `${cut.trimEnd()}…`;
}

/**
 * Текст сноски из её `<li id=…fn-N>`: `textContent` минус backref-стрелки (↩) и хвостовые пробелы.
 * `li` берётся вызывающим из DOM (GFM рендерит блок сносок top-level). Пусто → ''. Длинный текст
 * обрезается по слову до ~`max` символов + «…» (иначе карточка переполняла бы вьюпорт — ревью).
 */
export function footnoteText(li: Element | null, max = 400): string {
  if (!li) return '';
  // Клонируем и срезаем backref-якоря (`.data-footnote-backref`) — иначе ↩ попадёт в текст.
  const clone = li.cloneNode(true) as Element;
  clone.querySelectorAll('.data-footnote-backref, [class*="backref"]').forEach((a) => a.remove());
  const text = (clone.textContent ?? '').replace(/↩︎?/g, '').replace(/\s+/g, ' ').trim();
  if (text.length <= max) return text;
  const slice = text.slice(0, max);
  const lastSpace = slice.lastIndexOf(' ');
  const cut = lastSpace > max * 0.6 ? slice.slice(0, lastSpace) : slice;
  return `${cut.trimEnd()}…`;
}

/** Извлекает 1-based номер сноски N из href вида `#user-content-fn-N` / id `user-content-fn-N`. */
export function footnoteNumber(hrefOrId: string): string | null {
  const m = hrefOrId.match(/fn-([\w-]+)$/);
  return m ? m[1] : null;
}
