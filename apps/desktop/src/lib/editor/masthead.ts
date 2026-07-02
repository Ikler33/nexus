/**
 * Editorial masthead (Hermes-6 `editor.jsx`/`app.css`): kicker (теги) · display-title · mono-byline.
 * Чистая логика отделена от рендера `MarkdownPreview` — title из frontmatter/H1/имени файла, kicker из
 * тегов, тело с «погашенным» ведущим H1 (его текст становится заголовком масthead'а, чтобы не дублировать).
 *
 * КЛЮЧЕВОЙ ИНВАРИАНТ: H1 не ВЫРЕЗАЕТСЯ, а ОБНУЛЯЕТСЯ (строка → пустая, перевод строки сохранён) — иначе
 * сдвинулись бы номера строк, на которые завязаны тоггл тасков (EDIT-5) и переход по оглавлению (EDIT-7),
 * работающие против исходного `active.doc`. Обнуление сохраняет общее число строк и позиции всех остальных.
 */

import { extractFrontmatter, parseFrontmatterFields, type FmField } from '../markdown/frontmatter';
import { stripHeadingEmoji } from './headingText';

/** Поля frontmatter, которые выносятся в масthead (kicker/title) → не дублируем их в Properties-таблице. */
const MASTHEAD_FIELDS = new Set(['title', 'tags', 'tag']);

export interface Masthead {
  /** Заголовок (frontmatter `title` → текст ведущего H1 → имя файла). Может быть пустым. */
  title: string;
  /** Теги (значения frontmatter `tags`/`tag`, без ведущего `#`) — fallback-источник kicker'а. */
  tags: string[];
  /**
   * Текст eyebrow'а (S2): «тип · статус» из frontmatter `type`/`status` (напр. «Идея · seed»).
   * Если ни `type`, ни `status` нет — graceful fallback на теги (как было). Может быть пустым
   * (нет ни type/status, ни тегов) → eyebrow не рендерится.
   */
  kicker: string;
  /** Оставшиеся поля frontmatter для Properties-таблицы (title/tags убраны — они в масthead'е). */
  fields: FmField[];
  /** Тело с обнулённой строкой ведущего H1 (число строк и позиции сохранены). */
  body: string;
  /** 1-based номер исходной строки обнулённого H1 (для `data-outline-line` на заголовке) или null. */
  h1Line: number | null;
  /** Сырой текст ведущего H1 (с inline-разметкой) — для slug-id якоря (HEADANCHOR-1), или null. */
  h1Text: string | null;
}

/** Имя файла без каталога и расширения `.md`/`.markdown` (фолбэк-заголовок, как у вкладок DP-15). */
export function basenameTitle(path?: string): string {
  if (!path) return '';
  return path.slice(path.lastIndexOf('/') + 1).replace(/\.(md|markdown)$/i, '');
}

/**
 * Знак буквицы ВЕДУЩЕГО абзаца (порт `dropcap.js` + EDFIX-4 графем-гард): первая буква/цифра, до
 * которой стоят ТОЛЬКО пунктуация/пробелы (`^[\p{P}\s]*`). Так «Все…»→'В', диалог `— Привет…`→'П'
 * (тире Pd + пробел Zs), кавычки `«слово»`→'С', `«2026»`→'2' (владелец просил «большую красную
 * ЦИФРУ»; `toUpperCase()` цифру не меняет). В data-cap уходит ТОЛЬКО сама буква (m[1]) — CSS-тюнинг
 * оптического зазора матчит по одиночному глифу (`[data-cap='В']`). Лид-СИМВОЛ (`\p{S}`: стрелка `←`,
 * эмодзи `🚀`) НЕ пропускается → '' — абзац дисквалифицируется (иначе CSS `::first-letter` раздувал
 * бы стрелку в гигантскую оранжевую буквицу — репорт владельца, Рескорринг.md), и поиск в превью
 * продолжается к следующему «обычному» абзацу. Пусто и когда букв/цифр нет вовсе (`!!! ???`).
 * Единственная продакшен-функция буквицы (MarkdownPreview) — одна семантика, без дублей.
 */
