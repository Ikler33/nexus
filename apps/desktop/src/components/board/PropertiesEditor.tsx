import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FlushFailedError, writeFrontmatterField } from '../../lib/frontmatter-edit';
import { tauriApi, type NoteProperty } from '../../lib/tauri-api';
import { useToastStore } from '../../stores/toast';
import { isChecked, isValidForType } from './prop-widgets';
import styles from './PropertiesEditor.module.css';

/**
 * Properties-панель (PROP-3, спека §7): типизированные виджеты frontmatter-свойств заметки + ИНЛАЙН-правка
 * через `set_frontmatter_field` (общий безопасный путь `writeFrontmatterField` — флаш буфера, анти-эхо).
 * Тип — из реестра PROP-2 (`get_note_properties`). Значение не под типом → жёлтое invalid-поле «в source».
 * MVP-виджеты: text/number/checkbox/date (+ datetime/list/tags как текст). Списки-теги/«+ свойство» — PROP-4.
 */
export function PropertiesEditor({
  path,
  onOpenSource,
  onChanged,
}: {
  path: string;
  /** «Править в source» (invalid-значение) — открыть заметку в редакторе. */
  onOpenSource: () => void;
  /** Колбэк после успешной записи свойства (доска перечитывает карточки). */
  onChanged?: () => void;
}) {
  const { t } = useTranslation();
  const addToast = useToastStore((s) => s.addToast);
  const [props, setProps] = useState<NoteProperty[] | null>(null);
  const [savingKey, setSavingKey] = useState<string | null>(null);

  const load = useCallback(() => {
    let alive = true;
    setProps(null);
    tauriApi.properties
      .forNote(path)
      .then((p) => {
        if (alive) setProps(p);
      })
      .catch(() => {
        if (alive) setProps([]);
      });
    return () => {
      alive = false;
    };
  }, [path]);

  useEffect(() => load(), [load]);

  const save = async (key: string, next: string) => {
    const cur = props?.find((p) => p.key === key);
    if (!cur || cur.value === next) return; // без изменений
    setSavingKey(key);
    try {
      await writeFrontmatterField(path, key, next);
      setProps((ps) => ps?.map((p) => (p.key === key ? { ...p, value: next } : p)) ?? null);
      onChanged?.();
    } catch (e) {
      addToast(e instanceof FlushFailedError ? t('prop.flushError') : t('prop.saveError'), {
        kind: 'error',
      });
      load(); // откат к диску визуально
    } finally {
      setSavingKey(null);
    }
  };

  const enterBlur = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') e.currentTarget.blur();
  };

  if (props === null) {
    return <div className={styles.muted}>{t('board.loading')}</div>;
  }
  if (props.length === 0) {
    return <div className={styles.muted}>{t('prop.empty')}</div>;
  }

  return (
    <div className={styles.props}>
      {props.map((p) => {
        const disabled = savingKey === p.key;
        const inputKey = `${p.key}:${p.value}`;
        let widget;
        if (!isValidForType(p.type, p.value)) {
          widget = (
            <span className={styles.invalid}>
              <span className={styles.invalidVal}>{p.value}</span>
              <button type="button" className={styles.sourceBtn} onClick={onOpenSource}>
                {t('prop.editSource')}
              </button>
            </span>
          );
        } else if (p.type === 'list' || p.type === 'tags') {
          // Списки/теги — read-only здесь (чип-правка + автокомплит — PROP-4); правка через source.
          widget = (
            <span className={styles.readonly}>
              <span className={styles.readonlyVal}>{p.value}</span>
              <button type="button" className={styles.sourceBtn} onClick={onOpenSource}>
                {t('prop.editSource')}
              </button>
            </span>
          );
        } else if (p.type === 'checkbox') {
          widget = (
            <input
              type="checkbox"
              className={styles.checkbox}
              checked={isChecked(p.value)}
              disabled={disabled}
              onChange={(e) => void save(p.key, e.target.checked ? 'true' : 'false')}
              aria-label={p.key}
            />
          );
        } else {
          const type = p.type === 'number' ? 'number' : p.type === 'date' ? 'date' : 'text';
          widget = (
            <input
              key={inputKey}
              type={type}
              className={styles.input}
              defaultValue={p.value}
              disabled={disabled}
              onBlur={(e) => void save(p.key, e.target.value)}
              onKeyDown={enterBlur}
              aria-label={p.key}
            />
          );
        }
        return (
          <div key={p.key} className={styles.row}>
            <span className={styles.key}>{p.key}</span>
            <div className={styles.val}>{widget}</div>
          </div>
        );
      })}
    </div>
  );
}
