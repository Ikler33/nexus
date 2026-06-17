/**
 * Разбор ведущего frontmatter `---\n…\n---` для Properties-таблицы режима чтения (FRONTMATTER-1).
 * НЕ полный YAML — лёгкий построчный разбор `k: v` / `k: [a, b]` / `k:`+`  - item` (как делает
 * тупой edge-stripper бэкенда). Вложенные карты/мультистроки — как скаляр. Используется И для
 * подавления frontmatter из markdown-рендера ([[remarkFrontmatter]] по `endLine`, без сдвига строк
 * тела — EDIT-5/EDIT-7), И для самой таблицы.
 */

export interface Frontmatter {
  /** Содержимое между фенсами `---` (без самих фенсов). */
  raw: string;
  /** 1-based номер строки ЗАКРЫВАЮЩЕГО `---` (всё тело — строки > endLine). */
  endLine: number;
}

export interface FmField {
  key: string;
  values: string[];
}

/** Находит ведущий блок frontmatter. null — нет блока ИЛИ он не закрыт (не угадываем). */
export function extractFrontmatter(src: string): Frontmatter | null {
  if (!src.startsWith('---\n') && !src.startsWith('---\r\n')) return null;
  const lines = src.split('\n');
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].replace(/\r$/, '') === '---') {
      return { raw: lines.slice(1, i).join('\n'), endLine: i + 1 };
    }
  }
  return null; // незакрытый блок — показываем как есть, таблицу не строим
}

function stripQuotes(s: string): string {
  if (s.length >= 2 && ((s[0] === '"' && s.endsWith('"')) || (s[0] === "'" && s.endsWith("'")))) {
    return s.slice(1, -1);
  }
  return s;
}

/** Лёгкий разбор frontmatter в упорядоченные пары ключ→значения (см. шапку файла). */
export function parseFrontmatterFields(raw: string): FmField[] {
  const out: FmField[] = [];
  let current: FmField | null = null;
  for (const rawLine of raw.split('\n')) {
    const line = rawLine.replace(/\r$/, '');
    if (line.trim() === '') continue;
    // Элемент блок-списка (`  - item`) — только с отступом и при активном ключе.
    const item = /^\s/.test(line) ? line.match(/^\s*-\s+(.*)$/) : null;
    if (item && current) {
      current.values.push(stripQuotes(item[1].trim()));
      continue;
    }
    // `key: …` — ключ начинается не с пробела/двоеточия.
    const kv = line.match(/^([^:\s][^:]*?):\s*(.*)$/);
    if (kv) {
      const key = kv[1].trim();
      const val = kv[2].trim();
      current = { key, values: [] };
      out.push(current);
      if (val === '') {
        // block-list/значение придёт ниже
      } else if (val.startsWith('[') && val.endsWith(']')) {
        current.values = val
          .slice(1, -1)
          .split(',')
          .map((s) => stripQuotes(s.trim()))
          .filter((s) => s !== '');
      } else {
        current.values = [stripQuotes(val)];
      }
    }
    // строки без `:` и не блок-элементы (например многострочный скаляр) — игнорируем
  }
  return out;
}
