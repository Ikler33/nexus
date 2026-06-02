import type { FileEntry, NoteRef, VaultInfo } from '../tauri-api';

/**
 * Фейковый vault для браузерного превью и тестов (DESIGN §0): фронт работает на тех же
 * контрактах `tauriApi`, что и реальный бэкенд, не дожидаясь Rust.
 */

function basename(path: string): string {
  const i = path.lastIndexOf('/');
  return i >= 0 ? path.slice(i + 1) : path;
}

function dir(path: string, hasChildren: boolean): FileEntry {
  return { name: basename(path), path, isDir: true, hasChildren, sizeBytes: 0 };
}

function file(path: string, sizeBytes: number): FileEntry {
  return { name: basename(path), path, isDir: false, hasChildren: false, sizeBytes };
}

/** Каталог → его непосредственные дети (ленивая модель, как у Rust `list_dir`). */
const TREE: Record<string, FileEntry[]> = {
  '': [
    dir('Projects', true),
    dir('Notes', true),
    dir('Empty', false),
    file('README.md', 1200),
    file('Inbox.md', 340),
  ],
  Projects: [dir('Projects/Alpha', true), file('Projects/Roadmap.md', 800)],
  'Projects/Alpha': [
    file('Projects/Alpha/Spec.md', 2400),
    file('Projects/Alpha/Notes.md', 560),
  ],
  Notes: [file('Notes/Idea.md', 210), file('Notes/Meeting.md', 980)],
  Empty: [],
};

/** Содержимое мок-файлов (правки в превью держим в этой мапе). */
const CONTENT: Record<string, string> = {
  'README.md': '# Mock Vault\n\nДемо-хранилище для превью без Tauri.\n\nСвязи: [[Inbox]] и [[Projects/Roadmap]].\nТеги: #demo #docs\n',
  'Inbox.md': '# Inbox\n\nБыстрые заметки. Ссылка на [[README]].\n',
  'Projects/Roadmap.md': '# Roadmap\n\nПлан проекта Alpha. См. [[Projects/Alpha/Spec]].\n#planning\n',
  'Projects/Alpha/Spec.md': '# Alpha Spec\n\nСпецификация. Обратно к [[Projects/Roadmap]].\n',
  'Projects/Alpha/Notes.md': '# Alpha Notes\n\nЗаметки по Alpha. #alpha\n',
  'Notes/Idea.md': '# Idea\n\nИдея с тегом #idea и ссылкой [[Notes/Meeting]].\n',
  'Notes/Meeting.md': '# Meeting\n\nПротокол встречи.\n',
};

export async function openVault(path: string): Promise<VaultInfo> {
  return { root: path || '/mock/vault', name: 'Mock Vault' };
}

export async function listDir(dirPath: string): Promise<FileEntry[]> {
  return TREE[dirPath] ?? [];
}

export async function readFile(path: string): Promise<string> {
  return CONTENT[path] ?? `# ${basename(path)}\n\n(пустой мок-файл)\n`;
}

export async function writeFile(path: string, content: string): Promise<void> {
  CONTENT[path] = content;
}

export async function listNotes(): Promise<NoteRef[]> {
  const files = Object.values(TREE)
    .flat()
    .filter((e) => !e.isDir)
    .map((e) => ({ path: e.path, title: null }));
  // Уникализируем по пути.
  const seen = new Set<string>();
  return files.filter((n) => (seen.has(n.path) ? false : (seen.add(n.path), true)));
}
