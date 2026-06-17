import type { FmField } from '../../lib/markdown/frontmatter';
import styles from './MarkdownPreview.module.css';

/** Поля-теги: значения рендерятся чипами и (при onOpenTag) кликаются как `#tag`-фильтр. */
const TAG_FIELDS = new Set(['tags', 'tag', 'aliases']);

/**
 * Properties-таблица frontmatter в режиме чтения (FRONTMATTER-1, по образцу Obsidian Properties).
 * Сетка ключ→значение (div'ы, не `<table>` — чтобы не конфликтовать с `.preview table` GFM). Значения
 * полей-тегов — чипы (нижний регистр + фильтр сайдбара, как inline `#tag`); прочее — текст через запятую.
 */
export function PropertiesTable({
  fields,
  onOpenTag,
}: {
  fields: FmField[];
  onOpenTag?: (tag: string) => void;
}) {
  return (
    <div className={styles.properties}>
      {fields.map((f, i) => {
        const isTag = TAG_FIELDS.has(f.key.toLowerCase());
        return (
          <div className={styles.propRow} key={`${f.key}-${i}`}>
            <div className={styles.propKey}>{f.key}</div>
            <div className={styles.propVal}>
              {isTag
                ? f.values.map((v, j) => {
                    const tag = v.replace(/^#/, '').toLowerCase();
                    const label = v.startsWith('#') ? v : `#${v}`;
                    if (!onOpenTag) return <span key={j} className={styles.tag}>{label}</span>;
                    return (
                      <span
                        key={j}
                        className={styles.tag}
                        role="button"
                        tabIndex={0}
                        onClick={() => onOpenTag(tag)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault();
                            onOpenTag(tag);
                          }
                        }}
                      >
                        {label}
                      </span>
                    );
                  })
                : f.values.join(', ')}
            </div>
          </div>
        );
      })}
    </div>
  );
}
