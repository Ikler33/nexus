import { useEffect, useState, type ComponentType } from 'react';
import { Lightbulb, Link2, List, ScrollText, X } from 'lucide-react';
import { OrbitIcon } from '../common/BrandGlyphs';
import { useTranslation } from 'react-i18next';
import { SuggestView } from '../chat/SuggestView';
import { useUIStore } from '../../stores/ui';
import { BacklinksBar } from './BacklinksBar';
import { NoteSummary } from './NoteSummary';
import { OutlineBar } from './OutlineBar';
import { RelatedNotes } from './RelatedNotes';
import styles from './InspectorRail.module.css';

/** Секции инспектора: оглавление / беклинки / похожие / связи / резюме. «Связи» (suggest) переехали
 *  сюда из AI-панели (Hermes-6: панель = Чат+Castor) — per-заметочные предложения ссылок. */
type Section = 'outline' | 'backlinks' | 'related' | 'summary' | 'suggest';

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
  activeLine,
}: {
  doc: string;
  path: string;
  /** Переход к заголовку из оглавления (1-based строка) — тот же контракт, что у нижнего OutlineBar. */
  onJump: (line: number) => void;
  /** Hermes-8 S6 scroll-spy: исходная строка активного заголовка (считается в GroupPane) → подсветка
   *  пункта оглавления. Прокидывается в OutlineBar; вне режима чтения/превью rail скрыт. */
  activeLine?: number | null;
}) {
  const { t } = useTranslation();
  const [active, setActive] = useState<Section | null>(null);
  // Команда палитры «Связи» (или иной внешний запрос) открывает секцию инспектора — читаем и
  // сбрасываем отложенный запрос (паттерн pendingTagFilter).
  const pendingSection = useUIStore((s) => s.pendingInspectorSection);
  const consumeSection = useUIStore((s) => s.consumeInspectorSection);
  useEffect(() => {
    if (pendingSection) {
      setActive(pendingSection as Section);
      consumeSection();
    }
  }, [pendingSection, consumeSection]);

  const items: {
    key: Section;
    icon: ComponentType<{ size?: number; 'aria-hidden'?: boolean }>;
    label: string;
  }[] = [
    { key: 'outline', icon: List, label: t('inspector.outline') },
    { key: 'backlinks', icon: Link2, label: t('inspector.backlinks') },
    { key: 'related', icon: OrbitIcon, label: t('inspector.related') },
    { key: 'suggest', icon: Lightbulb, label: t('inspector.suggest') },
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
            {active === 'outline' && <OutlineBar doc={doc} onJump={onJump} activeLine={activeLine} />}
            {active === 'backlinks' && <BacklinksBar path={path} />}
            {active === 'related' && <RelatedNotes path={path} />}
            {active === 'suggest' && <SuggestView />}
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
