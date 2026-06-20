import { useState } from 'react';
import { Link2, List, ScrollText, Sparkles, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { BacklinksBar } from './BacklinksBar';
import { NoteSummary } from './NoteSummary';
import { OutlineBar } from './OutlineBar';
import { RelatedNotes } from './RelatedNotes';
import styles from './InspectorRail.module.css';

/** Секции инспектора (макет editor.jsx): оглавление / связи / похожие / резюме. */
type Section = 'outline' | 'backlinks' | 'related' | 'summary';

/**
 * Inspector-rail (макет editor.jsx): правый вертикальный rail с 4 тогглами + сворачиваемая панель,
 * рендерящая активную секцию. Переиспользует OutlineBar/BacklinksBar; `related` — `RelatedNotes`
 * (семантически близкие, get_related_notes), `summary` — `NoteSummary` (LLM-резюме текущего текста).
 *
 * Клик по активному тогглу сворачивает панель (как в макете onToggle: p === k ? null : k).
 */
export function InspectorRail({
  doc,
  path,
  onJump,
}: {
  doc: string;
  path: string;
  /** Переход к заголовку из оглавления (1-based строка) — тот же контракт, что у нижнего OutlineBar. */
  onJump: (line: number) => void;
}) {
  const { t } = useTranslation();
  const [active, setActive] = useState<Section | null>(null);

  const items: { key: Section; icon: typeof List; label: string }[] = [
    { key: 'outline', icon: List, label: t('inspector.outline') },
    { key: 'backlinks', icon: Link2, label: t('inspector.backlinks') },
    { key: 'related', icon: Sparkles, label: t('inspector.related') },
    { key: 'summary', icon: ScrollText, label: t('inspector.summary') },
  ];

  return (
    <div className={styles.row}>
      {active && (
        <aside className={styles.panel} aria-label={t('inspector.panel')}>
          <div className={styles.head}>
            <span>{items.find((i) => i.key === active)?.label}</span>
            <button
              type="button"
              className={styles.close}
              onClick={() => setActive(null)}
              aria-label={t('inspector.collapse')}
            >
              <X size={14} aria-hidden />
            </button>
          </div>
          <div className={styles.body}>
            {active === 'outline' && <OutlineBar doc={doc} onJump={onJump} />}
            {active === 'backlinks' && <BacklinksBar path={path} />}
            {active === 'related' && <RelatedNotes path={path} />}
            {active === 'summary' && <NoteSummary doc={doc} path={path} />}
          </div>
        </aside>
      )}
      <nav className={styles.rail} aria-label={t('inspector.title')}>
        {items.map(({ key, icon: Icon, label }) => (
          <button
            key={key}
            type="button"
            className={`${styles.railBtn} ${active === key ? styles.on : ''}`}
            onClick={() => setActive((p) => (p === key ? null : key))}
            title={label}
            aria-label={label}
            aria-pressed={active === key}
          >
            <Icon size={17} aria-hidden />
          </button>
        ))}
      </nav>
    </div>
  );
}
