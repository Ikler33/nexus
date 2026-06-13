// Захват без трения (P4): ежедневная заметка-якорь. Открывает Journal/YYYY-MM-DD.md, создавая её
// из шаблона при первом обращении в день. Единая точка для команды note.daily и (позже) календаря.
import { tauriApi } from './tauri-api';
import { useVaultStore } from '../stores/vault';
import { useWorkspaceStore } from '../stores/workspace';

/** Папка дневников (решение плана: Journal/, не Daily/). */
const JOURNAL_DIR = 'Journal';

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
