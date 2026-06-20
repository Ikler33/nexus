import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { tauriApi, type LinkSuggestion } from '../../lib/tauri-api';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './InspectorRail.module.css';

/** basename без `.md` — фолбэк заголовка (как табы DP-15). */
function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1).replace(/\.md$/, '');
}

/**
 * Inspector «Похожие» (макет editor.jsx): семантически близкие заметки (`get_related_notes` — дискавери,
 * включая уже связанные; max-sim по векторам). Клик — открыть заметку. Перезапрос при смене заметки.
 * Без RAG/LLM — читает готовые векторы (egress нет).
 */
export function RelatedNotes({ path }: { path: string }) {
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);
  const [state, setState] = useState<'loading' | 'ready' | 'error'>('loading');
  const [items, setItems] = useState<LinkSuggestion[]>([]);

  useEffect(() => {
    let alive = true;
    setState('loading');
    tauriApi.suggest
      .related(path, 8)
      .then((r) => {
        if (alive) {
          setItems(r);
          setState('ready');
        }
      })
      .catch(() => {
        if (alive) setState('error');
      });
    return () => {
      alive = false;
    };
  }, [path]);

  if (state === 'loading') return <p className={styles.placeholder}>{t('inspector.loading')}</p>;
  if (state === 'error') return <p className={styles.placeholder}>{t('inspector.error')}</p>;
  if (!items.length) return <p className={styles.placeholder}>{t('inspector.relatedEmpty')}</p>;

  return (
    <ul className={styles.relList}>
      {items.map((s) => (
        <li key={s.path}>
          <button
            type="button"
            className={styles.relItem}
            onClick={() => void openFile(s.path)}
            title={s.path}
          >
            <span className={styles.relTitle}>{s.title ?? basename(s.path)}</span>
            <span className={styles.relMeta}>
              {Math.round(s.score * 100)}%
              {s.reason ? ` · ${s.reason}` : ''}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
