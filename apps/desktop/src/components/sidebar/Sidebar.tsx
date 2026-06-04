import { useEffect, useState } from 'react';
import { Plus, Search, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type NoteRef } from '../../lib/tauri-api';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { FileTree } from './FileTree';
import styles from './Sidebar.module.css';

/**
 * Сайдбар: поле поиска + дерево файлов / результаты (Ф0-3 + Ф0-7). Пустой запрос → дерево;
 * непустой → результаты по title/path/tags (debounce 150 мс). Клик по результату открывает файл.
 */
export function Sidebar() {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<NoteRef[]>([]);
  const openFile = useWorkspaceStore((s) => s.openFile);
  const createNote = useVaultStore((s) => s.createNote);
  const vaultOpen = useVaultStore((s) => s.info != null);
  const q = query.trim();

  useEffect(() => {
    if (!q) {
      setResults([]);
      return;
    }
    let cancelled = false;
    const id = setTimeout(() => {
      tauriApi.search
        .searchVault(q)
        .then((r) => {
          if (!cancelled) setResults(r);
        })
        .catch(() => {
          if (!cancelled) setResults([]);
        });
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
  }, [q]);

  return (
    <div className={styles.sidebar}>
      {vaultOpen && (
        <button
          type="button"
          className={styles.newNote}
          onClick={() => void createNote().then((path) => openFile(path))}
          title={t('sidebar.newNote')}
        >
          <Plus size={15} aria-hidden />
          <span>{t('sidebar.newNote')}</span>
        </button>
      )}
      <div className={styles.searchBox}>
        <Search size={14} className={styles.searchIcon} aria-hidden />
        <input
          className={styles.searchInput}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t('sidebar.searchPlaceholder')}
          aria-label={t('sidebar.searchLabel')}
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

      {q ? (
        <ul className={styles.results} aria-label="Результаты поиска">
          {results.length === 0 ? (
            <li className={styles.empty}>{t('sidebar.noResults')}</li>
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
        <FileTree />
      )}
    </div>
  );
}

function noteBase(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}
