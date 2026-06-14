// Захват без трения (P4): ежедневная заметка-якорь. Открывает Journal/YYYY-MM-DD.md, создавая её
// из шаблона при первом обращении в день. Единая точка для команды note.daily и (позже) календаря.
import { tauriApi } from './tauri-api';
import { useVaultStore } from '../stores/vault';
import { useWorkspaceStore } from '../stores/workspace';

/** Папка дневников (решение плана: Journal/, не Daily/). */
export const JOURNAL_DIR = 'Journal';
/** Файл быстрого захвата мыслей (quick-capture); вход для Inbox-triage (INBOX-1). */
export const INBOX = 'Inbox.md';

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

/** Дата в формате YYYY-MM-DD по локальному времени. */
export function dateStamp(now: Date): string {
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** Путь дневной заметки на дату: Journal/YYYY-MM-DD.md. */
export function dailyNotePath(now: Date): string {
  return `${JOURNAL_DIR}/${dateStamp(now)}.md`;
}

/** Встроенный шаблон дневной заметки (Templates/Daily.md — отдельная фича позже). */
export function defaultDailyContent(stamp: string): string {
  return `# ${stamp}\n\n`;
}

/**
 * Открыть дневную заметку текущего дня; создать из шаблона, если её ещё нет.
 * После создания обновляет дерево (папка Journal/ + сама заметка) и открывает её в редакторе.
 */
export async function openOrCreateDaily(now = new Date()): Promise<string> {
  const path = dailyNotePath(now);
  // Существование без чтения тела: file_hash → null, если файла нет.
  const exists = (await tauriApi.vault.fileHash(path).catch(() => null)) !== null;
  if (!exists) {
    await tauriApi.vault.writeFile(path, defaultDailyContent(dateStamp(now)), true);
    // Папка Journal/ могла быть новой → обновляем корень, затем её содержимое (раскрыв её).
    const vault = useVaultStore.getState();
    await vault.refreshDir('');
    await vault.refreshDir(JOURNAL_DIR, true);
  }
  await useWorkspaceStore.getState().openFile(path);
  return path;
}

/** Время HH:MM по локальному времени. */
function timeStamp(now: Date): string {
  return `${pad(now.getHours())}:${pad(now.getMinutes())}`;
}

/**
 * Quick-capture: дозаписывает мысль строкой «- HH:MM текст» в конец Inbox.md (создаёт «# Inbox»,
 * если файла нет) — мгновенный захват без открытия файла. Запись атомарна (SAFE-1); если Inbox
 * открыт в редакторе, watcher-reload/guard (SAFE-3) подхватят изменение.
 */
export async function appendCapture(text: string, now = new Date()): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  const had = (await tauriApi.vault.fileHash(INBOX).catch(() => null)) !== null;
  const existing = had ? await tauriApi.vault.readFile(INBOX) : '# Inbox\n';
  const sep = existing.endsWith('\n') || existing === '' ? '' : '\n';
  const next = `${existing}${sep}- ${timeStamp(now)} ${trimmed}\n`;
  await tauriApi.vault.writeFile(INBOX, next, true);
  if (!had) await useVaultStore.getState().refreshDir('');
}
