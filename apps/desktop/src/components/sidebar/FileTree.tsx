import { useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { ChevronRight, File as FileIcon, Folder, Star } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStarredStore } from '../../stores/starred';
import { flattenVisible, useVaultStore } from '../../stores/vault';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import styles from './FileTree.module.css';

const ROW_HEIGHT = 28;

/**
 * Виртуализированное файловое дерево (DESIGN §4 FileTree): рендерит только видимую
 * область (flatten раскрытых ветвей → `@tanstack/react-virtual`), дети грузятся лениво.
 * Клавиатура: ↑/↓ — навигация, →/← — раскрыть/свернуть, Enter/Space — активировать
 * (паттерн `aria-activedescendant`, дружит с виртуализацией). Ориентир: AC-PERF-7.
 */
export function FileTree() {
  const childrenByPath = useVaultStore((s) => s.childrenByPath);
  const expanded = useVaultStore((s) => s.expanded);
  const loading = useVaultStore((s) => s.loading);
  const selectedPath = useWorkspaceStore(activePath);
  const toggleDir = useVaultStore((s) => s.toggleDir);
  const createNote = useVaultStore((s) => s.createNote);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const starredPaths = useStarredStore((s) => s.paths);
  const toggleStar = useStarredStore((s) => s.toggle);
  const { t } = useTranslation();

  const nodes = useMemo(
    () => flattenVisible(childrenByPath, expanded, loading),
    [childrenByPath, expanded, loading],
  );

  const [active, setActive] = useState(0);
  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: nodes.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    // Стартовый размер, чтобы строки рендерились до измерения (важно для тестов в jsdom).
    initialRect: { width: 280, height: 600 },
  });

  if (nodes.length === 0) {
    return (
      <div className={styles.empty} role="note">
        <p className={styles.emptyText}>{t('tree.empty')}</p>
        <button
          type="button"
          className={styles.newNoteBtn}
          onClick={() =>
            void createNote('', { baseName: 'Welcome', content: t('tree.welcomeBody') }).then(
              (path) => openFile(path),
            )
          }
        >
          {t('tree.newNote')}
        </button>
      </div>
    );
  }

  const activate = (index: number) => {
    const node = nodes[index];
    if (!node) return;
    if (node.entry.isDir) void toggleDir(node.entry.path);
    else void openFile(node.entry.path);
  };

  const move = (index: number) => {
    const clamped = Math.max(0, Math.min(nodes.length - 1, index));
    setActive(clamped);
    virtualizer.scrollToIndex(clamped);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const node = nodes[active];
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        move(active + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        move(active - 1);
        break;
      case 'ArrowRight':
        if (node?.entry.isDir && !node.expanded) {
          e.preventDefault();
          void toggleDir(node.entry.path);
        }
        break;
      case 'ArrowLeft':
        if (node?.entry.isDir && node.expanded) {
          e.preventDefault();
          void toggleDir(node.entry.path);
        }
        break;
      case 'Enter':
      case ' ':
        e.preventDefault();
        activate(active);
        break;
    }
  };

  return (
    <div
      ref={parentRef}
      className={styles.tree}
      role="tree"
      aria-label={t('tree.label')}
      tabIndex={0}
      aria-activedescendant={`treeitem-${active}`}
      onKeyDown={onKeyDown}
    >
      <div style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative' }}>
        {virtualizer.getVirtualItems().map((vItem) => {
          const node = nodes[vItem.index];
          const { entry } = node;
          const isActive = vItem.index === active;
          const isSelected = selectedPath === entry.path;
          return (
            <div
              key={entry.path}
              id={`treeitem-${vItem.index}`}
              role="treeitem"
              aria-level={node.depth + 1}
              aria-expanded={entry.isDir ? node.expanded : undefined}
              aria-selected={isSelected}
              className={styles.row}
              data-active={isActive || undefined}
              data-selected={isSelected || undefined}
              style={{
                position: 'absolute',
                top: 0,
                left: 0,
                right: 0,
                height: `${vItem.size}px`,
                transform: `translateY(${vItem.start}px)`,
                paddingLeft: `${node.depth * 14 + 8}px`,
              }}
              onClick={() => {
                setActive(vItem.index);
                activate(vItem.index);
              }}
            >
              {entry.isDir ? (
                <ChevronRight
                  size={14}
                  className={styles.caret}
                  data-expanded={node.expanded || undefined}
                  aria-hidden
                />
              ) : (
                <span className={styles.caretSpacer} aria-hidden />
              )}
              {entry.isDir ? <Folder size={15} aria-hidden /> : <FileIcon size={15} aria-hidden />}
              <span className={styles.name}>{entry.name}</span>
              {!entry.isDir && (
                <button
                  type="button"
                  className={styles.star}
                  data-on={starredPaths.includes(entry.path) || undefined}
                  title={t('sidebar.star')}
                  aria-label={t('sidebar.star')}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleStar(entry.path);
                  }}
                >
                  <Star size={13} aria-hidden />
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
