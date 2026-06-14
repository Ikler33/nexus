import { Keyboard, X } from 'lucide-react';
import { type TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { commands, formatCombo } from '../../lib/commands';
import { useFocusTrap } from '../../hooks/useFocusTrap';
import { useUIStore } from '../../stores/ui';
import styles from './HotkeysCheatsheet.module.css';

/** Произносимая метка сочетания для скринридера: ⌘⇧P читается как «Command Shift P» (не «⌘⇧P»). */
const SPELL: Record<string, string> = {
  mod: 'Mod',
  meta: 'Cmd',
  cmd: 'Cmd',
  command: 'Cmd',
  ctrl: 'Ctrl',
  control: 'Ctrl',
  shift: 'Shift',
  alt: 'Alt',
  option: 'Alt',
};
function spellCombo(combo: string): string {
  return combo
    .split('+')
    .map((p) => SPELL[p.trim().toLowerCase()] ?? p.trim().toUpperCase())
    .join(' ');
}

/**
 * POLISH «шпаргалка хоткеев» (⌘/): overlay-карта всех горячих клавиш. Источник истины —
 * РЕЕСТР команд (`commands.list()`), показываем только команды С действующим сочетанием
 * (`effectiveKey` уважает пользовательские ремапы → карта честная, без дрейфа от реальности).
 * Группировка по префиксу id команды; формат сочетания — `formatCombo` (⌘/Ctrl/⇧/⌥).
 * Read-only: ничего не запускает, Esc/клик-вне закрывают (паттерн палитры + `useFocusTrap`).
 */

/** Секции шпаргалки по префиксу id (порядок = порядок отображения). Последняя ловит остаток. */
const GROUPS: { id: string; match: (id: string) => boolean }[] = [
  { id: 'navigation', match: (id) => /^(palette|recents|nav|vault)\./.test(id) },
  { id: 'notes', match: (id) => /^(file|note|capture)\./.test(id) },
  { id: 'editor', match: (id) => id.startsWith('editor.') },
  // Вид/оформление/справка + любой непокрытый id (последняя ветка — catch-all, чтобы хоткей не пропал).
  { id: 'view', match: () => true },
];

/** Группирует команды-с-хоткеем (effectiveKey уважает ремапы) по секциям. Дёшево (~20 шт) —
 *  считаем на рендере (только когда шпаргалка открыта), без кэша → всегда отражает свежий ремап. */
function buildSections(t: TFunction) {
  const withKey = commands
    .list()
    .map((c) => ({ id: c.id, title: c.titleKey ? t(c.titleKey) : c.title, key: commands.effectiveKey(c.id) }))
    .filter((c): c is { id: string; title: string; key: string } => Boolean(c.key));
  const used = new Set<string>();
  return GROUPS.map((g) => {
    const items = withKey.filter((c) => !used.has(c.id) && g.match(c.id));
    items.forEach((c) => used.add(c.id));
    return { id: g.id, items };
  }).filter((s) => s.items.length > 0);
}

export function HotkeysCheatsheet() {
  const { t } = useTranslation();
  const open = useUIStore((s) => s.cheatsheetOpen);
  const close = useUIStore((s) => s.closeCheatsheet);
  const ref = useFocusTrap<HTMLDivElement>(close);

  if (!open) return null;
  const sections = buildSections(t);

  return (
    <div className={styles.overlay} onClick={close} role="presentation">
      <div
        ref={ref}
        className={styles.sheet}
        role="dialog"
        aria-modal="true"
        aria-label={t('help.cheatsheet.title')}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <Keyboard size={16} aria-hidden />
          <span className={styles.title}>{t('help.cheatsheet.title')}</span>
          <button
            type="button"
            className={styles.close}
            onClick={close}
            aria-label={t('help.cheatsheet.close')}
            title={t('help.cheatsheet.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>
        <div className={styles.body}>
          {sections.map((s) => (
            <section key={s.id} className={styles.group}>
              <h3 className={styles.groupTitle}>{t(`help.cheatsheet.groups.${s.id}`)}</h3>
              <ul className={styles.list}>
                {s.items.map((c) => (
                  <li key={c.id} className={styles.row}>
                    <span className={styles.label}>{c.title}</span>
                    <kbd className={styles.kbd} aria-label={spellCombo(c.key)}>
                      {formatCombo(c.key)}
                    </kbd>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
        <footer className={styles.foot}>{t('help.cheatsheet.foot')}</footer>
      </div>
    </div>
  );
}
