import { open as openDialog } from '@tauri-apps/plugin-dialog';
import * as mockTags from '../../mock/tags';
import * as mockVault from '../../mock/vault';
import { bridge, isTauri, subscribe } from '../bridge';
import type { FileEntry, NoteRef, TagCount, VaultInfo } from './types';

/**
 * Vault-домен (F-2a): файловые операции, дерево, open vault, версии-снапшоты, вложения,
 * watcher-подписки. Все request/response-вызовы — через `bridge` (Tauri ↔ мок `lib/mock/*`);
 * потребители ходят сюда по-прежнему через `tauriApi.vault`/`tauriApi.attachments`/
 * `tauriApi.events` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */

export const vault = {
  /** Открывает vault по абсолютному пути; в браузере — мок. */
  openVault: (path: string) =>
    bridge<VaultInfo>('open_vault', { path }, () => mockVault.openVault(path)),

  /** Ленивый листинг каталога (`dirPath` относительный; '' = корень). */
  listDir: (dirPath: string) =>
    bridge<FileEntry[]>('list_dir', { dirPath }, () => mockVault.listDir(dirPath)),

  /** Читает содержимое файла vault. */
  readFile: (path: string) => bridge<string>('read_file', { path }, () => mockVault.readFile(path)),

  /** Читает контент + хеш (`baseHash` буфера для детекта внешних изменений, SAFE-3). */
  readFileMeta: (path: string) =>
    bridge<{ content: string; hash: string }>('read_file_meta', { path }, () =>
      mockVault.readFileMeta(path),
    ),

  /** Хеш файла на диске без чтения содержимого (дешёвая сверка `baseHash`); `null`, если файла нет. */
  fileHash: (path: string) =>
    bridge<string | null>('file_hash', { path }, () => mockVault.fileHash(path)),

  /** Пишет содержимое файла vault. Возвращает хеш записанного (фронт обновляет `baseHash`).
   *  `manual` (Ctrl-S/палитра vs автосейв) управляет троттлом снапшота истории (SAFE-5). */
  writeFile: (path: string, content: string, manual = false) =>
    bridge<string>('write_file', { path, content, manual }, () =>
      mockVault.writeFile(path, content),
    ),

  /** BOARD-1: правит ОДИН плоский frontmatter-ключ заметки (статус задачи/project/priority/Properties),
   *  сохраняя остальной YAML/тело. Возвращает новый контент+хеш — фронт кладёт хеш в `baseHash`
   *  (анти-эхо SAFE-3) и обновляет буфер, если заметка открыта. Незакрытый `---` → ошибка. */
  setFrontmatterField: (path: string, key: string, value: string) =>
    bridge<{ content: string; hash: string }>('set_frontmatter_field', { path, key, value }, () =>
      mockVault.setFrontmatterField(path, key, value),
    ),

  /** Удаляет заметку/каталог в корзину `.nexus/.trash/` (CURATE-1) — обратимо. */
  deletePath: (path: string) =>
    bridge<void>('delete_path', { path }, () => mockVault.deletePath(path)),

  /** Переименовывает/перемещает путь `from`→`to` (CURATE-2); беклинки сохраняются по id. */
  renamePath: (from: string, to: string) =>
    bridge<void>('rename_path', { from, to }, () => mockVault.renamePath(from, to)),

  /** Версии-снапшоты заметки (SAFE-5/6): время + размер, новейший первым. */
  listVersions: (path: string) =>
    bridge<{ ts: number; size: number }[]>('list_versions', { path }, () =>
      mockVault.listVersions(path),
    ),

  /** Содержимое версии-снапшота по `ts` (diff/восстановление, SAFE-6). */
  readVersion: (path: string, ts: number) =>
    bridge<string>('read_version', { path, ts }, () => mockVault.readVersion(path, ts)),

  /** Заметки vault (path + title) для автокомплита `[[wikilink]]`. #22: опциональный
   * подстрочный `query`-фильтр + `limit` — топ-N вместо всего vault (префиксы ранжируются выше). */
  listNotes: (query?: string, limit?: number) =>
    bridge<NoteRef[]>('list_notes', { query, limit }, () => mockVault.listNotes(query, limit)),

  /** Резолвит цель `[[wikilink]]` в путь файла — бэкенд-семантика индексатора (путь / +`.md` /
   * basename, затем алиас V4.1); #22: клик по ссылке без полного списка заметок на фронте. */
  resolveNote: (target: string) =>
    bridge<string | null>('resolve_note', { target }, () => mockVault.resolveNote(target)),

  /** Теги vault с количеством заметок — панель «Теги» сайдбара (DP-2). */
  listTags: (): Promise<TagCount[]> =>
    bridge<TagCount[]>('list_tags', undefined, () => mockTags.listTags()),

  /** Заметки с ТОЧНЫМ тегом (клик по тегу → exact-фильтр, не зашумлённый substring-поиск). */
  notesByTag: (tag: string): Promise<NoteRef[]> =>
    bridge<NoteRef[]>('notes_by_tag', { tag }, () => mockTags.notesByTag(tag)),

  /** Ручной реиндекс vault (quick action «Переиндексировать», макет home.jsx): фоновый
   * полный обход; по завершении бэкенд шлёт `vault:changed`. В браузере — no-op. */
  rescan: (): Promise<void> => bridge<void>('rescan_vault', undefined, () => mockVault.rescan()),

  /** Число живых заметок индекса — статусбар «Проиндексировано · N» (DP-14). Мок — 847,
   * как в демо-данных Home (`lib/mock/home.ts`). */
  notesCount: (): Promise<number> =>
    bridge<number>('notes_count', undefined, () => mockVault.notesCount()),

  /** Unix-mtime файла (сек) — clock-чип doc-meta превью (DP-15). Мок — «3 ч назад». */
  fileMtime: (path: string): Promise<number> =>
    bridge<number>('file_mtime', { path }, () => mockVault.fileMtime()),

  /** Системный выбор папки vault (нативный диалог Tauri). Вне Tauri — `null`.
   *  Bridge-исключение (см. `../bridge.ts`): путь с OS-диалогом, мокать нечего. */
  pickDirectory: async (): Promise<string | null> => {
    if (!isTauri()) return null;
    const picked = await openDialog({ directory: true, multiple: false });
    return typeof picked === 'string' ? picked : null;
  },
};

