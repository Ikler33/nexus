import { useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import {
  ChevronRight,
  File as FileIcon,
  Folder,
  LayoutGrid,
  Pencil,
  Star,
  Trash2,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { promoteNoteToBoard } from '../../lib/commands-core';
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
  const deleteFile = useVaultStore((s) => s.deleteFile);
  const renameFile = useVaultStore((s) => s.renameFile);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const starredPaths = useStarredStore((s) => s.paths);
  const toggleStar = useStarredStore((s) => s.toggle);
  const { t } = useTranslation();

  // Контекст-меню (right-click): позиция + цель. Закрывается кликом вне и Escape.
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    path: string;
    name: string;
    isDir: boolean;
  } | null>(null);
  // Инлайн-переименование: путь редактируемой строки + текущее значение (имя без .md).
  const [renaming, setRenaming] = useState<{ path: string; value: string } | null>(null);

  // Применить переименование: новый путь = тот же каталог + введённое имя (+ .md для файла).
  const commitRename = (entry: { path: string; isDir: boolean }) => {
    const r = renaming;
    setRenaming(null);
    if (!r) return;
    const newName = r.value.trim();
    if (!newName) return;
    const dir = entry.path.includes('/') ? entry.path.slice(0, entry.path.lastIndexOf('/')) : '';
    const newPath = `${dir ? `${dir}/` : ''}${newName}${entry.isDir ? '' : '.md'}`;
    if (newPath !== entry.path) void renameFile(entry.path, newPath);
  };
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMenu(null);
    };
    window.addEventListener('click', close);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('click', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [menu]);

  const nodes = useMemo(
    () => flattenVisible(childrenByPath, expanded, loading),
    [childrenByPath, expanded, loading],
  );

  const [active, setActive] = useState(0);
  // a11y (audit B10): когда дерево схлопывается/удаляются файлы и узлов стало меньше — `active`
  // мог указывать за пределы → `aria-activedescendant` ссылался на несуществующий treeitem, а
  // onKeyDown читал `nodes[active] === undefined`. Держим индекс в валидном диапазоне.
  useEffect(() => {
    if (active >= nodes.length && nodes.length > 0) setActive(nodes.length - 1);
  }, [nodes.length, active]);
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
    <>
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
              onContextMenu={(e) => {
                e.preventDefault();
                setActive(vItem.index);
                setMenu({
                  x: e.clientX,
                  y: e.clientY,
                  path: entry.path,
                  name: entry.name.replace(/\.md$/, ''),
                  isDir: entry.isDir,
                });
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
              {/* DP-15 (макет sidebar.jsx): расширение .md в дереве не показываем. */}
              {renaming?.path === entry.path ? (
                <input
                  className={styles.renameInput}
                  value={renaming.value}
                  autoFocus
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => setRenaming({ path: entry.path, value: e.target.value })}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === 'Enter') commitRename(entry);
                    else if (e.key === 'Escape') setRenaming(null);
                  }}
                  onBlur={() => commitRename(entry)}
                />
              ) : (
                <span className={styles.name}>{entry.name.replace(/\.md$/, '')}</span>
              )}
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
    {menu && (
      <div
        className={styles.ctxMenu}
        style={{ top: menu.y, left: menu.x }}
        role="menu"
        onClick={(e) => e.stopPropagation()}
      >
        {!menu.isDir && (
          <button
            type="button"
            className={styles.ctxItemNeutral}
            role="menuitem"
            onClick={() => {
              const target = menu;
              setMenu(null);
              void promoteNoteToBoard(target.path);
            }}
          >
            <LayoutGrid size={14} aria-hidden /> {t('tree.toBoard')}
          </button>
        )}
        <button
          type="button"
          className={styles.ctxItemNeutral}
          role="menuitem"
          onClick={() => {
            const target = menu;
            setMenu(null);
            setRenaming({ path: target.path, value: target.name });
          }}
        >
          <Pencil size={14} aria-hidden /> {t('tree.rename')}
        </button>
        <button
          type="button"
          className={styles.ctxItem}
          role="menuitem"
          onClick={() => {
            const target = menu;
            setMenu(null);
            if (window.confirm(t('tree.deleteConfirm', { name: target.name }))) {
              void deleteFile(target.path);
            }
          }}
        >
          <Trash2 size={14} aria-hidden /> {t('tree.delete')}
        </button>
      </div>
    )}
    </>
  );
}
