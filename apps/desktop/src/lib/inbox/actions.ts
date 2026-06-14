import {
  dailyNotePath,
  dateStamp,
  defaultDailyContent,
  INBOX,
  JOURNAL_DIR,
} from '../daily';
import { tauriApi } from '../tauri-api';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { type InboxItem, parseInbox, removeLine } from './parse';

/**
 * Действия GTD-разбора Inbox (INBOX-1). Каждое: проверяет, что строка не сдвинулась (drift-guard по
 * тексту), применяет эффект (в задачу/в заметку — НИЧЕГО для «удалить»), затем вырезает строку из
 * Inbox.md. Эффект ДО удаления: потерять захват хуже, чем оставить дубль. Все операции буфер-aware —
 * если Inbox/daily открыты в редакторе, пишем через updateBufferDoc (как EDIT-5/TASK-1), иначе на диск.
 * Возврат false → строка изменилась/ошибка → UI перезагрузит список.
 */

/** Чтение Inbox: открытый буфер — источник правды, иначе диск. */
async function readInbox(): Promise<string> {
  const buf = useWorkspaceStore.getState().buffers[INBOX];
  return buf ? buf.doc : tauriApi.vault.readFile(INBOX);
}

/** Захваченные строки Inbox для панели (буфер-aware). Нет файла → пусто. */
export async function loadInbox(): Promise<InboxItem[]> {
  try {
    return parseInbox(await readInbox());
  } catch {
    return []; // Inbox.md ещё нет — пустой список
  }
}

/** Запись Inbox: буфер-aware. */
async function writeInbox(next: string): Promise<void> {
  const ws = useWorkspaceStore.getState();
  if (ws.buffers[INBOX]) ws.updateBufferDoc(INBOX, next);
  else await tauriApi.vault.writeFile(INBOX, next, false);
}

/** Дозаписывает строку в конец текста (с переводом строки). */
function joinAppend(doc: string, line: string): string {
  const sep = doc.endsWith('\n') || doc === '' ? '' : '\n';
  return `${doc}${sep}${line}\n`;
}

/** Дозаписывает строку в сегодняшний дневник (создаёт из шаблона при необходимости); буфер-aware. */
async function appendToDaily(line: string): Promise<void> {
  const path = dailyNotePath(new Date());
  const ws = useWorkspaceStore.getState();
  const buf = ws.buffers[path];
  if (buf) {
    ws.updateBufferDoc(path, joinAppend(buf.doc, line));
    return;
  }
  const exists = (await tauriApi.vault.fileHash(path).catch(() => null)) !== null;
  const existing = exists
    ? await tauriApi.vault.readFile(path)
    : defaultDailyContent(dateStamp(new Date()));
  await tauriApi.vault.writeFile(path, joinAppend(existing, line), false);
  if (!exists) {
    const v = useVaultStore.getState();
    await v.refreshDir('');
    await v.refreshDir(JOURNAL_DIR, true);
  }
}

/** Безопасное имя файла из текста захвата (срез спецсимволов пути/markdown, лимит длины). */
function noteSlug(text: string): string {
  return text
    .replace(/[\\/:*?"<>|#[\]]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 60);
}

/** Уникальный путь `<slug>.md` (анти-overwrite суффиксом « N»). */
async function uniquePath(slug: string): Promise<string> {
  const exists = async (p: string) =>
    (await tauriApi.vault.fileHash(p).catch(() => null)) !== null;
  if (!(await exists(`${slug}.md`))) return `${slug}.md`;
  for (let i = 2; i < 100; i++) {
    const p = `${slug} ${i}.md`;
    if (!(await exists(p))) return p;
  }
  return `${slug} ${Date.now()}.md`;
}

/** Общий путь: drift-guard по тексту → эффект → вырезание строки из Inbox. */
async function consume(item: InboxItem, effect?: () => Promise<void>): Promise<boolean> {
  try {
    const doc = await readInbox();
    const cur = parseInbox(doc).find((x) => x.line === item.line);
    if (!cur || cur.text !== item.text) return false; // строка сдвинулась/изменилась
    if (effect) await effect();
    const next = removeLine(doc, item.line);
    if (next == null) return false;
    await writeInbox(next);
    return true;
  } catch {
    return false;
  }
}

/** «В задачу»: добавляет `- [ ] текст` в сегодняшний дневник (появится в TASK-дашборде). */
export function toTask(item: InboxItem): Promise<boolean> {
  return consume(item, () => appendToDaily(`- [ ] ${item.text}`));
}

/** «В заметку»: создаёт новую заметку из текста захвата и открывает её. */
export function toNote(item: InboxItem): Promise<boolean> {
  return consume(item, async () => {
    const title = item.text.trim().slice(0, 60);
    const path = await uniquePath(noteSlug(title) || 'note');
    await tauriApi.vault.writeFile(path, `# ${title}\n\n`, true);
    await useVaultStore.getState().refreshDir('');
    await useWorkspaceStore.getState().openFile(path);
  });
}

/** «Удалить»: просто вырезает строку из Inbox (сброс захвата). */
export function discard(item: InboxItem): Promise<boolean> {
  return consume(item);
}