/** Вложения vault (IMG-1/IMG-EMBED): запись/чтение/резолв картинок — файловые операции домена. */
export const attachments = {
  /** Пишет картинку в `attachments/<name>` из base64 (IMG-1). Возвращает относительный путь `![](…)`. */
  write: (name: string, dataBase64: string) =>
    bridge<string>('write_attachment', { name, dataBase64 }, () => mockVault.writeAttachment(name)),

  /** Читает вложение-картинку как `data:`-URL для превью (IMG-1). */
  read: (path: string) =>
    bridge<string>('read_attachment', { path }, () => mockVault.readAttachment(path)),

  /** Резолвит цель `![[pic.png]]` → относительный путь vault (basename-обход) или null (IMG-EMBED). */
  resolve: (name: string) =>
    bridge<string | null>('resolve_attachment', { name }, () => mockVault.resolveAttachment(name)),
};

/** Watcher-подписки vault-домена (реиндекс/изменения файлов/прогресс скана). Вне Tauri — no-op
 *  (мок-бэкенд ФС не вотчит и не индексирует). Каждая возвращает функцию отписки. */
export const vaultEvents = {
  /**
   * Подписка на событие «индекс vault обновлён» (backend `emit("vault:changed")` после реиндекса —
   * ADR-007 S8 event-канал). Возвращает функцию отписки. Вне Tauri — no-op (мок-бэкенд не индексирует).
   */
  onVaultChanged: (cb: () => void): Promise<() => void> => subscribe('vault:changed', () => cb()),

  /**
   * Подписка на «конкретный файл на диске изменился» (`vault:file-changed {path, hash}`, SAFE-3).
   * Фронт сверяет hash с `Buffer.baseHash`: эхо своего сейва → игнор; чистый буфер → тихий reload;
   * грязный → баннер guard'а. Вне Tauri — no-op (мок-бэкенд не вотчит ФС).
   */
  onFileChanged: (cb: (p: { path: string; hash: string }) => void): Promise<() => void> =>
    subscribe<{ path: string; hash: string }>('vault:file-changed', cb),

  /**
   * Подписка на прогресс полного скана индексатора (`vault:index-progress`, {done,total}) —
   * статусбар «Индексация N/M» (макет app.jsx). Старт (0,total) → шаги → финиш (total,total).
   * Вне Tauri — no-op (мок не сканирует).
   */
  onIndexProgress: (cb: (p: { done: number; total: number }) => void): Promise<() => void> =>
    subscribe<{ done: number; total: number }>('vault:index-progress', cb),
};
