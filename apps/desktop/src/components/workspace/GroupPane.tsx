import { lazy, Suspense, useEffect, useState } from 'react';
import { BookOpen, Clock, Columns2, FileText, PenLine, Plus, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { relTime } from '../../lib/time';
import { tauriApi } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { flush } from '../../stores/autosave';
import { Editor } from '../editor/Editor';
import { FileViewer } from '../editor/FileViewer';
import { isViewable } from '../../lib/file-kind';
import { BacklinksBar } from '../editor/BacklinksBar';
import styles from './GroupPane.module.css';

// Preview грузится лениво (react-markdown+micromark ~160KB) — нужен только при включении режима «Просмотр».
const MarkdownPreview = lazy(() =>
  import('../editor/MarkdownPreview').then((m) => ({ default: m.MarkdownPreview })),
);

/** MIME-тип DnD вкладок между панами — контракт макета `editor.jsx` (DP-3). */
const TAB_MIME = 'text/nexus-tab';

/** Имя вкладки: basename без `.md` (DP-15, макет: табы носят title заметки, не имя файла). */
function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1).replace(/\.md$/, '');
}

/** Markdown-файл → доступен переключатель source/preview (#20). */
function isMarkdown(path: string): boolean {
  return /\.(md|markdown)$/i.test(path);
}

/** Слов в документе (для doc-meta строки превью). */
function wordCount(doc: string): number {
  return doc.split(/\s+/).filter(Boolean).length;
}

/**
 * Одна группа (сплит): floating-вкладки (DnD между панами, DP-3) + редактор/превью активной
 * вкладки (режим — в сторе, ⌘E / mode-float пилюля) + backlinks-бар. В режиме чтения хром
 * вкладок упрощается (App `.reading`).
 */
