import { useEffect, useState } from 'react';
import { Link2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi, type LinkSuggestion } from '../../lib/tauri-api';
import { useToastStore } from '../../stores/toast';
import { useWorkspaceStore } from '../../stores/workspace';
import styles from './NewsView.module.css';

/**
 * FLOW: «Связанные заметки» под ледом ридера — заметки vault, семантически близкие к открытой
 * новости (RAG по заголовку+резюме на бэкенде). Заметка, созданная из этой же новости
 * (frontmatter `source`==url), отфильтрована бэкендом — иначе новость «связана сама с собой».
 * Ленивый запрос по `itemId`; секция не рендерится при загрузке/пустом результате (без шума и
 * без скачка раскладки на «ничего не нашлось»). Релевантность — RRF-score, владельцу не
 * показываем (число без значения для человека); клик открывает заметку в редакторе.
 */
export function RelatedNotesSection(props: { itemId: number }) {
  const { itemId } = props;
  const { t } = useTranslation();
  const openFile = useWorkspaceStore((s) => s.openFile);
  const [items, setItems] = useState<LinkSuggestion[] | null>(null);

  // Открытие связанной заметки закрывает ридер новостей (openFile → closeNews). Если RAG устарел и
  // заметка удалена/переименована, openFile реджектится — БЕЗ обработчика юзер молча оказался бы на
  // пустом редакторе. Ловим и сигналим тостом (TOAST-1) вместо тихого провала.
  const openNote = (path: string) => {
    void openFile(path).catch(() =>
      useToastStore.getState().addToast(t('news.related.openFailed'), { kind: 'error' }),
    );
  };

  useEffect(() => {
    let alive = true;
    setItems(null);
    tauriApi.news
      .related(itemId)
      .then((r) => {
        if (alive) setItems(r);
      })
      .catch(() => {
        if (alive) setItems([]); // ошибка/нет RAG → секция просто не появляется
      });
    return () => {
      alive = false;
    };
  }, [itemId]);

  // Грузится или пусто → не показываем секцию (не мигаем заголовком на «ничего»).
  if (!items || items.length === 0) return null;

  return (
    <section className={styles.relatedBox} aria-label={t('news.related.title')}>
      <div className={styles.relatedHead}>
        <Link2 size={14} aria-hidden />
        {t('news.related.title')}
      </div>
      <ul className={styles.relatedList}>
        {items.map((s) => (
          <li key={s.path}>
            <button
              type="button"
              className={styles.relatedCard}
              title={s.path}
              onClick={() => openNote(s.path)}
            >
              <span className={styles.relatedTitle}>{s.title ?? s.path}</span>
              {s.reason && <span className={styles.relatedReason}>{s.reason}</span>}
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
