// Шаблоны заметок (CAP-3, P4-захват): заметки из папки Templates/ как заготовки. «Новая из шаблона»
// подставляет плейсхолдеры и создаёт заметку в папке активной (или корне). Снимает трение старта
// структурированной заметки (встреча, ретро, чек-лист) — продолжение CAP-1 (daily) / CAP-2 (inbox).
import { tauriApi } from './tauri-api';
import { useVaultStore } from '../stores/vault';
import { activePath, useWorkspaceStore } from '../stores/workspace';

/** Папка шаблонов (решение плана: Templates/). */
export const TEMPLATES_DIR = 'Templates';

function pad(n: number): string {
  return String(n).padStart(2, '0');
}
function dateStamp(now: Date): string {
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}
function timeStamp(now: Date): string {
  return `${pad(now.getHours())}:${pad(now.getMinutes())}`;
}

/** Каталог пути (без завершающего имени); '' для корня. */
function parentDir(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}

/** Имя шаблона для показа/имени заметки: basename без .md. */
export function templateTitle(templatePath: string): string {
  const base = templatePath.slice(templatePath.lastIndexOf('/') + 1);
  return base.endsWith('.md') ? base.slice(0, -3) : base;
}

/** Список шаблонов (только .md из Templates/), относительные пути. Нет папки → пусто. */
export async function listTemplates(): Promise<string[]> {
  const entries = await tauriApi.vault.listDir(TEMPLATES_DIR).catch(() => []);
  return entries.filter((e) => !e.isDir && e.name.endsWith('.md')).map((e) => e.path);
}

/** Подстановка плейсхолдеров шаблона: {{date}} {{time}} {{datetime}} {{title}} (пробелы внутри ок). */
export function applyPlaceholders(content: string, title: string, now = new Date()): string {
  const date = dateStamp(now);
  const time = timeStamp(now);
  // Функция-replacer (не строка): значение подставляется буквально, без интерпретации $&/$1/$$
  // (имя шаблона может содержать `$`).
  return content
    .replace(/\{\{\s*date\s*\}\}/g, () => date)
    .replace(/\{\{\s*time\s*\}\}/g, () => time)
    .replace(/\{\{\s*datetime\s*\}\}/g, () => `${date} ${time}`)
    .replace(/\{\{\s*title\s*\}\}/g, () => title);
}

/**
 * Создать заметку из шаблона: читаем тело, подставляем плейсхолдеры (title = имя шаблона), создаём в
 * папке активной заметки (или корне) с этим именем (createNote разрулит коллизии имён), открываем.
 */
export async function newNoteFromTemplate(templatePath: string, now = new Date()): Promise<string> {
  const meta = await tauriApi.vault.readFileMeta(templatePath);
  const base = templateTitle(templatePath);
  const active = activePath(useWorkspaceStore.getState());
  const dir = active ? parentDir(active) : '';
  const content = applyPlaceholders(meta.content, base, now);
  // Гарантируем актуальный список детей целевой папки ДО createNote: иначе при нераскрытой папке
  // его дедуп коллизий (по кэшу childrenByPath) пуст → atomic-write молча перезатёр бы одноимённый
  // файл (риск потери данных, класс SAFE-1). refreshDir заполняет кэш реальным содержимым.
  const vault = useVaultStore.getState();
  await vault.refreshDir(dir);
  const path = await vault.createNote(dir, { baseName: base, content });
  await useWorkspaceStore.getState().openFile(path);
  return path;
}