export function GroupPane({ groupId }: { groupId: string }) {
  const { t, i18n } = useTranslation();
  const group = useWorkspaceStore((s) => s.groups.find((g) => g.id === groupId));
  const buffers = useWorkspaceStore((s) => s.buffers);
  const isActive = useWorkspaceStore((s) => s.activeGroupId === groupId);
  const mode = useWorkspaceStore((s) => s.modes[groupId] ?? 'source');
  const setActiveTab = useWorkspaceStore((s) => s.setActiveTab);
  const setActiveGroup = useWorkspaceStore((s) => s.setActiveGroup);
  const closeTab = useWorkspaceStore((s) => s.closeTab);
  const moveTab = useWorkspaceStore((s) => s.moveTab);
  const toggleMode = useWorkspaceStore((s) => s.toggleMode);
  const splitRight = useWorkspaceStore((s) => s.splitRight);
  const updateBufferDoc = useWorkspaceStore((s) => s.updateBufferDoc);
  const saveBuffer = useWorkspaceStore((s) => s.saveBuffer);
  const reloadFromDisk = useWorkspaceStore((s) => s.reloadFromDisk);
  const keepMine = useWorkspaceStore((s) => s.keepMine);
  const openLink = useWorkspaceStore((s) => s.openLink);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const createNote = useVaultStore((s) => s.createNote);
  const reading = useUIStore((s) => s.reading);
  const [dropTarget, setDropTarget] = useState(false);

  // DP-15 (макет editor.jsx): clock-чип doc-meta — mtime активного файла; перечитываем при смене
  // вкладки и переключении в превью (после правок mtime обновился сохранением).
  const activePath = group?.activeTab ?? null;
  const [mtime, setMtime] = useState<number | null>(null);
  useEffect(() => {
    if (!activePath) {
      setMtime(null);
      return;
    }
    let cancelled = false;
    tauriApi.vault
      .fileMtime(activePath)
      .then((v) => {
        if (!cancelled) setMtime(v);
      })
      .catch(() => {
        if (!cancelled) setMtime(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activePath, mode]);

  if (!group) return null;
  const active = group.activeTab ? buffers[group.activeTab] : null;
  const mdActive = active != null && !isViewable(active.path) && isMarkdown(active.path);

  return (
    <section
      className={`${styles.pane} ${dropTarget ? styles.dropTarget : ''}`}
      data-active={isActive || undefined}
      onMouseDownCapture={() => {
        if (!isActive) setActiveGroup(groupId);
      }}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(TAB_MIME)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        setDropTarget(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropTarget(false);
      }}
      onDrop={(e) => {
        setDropTarget(false);
        const raw = e.dataTransfer.getData(TAB_MIME);
        if (!raw) return;
        e.preventDefault();
        try {
          const { path, group: from } = JSON.parse(raw) as { path: string; group: string };
          moveTab(from, groupId, path);
        } catch {
          /* чужой/битый payload — игнор */
        }
      }}
      aria-label={`Группа редактора ${groupId}`}
    >
      <div className={styles.tabbar}>
        <div className={styles.tabs} role="tablist">
          {group.tabs.map((path) => {
            const dirty = Boolean(buffers[path]?.dirty);
            return (
              <div
                key={path}
                role="tab"
                aria-selected={path === group.activeTab}
                data-active={path === group.activeTab || undefined}
                className={styles.tab}
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData(TAB_MIME, JSON.stringify({ path, group: groupId }));
                  e.dataTransfer.effectAllowed = 'move';
                  e.currentTarget.classList.add(styles.dragging);
                }}
                onDragEnd={(e) => e.currentTarget.classList.remove(styles.dragging)}
                onClick={() => setActiveTab(groupId, path)}
                title={path}
              >
                <FileText size={13} className={styles.tabIco} aria-hidden />
                <span className={styles.tabName}>{basename(path)}</span>
                {dirty ? (
                  <span className={styles.dot} aria-label={t('editor.unsaved')} />
                ) : (
                  <button
                    className={styles.close}
                    onClick={(e) => {
                      e.stopPropagation();
                      closeTab(groupId, path);
                    }}
                    aria-label={t('editor.close', { name: basename(path) })}
                  >
                    <X size={12} aria-hidden />
                  </button>
                )}
              </div>
            );
          })}
          <button
            className={styles.tabAdd}
            onClick={() => void createNote().then((path) => openFile(path, groupId))}
            title={t('editor.newTab')}
            aria-label={t('editor.newTab')}
          >
            <Plus size={15} aria-hidden />
          </button>
        </div>
        <div className={styles.tabTools}>
          <button
            className={styles.split}
            onClick={() => splitRight()}
            title={t('editor.splitRight')}
            aria-label={t('editor.splitRight')}
          >
            <Columns2 size={14} aria-hidden />
          </button>
        </div>
      </div>

      {active ? (
        <>
          {/* SAFE-3 guard: файл изменился на диске, пока в буфере были несохранённые правки. */}
          {active.externalChange && (
            <div className={styles.externalBanner} role="alert">
              <span className={styles.externalMsg}>{t('editor.external.title')}</span>
              <div className={styles.externalActions}>
                <button
                  className={styles.externalBtn}
                  onClick={() => void keepMine(active.path)}
                >
                  {t('editor.external.keepMine')}
                </button>
                <button
                  className={`${styles.externalBtn} ${styles.externalPrimary}`}
                  onClick={() => void reloadFromDisk(active.path)}
                >
                  {t('editor.external.loadDisk')}
                </button>
                <button
                  className={styles.externalBtn}
                  disabled
                  title={t('editor.external.compareSoon')}
                >
                  {t('editor.external.compare')}
                </button>
              </div>
            </div>
          )}
          <div className={styles.scroll}>
            {/* Mode-float (DP-3): плавающая пилюля Edit/Preview — иконка показывает ДЕЙСТВИЕ. */}
            {mdActive && !reading && (
              <button
                className={styles.modeFloat}
                onClick={() => toggleMode(groupId)}
                title={mode === 'source' ? t('editor.preview') : t('editor.source')}
                aria-label={mode === 'source' ? t('editor.preview') : t('editor.source')}
                aria-pressed={mode === 'preview'}
              >
                <span key={mode} className={styles.modeIco}>
                  {mode === 'source' ? (
                    <BookOpen size={16} aria-hidden />
                  ) : (
                    <PenLine size={16} aria-hidden />
                  )}
                </span>
              </button>
            )}
            {isViewable(active.path) ? (
              <FileViewer path={active.path} />
            ) : mdActive && (mode === 'preview' || reading) ? (
              <Suspense fallback={null}>
                <div className={styles.docMeta}>
                  {mtime != null && (
                    <>
                      <span>
                        <Clock size={13} aria-hidden /> {relTime(mtime, i18n.language)}
                      </span>
                      <span>·</span>
                    </>
                  )}
                  <span>{t('editor.metaWords', { count: wordCount(active.doc) })}</span>
                  <span>·</span>
                  <span>
                    {t('editor.metaReading', {
                      count: Math.max(1, Math.round(wordCount(active.doc) / 200)),
                    })}
                  </span>
                </div>
                <MarkdownPreview source={active.doc} onOpenLink={(target) => void openLink(target)} />
              </Suspense>
            ) : (
              <Editor
                key={groupId}
                path={active.path}
                initialDoc={active.doc}
                onChange={(doc) => updateBufferDoc(active.path, doc)}
                onSave={(doc) => {
                  updateBufferDoc(active.path, doc);
                  void saveBuffer(active.path);
                }}
                onBlur={() => void flush(active.path)}
                onOpenLink={(t) => void openLink(t)}
                fetchNotes={(q) => tauriApi.vault.listNotes(q, 50)}
              />
            )}
          </div>
          {!isViewable(active.path) && !reading && <BacklinksBar path={active.path} />}
        </>
      ) : (
        <p className={styles.empty}>{t('editor.emptyGroup')}</p>
      )}
    </section>
  );
}
