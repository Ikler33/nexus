import { Fragment } from 'react';
import type { FmField } from '../../lib/markdown/frontmatter';
import styles from './MarkdownPreview.module.css';

/** Поля-теги: значения рендерятся чипами и (при onOpenTag) кликаются как `#tag`-фильтр. */
const TAG_FIELDS = new Set(['tags', 'tag', 'aliases']);

/**
 * Properties-таблица frontmatter в режиме чтения (FRONTMATTER-1, по образцу Obsidian Properties).
 *
 * Hermes-8 S4 «Вариант А · Колонка»: `.properties` — сам CSS-grid (`92px 1fr`), а `.propKey`/`.propVal` —
 * прямые дети grid (без обёртки `.propRow`), чтобы ключи/значения выравнивались по 2 колонкам сквозь ВСЕ
 * строки. div'ы, не `<table>` — чтобы не конфликтовать с `.preview table` GFM. Значения полей-тегов —
 * sage-чипы (нижний регистр + фильтр сайдбара, как inline `#tag`); прочее — текст через запятую.
 * Значение ключа `type` получает ember-акцент (`.acc`) по README.
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
        const isType = f.key.toLowerCase() === 'type';
        return (
          // Фрагмент, а не `.propRow`-обёртка: ключ+значение — прямые дети grid-контейнера `.properties`,
          // чтобы колонки 92px/1fr выравнивались по всем строкам (README «Вариант А · Колонка»).
          <Fragment key={`${f.key}-${i}`}>
            <div className={styles.propKey}>{f.key}</div>
            <div className={`${styles.propVal}${isType ? ` ${styles.acc}` : ''}`}>
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
          </Fragment>
        );
      })}
    </div>
  );
}
