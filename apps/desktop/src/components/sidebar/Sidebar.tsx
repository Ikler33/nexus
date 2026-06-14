import { useEffect, useState } from 'react';
import { FileText, Files, Hash, Home, Plus, Search, Star, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type NoteRef, type TagCount } from '../../lib/tauri-api';
import { useStarredStore } from '../../stores/starred';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { FileTree } from './FileTree';
import styles from './Sidebar.module.css';

/** Панели сайдбара (DP-2, макет `sidebar.jsx`): icon-rail переключает содержимое. */
type Panel = 'files' | 'search' | 'tags' | 'starred';

/** Потолок выдачи тег-фильтра — ЗЕРКАЛО `tags::notes_by_tag` LIMIT (бэкенд). При упоре в него счётчик
 *  чипа показывает «N+» (честно, без молчаливого усечения). */
const TAG_FILTER_LIMIT = 200;

/**
 * Сайдбар (DP-2): icon-rail (файлы / поиск / теги / избранное) + side-nav (Home, новая заметка)
 * + активная панель. Файлы — виртуализированное дерево (Ф0-3); поиск — title/path/tags с
 * debounce (Ф0-7); теги — `list_tags` (клик = поиск по тегу); избранное — звёзды localStorage.
 */
export function Sidebar() {
  const { t } = useTranslation();
  const [panel, setPanel] = useState<Panel>('files');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<NoteRef[]>([]);
  const [tags, setTags] = useState<TagCount[]>([]);
  /** Активный ТОЧНЫЙ фильтр по тегу (клик по тегу): exact-match вместо зашумлённого substring-поиска.
   *  Взаимоисключим с текстовым `query` — ввод текста выходит из тег-режима, × снимает фильтр. */
  const [tagFilter, setTagFilter] = useState<string | null>(null);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const createNote = useVaultStore((s) => s.createNote);
  const vaultOpen = useVaultStore((s) => s.info != null);
  const homeOpen = useUIStore((s) => s.homeOpen);
  const openHome = useUIStore((s) => s.openHome);
  const starred = useStarredStore((s) => s.paths);
  const q = query.trim();

  // Один эффект на оба режима поиск-панели: тег-фильтр (exact, приоритет) ИЛИ текстовый поиск (substring).
  useEffect(() => {
    if (panel !== 'search') {
      setResults([]);
      return;
    }
    let cancelled = false;
    const set = (r: NoteRef[]) => {
      if (!cancelled) setResults(r);
    };
    if (tagFilter) {
      tauriApi.vault
        .notesByTag(tagFilter)
        .then(set)
        .catch(() => set([]));
      return () => {
        cancelled = true;
      };
    }
    if (!q) {
      setResults([]);
      return;
    }
    const id = setTimeout(() => {
      tauriApi.search
        .searchVault(q)
        .then(set)
        .catch(() => set([]));
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
  }, [panel, q, tagFilter]);

  useEffect(() => {
    if (panel !== 'tags' || !vaultOpen) return;
    void tauriApi.vault
      .listTags()
      .then(setTags)
      .catch(() => setTags([]));
  }, [panel, vaultOpen]);

  const rail: { id: Panel; icon: typeof Files; label: string }[] = [
    { id: 'files', icon: Files, label: t('sidebar.files') },
    { id: 'search', icon: Search, label: t('sidebar.search') },
    { id: 'tags', icon: Hash, label: t('sidebar.tags') },
    { id: 'starred', icon: Star, label: t('sidebar.starred') },
  ];

  const searchByTag = (name: string) => {
    setPanel('search');
    setQuery('');
    setResults([]); // не держать прошлый срез на экране, пока грузится notesByTag (асинхронно)
    setTagFilter(name); // ТОЧНЫЙ фильтр, не текстовый поиск по имени тега
  };
  // Ввод текста в поиск выходит из тег-режима (взаимоисключимо).
  const onSearchInput = (v: string) => {
    setQuery(v);
    // Выход из тег-режима: гасим тег-результаты СРАЗУ, иначе они мелькали бы под новым запросом на
    // время debounce (adversarial-ревью). Пустой ввод (стёрли всё) тег НЕ снимает.
    if (v && tagFilter) {
      setTagFilter(null);
      setResults([]);
    }
  };

  return (
    <div className={styles.sidebar}>
      <div className={styles.rail} role="tablist" aria-label={t('sidebar.railLabel')}>
        {rail.map(({ id, icon: Icon, label }) => (
          <button
            key={id}
            type="button"
            role="tab"
            className={`${styles.railBtn} ${panel === id ? styles.railOn : ''}`}
            aria-selected={panel === id}
            title={label}
            aria-label={label}
            onClick={() => setPanel(id)}
          >
            <Icon size={16} aria-hidden />
          </button>
        ))}
      </div>

      {vaultOpen && (
        <nav className={styles.sideNav} aria-label={t('sidebar.navLabel')}>
          <button
            type="button"
            className={`${styles.navItem} ${homeOpen ? styles.navOn : ''}`}
            onClick={() => openHome()}
            aria-current={homeOpen ? 'page' : undefined}
          >
            <Home size={15} aria-hidden />
            <span>{t('sidebar.home')}</span>
          </button>
          <button
            type="button"
            className={styles.navItem}
            onClick={() => void createNote().then((path) => openFile(path))}
          >
            <Plus size={15} aria-hidden />
            <span>{t('sidebar.newNote')}</span>
          </button>
        </nav>
      )}

      {panel === 'files' && (
        <>
          {/* DP-15 (макет sidebar.jsx side-head): заголовок секции с «+» (новая заметка). */}
          {vaultOpen && (
            <div className={styles.panelHead}>
              {t('sidebar.files')}
              <button
                type="button"
                className={styles.panelHeadBtn}
                onClick={() => void createNote().then((path) => openFile(path))}
                title={t('sidebar.newNote')}
                aria-label={t('sidebar.newNote')}
              >
                <Plus size={14} aria-hidden />
              </button>
            </div>
          )}
          <FileTree />
        </>
      )}

      {panel === 'search' && (
        <>
          <div className={styles.searchBox}>
            <Search size={14} className={styles.searchIcon} aria-hidden />
            <input
              className={styles.searchInput}
              type="text"
              value={query}
              onChange={(e) => onSearchInput(e.target.value)}
              placeholder={t('sidebar.searchPlaceholder')}
              aria-label={t('sidebar.searchLabel')}
              autoFocus
            />
            {query && (
              <button
                className={styles.clear}
                onClick={() => setQuery('')}
                aria-label={t('sidebar.clearSearch')}
              >
                <X size={14} aria-hidden />
              </button>
            )}
          </div>
          {tagFilter && (
            <div className={styles.tagFilter}>
              <span className={styles.tagFilterChip}>
                <Hash size={12} aria-hidden />
                {tagFilter}
                <button
                  type="button"
                  className={styles.tagFilterClear}
                  onClick={() => setTagFilter(null)}
                  aria-label={t('sidebar.tagFilterClear')}
                >
                  <X size={12} aria-hidden />
                </button>
              </span>
              <span className={styles.tagFilterCount}>
                {results.length >= TAG_FILTER_LIMIT
                  ? t('sidebar.tagFilterCountCapped', { count: TAG_FILTER_LIMIT })
                  : t('sidebar.tagFilterCount', { count: results.length })}
              </span>
            </div>
          )}
          {q || tagFilter ? (
            <ul className={styles.results} aria-label={t('sidebar.resultsLabel')}>
              {results.length === 0 ? (
                <li className={styles.empty}>
                  {tagFilter ? t('sidebar.tagNoResults') : t('sidebar.noResults')}
                </li>
              ) : (
                results.map((r) => (
                  <li key={r.path}>
                    <button className={styles.result} onClick={() => void openFile(r.path)}>
                      <span className={styles.resultName}>{noteBase(r.path)}</span>
                      <span className={styles.resultPath}>{r.path}</span>
                    </button>
                  </li>
                ))
              )}
            </ul>
          ) : (
            <div className={styles.panelHint}>{t('sidebar.searchHint')}</div>
          )}
        </>
      )}

      {panel === 'tags' && (
        <div className={styles.panelScroll}>
          <div className={styles.panelHead}>{t('sidebar.tags')}</div>
          {tags.length === 0 ? (
            <div className={styles.panelHint}>{t('sidebar.tagsEmpty')}</div>
          ) : (
            tags.map((tag) => (
              <button
                key={tag.name}
                type="button"
                className={styles.tagRow}
                onClick={() => searchByTag(tag.name)}
              >
                <Hash size={14} className={styles.tagIcon} aria-hidden />
                <span className={styles.tagName}>{tag.name}</span>
                <span className={styles.tagCount}>{tag.count}</span>
              </button>
            ))
          )}
        </div>
      )}

      {panel === 'starred' && (
        <div className={styles.panelScroll}>
          <div className={styles.panelHead}>{t('sidebar.starred')}</div>
          {starred.length === 0 ? (
            <div className={styles.panelHint}>{t('sidebar.starredEmpty')}</div>
          ) : (
            starred.map((path) => (
              <button
                key={path}
                type="button"
                className={styles.tagRow}
                onClick={() => void openFile(path)}
              >
                <FileText size={14} aria-hidden />
                <span className={styles.tagName}>{noteBase(path)}</span>
                <Star size={12} className={styles.starOn} aria-hidden />
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
}

function noteBase(path: string): string {
  const base = path.slice(path.lastIndexOf('/') + 1);
  return base.endsWith('.md') ? base.slice(0, -3) : base;
}
