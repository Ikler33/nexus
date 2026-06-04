import { lazy, Suspense, useState } from 'react';
import { BookOpen, Columns2, PenLine, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { Editor } from '../editor/Editor';
import { FileViewer } from '../editor/FileViewer';
import { isViewable } from '../../lib/file-kind';
import { BacklinksBar } from '../editor/BacklinksBar';
import styles from './GroupPane.module.css';

// Preview грузится лениво (react-markdown+micromark ~160KB) — нужен только при включении режима «Просмотр».
const MarkdownPreview = lazy(() =>
  import('../editor/MarkdownPreview').then((m) => ({ default: m.MarkdownPreview })),
);

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

/** Markdown-файл → доступен переключатель source/preview (#20). */
function isMarkdown(path: string): boolean {
  return /\.(md|markdown)$/i.test(path);
}

/** Одна группа (сплит): панель вкладок + редактор активной вкладки + backlinks-бар. */
export function GroupPane({ groupId }: { groupId: string }) {
  const { t } = useTranslation();
  const group = useWorkspaceStore((s) => s.groups.find((g) => g.id === groupId));
  const buffers = useWorkspaceStore((s) => s.buffers);
  const isActive = useWorkspaceStore((s) => s.activeGroupId === groupId);
  const setActiveTab = useWorkspaceStore((s) => s.setActiveTab);
  const setActiveGroup = useWorkspaceStore((s) => s.setActiveGroup);
  const closeTab = useWorkspaceStore((s) => s.closeTab);
  const splitRight = useWorkspaceStore((s) => s.splitRight);
  const updateBufferDoc = useWorkspaceStore((s) => s.updateBufferDoc);
  const saveBuffer = useWorkspaceStore((s) => s.saveBuffer);
  const openLink = useWorkspaceStore((s) => s.openLink);
  const [mode, setMode] = useState<'source' | 'preview'>('source');

  if (!group) return null;
  const active = group.activeTab ? buffers[group.activeTab] : null;
  const mdActive = active != null && !isViewable(active.path) && isMarkdown(active.path);

  return (
    <section
      className={styles.pane}
      data-active={isActive || undefined}
      onMouseDownCapture={() => {
        if (!isActive) setActiveGroup(groupId);
      }}
      aria-label={`Группа редактора ${groupId}`}
    >
      <div className={styles.tabbar}>
        <div className={styles.tabs} role="tablist">
          {group.tabs.map((path) => (
            <div
              key={path}
              role="tab"
              aria-selected={path === group.activeTab}
              data-active={path === group.activeTab || undefined}
              className={styles.tab}
              onClick={() => setActiveTab(groupId, path)}
              title={path}
            >
              <span className={styles.tabName}>{basename(path)}</span>
              {buffers[path]?.dirty && (
                <span className={styles.dot} aria-label={t('editor.unsaved')} />
              )}
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
            </div>
          ))}
        </div>
        {mdActive && (
          <button
            className={styles.split}
            onClick={() => setMode((m) => (m === 'source' ? 'preview' : 'source'))}
            title={mode === 'source' ? t('editor.preview') : t('editor.source')}
            aria-label={mode === 'source' ? t('editor.preview') : t('editor.source')}
            aria-pressed={mode === 'preview'}
          >
            {mode === 'source' ? (
              <BookOpen size={14} aria-hidden />
            ) : (
              <PenLine size={14} aria-hidden />
            )}
          </button>
        )}
        <button
          className={styles.split}
          onClick={() => splitRight()}
          title={t('editor.splitRight')}
          aria-label={t('editor.splitRight')}
        >
          <Columns2 size={14} aria-hidden />
        </button>
      </div>

      {active ? (
        <>
          <div className={styles.scroll}>
            {isViewable(active.path) ? (
              <FileViewer path={active.path} />
            ) : mdActive && mode === 'preview' ? (
              <Suspense fallback={null}>
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
                onOpenLink={(t) => void openLink(t)}
                getNotes={() => useVaultStore.getState().notes}
              />
            )}
          </div>
          {!isViewable(active.path) && <BacklinksBar path={active.path} />}
        </>
      ) : (
        <p className={styles.empty}>{t('editor.emptyGroup')}</p>
      )}
    </section>
  );
}