export function dropCapLetter(text: string): string {
  const m = (text || '').trim().match(/^[\p{P}\s]*([\p{L}\p{Nd}])/u);
  return m ? m[1].toUpperCase() : '';
}

/** Значение поля frontmatter по ключу (регистронезависимо). */
function fieldValues(fields: FmField[], key: string): string[] {
  const f = fields.find((x) => x.key.toLowerCase() === key);
  return f ? f.values : [];
}

/**
 * Считает данные масthead'а из исходника заметки. `notePath` — для фолбэк-заголовка по имени файла.
 * Ведущий ATX-H1 (`# …`, обязателен пробел — `#tag` не считается) обнуляется в `body`; его текст —
 * кандидат в заголовок (после frontmatter `title`). Setext-H1 (подчёркивание `===`) не обрабатываем —
 * как и в макете (`^#\s`).
 */
export function deriveMasthead(source: string, notePath?: string): Masthead {
  const fm = extractFrontmatter(source);
  const allFields = fm ? parseFrontmatterFields(fm.raw) : [];

  const fmTitle = fieldValues(allFields, 'title')[0]?.trim() ?? '';
  const tags = [...fieldValues(allFields, 'tags'), ...fieldValues(allFields, 'tag')]
    .map((v) => v.replace(/^#/, '').trim())
    .filter((v) => v !== '');

  // Eyebrow «тип · статус» (S2): берём ПЕРВЫЕ значения frontmatter `type`/`status`. Поля остаются в
  // Properties-таблице (там их место по макету) — eyebrow лишь дублирует их как kicker. Если нет ни
  // type, ни status → graceful fallback на теги (прежнее поведение). Пусто → eyebrow не рисуется.
  const typeVal = fieldValues(allFields, 'type')[0]?.trim() ?? '';
  const statusVal = fieldValues(allFields, 'status')[0]?.trim() ?? '';
  const typeStatus = [typeVal, statusVal].filter((v) => v !== '').join(' · ');
  const kicker = typeStatus || tags.join(' · ');

  // Обнуляем ведущий H1, сохраняя число строк (см. шапку файла).
  const lines = source.split('\n');
  const start = fm ? fm.endLine : 0; // 0-based индекс первой строки тела (endLine — 1-based номер закрывающего ---)
  let i = start;
  while (i < lines.length && lines[i].trim() === '') i++;
  let h1Text: string | null = null;
  let h1Line: number | null = null;
  // `(?:\s+#+)?\s*$` — снимаем закрывающую последовательность ATX (`# Заголовок #`, CommonMark: только
  // после пробела, поэтому `# foo#` без пробела сохраняется как «foo#»).
  const m = i < lines.length ? lines[i].match(/^#\s+(.+?)(?:\s+#+)?\s*$/) : null;
  if (m) {
    h1Text = m[1].trim();
    h1Line = i + 1; // 1-based
    lines[i] = ''; // обнуление (не удаление) — номера строк ниже не сдвигаются
  }

  // Для отображаемого заголовка снимаем inline-маркеры `*`/`` ` `` (как parseOutline в макете), чтобы из
  // `# Идея **важная**` не торчали звёздочки; `_` НЕ трогаем (часто snake_case в именах). Эмодзи из H1
  // тоже срезаем (`# 📅 …` daily → чистый редакционный вид, как заголовки тела). frontmatter title —
  // литерал, его не чистим. Для slug-id используем СЫРОЙ h1Text (slugify сам срежет пунктуацию).
  const titleFromH1 = h1Text ? stripHeadingEmoji(h1Text.replace(/[*`]/g, '')) : '';
  const title = fmTitle || titleFromH1 || basenameTitle(notePath);
  const fields = allFields.filter((f) => !MASTHEAD_FIELDS.has(f.key.toLowerCase()));

  return { title, tags, kicker, fields, body: lines.join('\n'), h1Line, h1Text };
}
