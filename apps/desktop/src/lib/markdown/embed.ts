/**
 * Чистые помощники транклюзии `![[embed]]` (Live-Preview, режим чтения). Без React/IO — тестируются
 * отдельно. Сам рендер вставки — `NoteEmbed` (резолв заметки `resolveNote` + `readFile`, рекурсивный
 * `MarkdownPreview` с гард-циклом), детект блока-вставки — remark-плагин `remarkEmbeds`.
 */

/** Цель вставки, разобранная из внутренностей `![[ … ]]`. */
export interface EmbedTarget {
  /** Имя/путь заметки (до `#` и `|`). Пусто — вставка той же заметки (`![[#Heading]]`, пока не поддержано). */
  note: string;
  /** Якорь после `#` без решётки (заголовок), либо `^id` (блок-ссылка), либо null. */
  anchor: string | null;
}

/** Расширения, которые трактуем как картинку, — `![[pic.png]]` НЕ заметка (вставка картинки — отдельный
 *  слайс; сейчас такой паттерн пропускается в remark и падает в старое поведение `!`+вики-ссылка). */
const IMAGE_RE = /\.(png|jpe?g|gif|webp|svg|bmp|avif|ico)$/i;

/** `![[pic.png]]` — картинка-вставка (вне охвата текущего слайса транклюзии заметок). */
export function isImageTarget(note: string): boolean {
  return IMAGE_RE.test(note.trim());
}

/**
 * Разбирает внутренности `![[inner]]`: `note`, `note#Heading`, `note#^block`, `note|alias`,
 * `note#Heading|alias`, `folder/sub/note#H`. Алиас (`|…`) для вставки не нужен — отбрасываем.
 */
export function parseEmbedTarget(inner: string): EmbedTarget {
  const bar = inner.indexOf('|');
  const noAlias = bar >= 0 ? inner.slice(0, bar) : inner;
  const hash = noAlias.indexOf('#');
  const note = (hash >= 0 ? noAlias.slice(0, hash) : noAlias).trim();
  const anchorRaw = hash >= 0 ? noAlias.slice(hash + 1).trim() : '';
  return { note, anchor: anchorRaw.length > 0 ? anchorRaw : null };
}

/** Блок-ссылка `#^id` — пока не поддержана (Obsidian block refs). */
export function isBlockAnchor(anchor: string | null): boolean {
  return anchor != null && anchor.startsWith('^');
}

/** Параметры картинки-вставки `![[img.png|alt|300]]`: `alt`-текст и `width` (Obsidian-синтаксис —
 *  числовой сегмент = ширина в px, `ШxВ` → берём ширину; нечисловой = alt). Сегменты после первого `|`. */
export function parseImageParams(inner: string): { alt: string; width: number | null } {
  const segs = inner.split('|').slice(1); // [0] — имя файла, его берём из parseEmbedTarget.note
  let alt = '';
  let width: number | null = null;
  for (const seg of segs) {
    const t = seg.trim();
    const m = /^(\d+)(?:x\d+)?$/.exec(t); // ширина или ШxВ
    if (m) {
      // Числовой сегмент — ВСЕГДА спецификатор ширины (не alt). `0` невалиден → игнор (натуральный
      // размер), но `0` НЕ становится alt-текстом «0» (ревью: иначе 0px-картинка / мусорный alt).
      if (Number(m[1]) > 0) width = Number(m[1]);
    } else if (t) {
      alt = t;
    }
  }
  return { alt, width };
}

/** Нормализация текста заголовка для сравнения: срез закрывающих `#`, trim, lower. */
function normHeading(s: string): string {
  return s
    .replace(/\s+#+\s*$/, '') // закрытый ATX `## H ##`
    .trim()
    .toLowerCase();
}

/** Открытие/закрытие fenced-кода ```/~~~ (с возможным отступом) — внутри такого блока строка вида
 *  `# …` НЕ заголовок (ревью транклюзии: иначе ложная граница рубила бы секцию по `#` в коде). */
const FENCE_RE = /^\s*(`{3,}|~{3,})/;

/** Заголовки распознаём ТОЛЬКО ATX (`#…`) — как и весь аппарат заголовков приложения (outline.ts);
 *  setext (`===`/`---`) намеренно не поддержан (единообразие; не найден → честная заглушка). */
const ATX_RE = /^(#{1,6})\s+(.*)$/;

/**
 * Извлекает секцию под ATX-заголовком `heading`: строки от строки-заголовка (включительно) до следующего
 * заголовка того же или более высокого уровня (исключительно). Регистронезависимо, по тексту заголовка.
 * Заголовки ВНУТРИ fenced-кода (```/~~~) игнорируются (не граница). Возвращает null, если не найден.
 */
export function extractSection(body: string, heading: string): string | null {
  const want = normHeading(heading);
  if (want.length === 0) return null;
  const lines = body.split('\n');
  // Предпросчёт: какие строки внутри fenced-блока. Строка-открывашка ``` сама «снаружи», строки тела и
  // строка-закрывашка — «внутри» (для целей детекта заголовков это безразлично, маркеры не заголовки).
  const fenced: boolean[] = [];
  let inFence = false;
  for (let i = 0; i < lines.length; i++) {
    if (FENCE_RE.test(lines[i])) {
      fenced[i] = inFence;
      inFence = !inFence;
    } else {
      fenced[i] = inFence;
    }
  }
  let start = -1;
  let level = 0;
  for (let i = 0; i < lines.length; i++) {
    if (fenced[i]) continue;
    const m = ATX_RE.exec(lines[i]);
    if (m && normHeading(m[2]) === want) {
      start = i;
      level = m[1].length;
      break;
    }
  }
  if (start < 0) return null;
  let end = lines.length;
  for (let j = start + 1; j < lines.length; j++) {
    if (fenced[j]) continue;
    const m = ATX_RE.exec(lines[j]);
    if (m && m[1].length <= level) {
      end = j;
      break;
    }
  }
  return lines.slice(start, end).join('\n').trim();
}

/** Регэксп блока-вставки: вся (trim) строка/абзац — ровно `![[ … ]]` (без переводов строки внутри). */
export const EMBED_PARAGRAPH_RE = /^!\[\[([^\]\n]+)\]\]$/;
