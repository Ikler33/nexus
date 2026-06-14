/** Одна захваченная строка Inbox (INBOX-1): 1-based номер строки, время HH:MM, текст. */
export interface InboxItem {
  line: number;
  time: string;
  text: string;
}

/** Формат строк quick-capture (CAP-2): `- HH:MM текст`. Завершающий `\s*$` вбирает CRLF `\r`. */
const INBOX_LINE_RE = /^\s*-\s+(\d{2}:\d{2})\s+(.+?)\s*$/;

/**
 * Извлекает захваченные мысли из Inbox.md (INBOX-1, GTD-разбор). Парсит строки вида `- HH:MM текст`
 * (как пишет appendCapture, CAP-2); заголовок `# Inbox` и прочее игнорируются. Чистая, тестируемая.
 */
export function parseInbox(doc: string): InboxItem[] {
  const out: InboxItem[] = [];
  const lines = doc.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const m = INBOX_LINE_RE.exec(lines[i]);
    if (m) out.push({ line: i + 1, time: m[1], text: m[2] });
  }
  return out;
}

/** Удаляет 1-based строку `line` из текста (после переноса захвата в задачу/заметку или сброса).
 *  Возвращает новый текст или `null`, если строка вне диапазона (дрейф между загрузкой и кликом). */
export function removeLine(doc: string, line: number): string | null {
  const lines = doc.split('\n');
  if (line < 1 || line > lines.length) return null;
  lines.splice(line - 1, 1);
  return lines.join('\n');
}
