import { useEffect, useMemo, useState } from 'react';
import { commands, formatCombo } from '../../lib/commands';
import { useUIStore } from '../../stores/ui';
import styles from './CommandPalette.module.css';

/**
 * Command Palette (Cmd/Ctrl+P) поверх единого реестра команд (§4.6). Клавиатура:
 * ↑/↓ — выбор, Enter — выполнить, Esc — закрыть. Keyboard-first (DESIGN §1/§9a).
 */
export function CommandPalette() {
  const open = useUIStore((s) => s.paletteOpen);
  const close = useUIStore((s) => s.closePalette);

  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [version, setVersion] = useState(0);

  // Перерисовка при изменении набора команд (register/dispose).
  useEffect(() => commands.subscribe(() => setVersion((v) => v + 1)), []);
  useEffect(() => {
    if (open) {
      setQuery('');
      setActive(0);
    }
  }, [open]);

  const filtered = useMemo(() => {
    void version; // пересчёт при register/dispose
    const q = query.trim().toLowerCase();
    const all = commands.list().sort((a, b) => a.title.localeCompare(b.title));
    return q ? all.filter((c) => c.title.toLowerCase().includes(q)) : all;
  }, [query, version]);

  if (!open) return null;

  const runAt = (index: number) => {
    const cmd = filtered[index];
    if (!cmd) return;
    close();
    void commands.run(cmd.id);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActive((a) => Math.min(filtered.length - 1, a + 1));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setActive((a) => Math.max(0, a - 1));
        break;
      case 'Enter':
        e.preventDefault();
        runAt(active);
        break;
      case 'Escape':
        e.preventDefault();
        close();
        break;
    }
  };

  return (
    <div className={styles.overlay} onClick={close}>
      <div
        className={styles.palette}
        role="dialog"
        aria-label="Палитра команд"
        onClick={(e) => e.stopPropagation()}
      >
        <input
          className={styles.input}
          autoFocus
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          placeholder="Команда…"
          aria-label="Команда"
          role="combobox"
          aria-expanded
          aria-controls="command-list"
        />
        <ul className={styles.list} id="command-list" role="listbox">
          {filtered.length === 0 ? (
            <li className={styles.empty}>Нет команд</li>
          ) : (
            filtered.map((cmd, i) => (
              <li
                key={cmd.id}
                role="option"
                aria-selected={i === active}
                data-active={i === active || undefined}
                className={styles.item}
                onMouseEnter={() => setActive(i)}
                onClick={() => runAt(i)}
              >
                <span className={styles.title}>{cmd.title}</span>
                {cmd.defaultKey && <kbd className={styles.kbd}>{formatCombo(cmd.defaultKey)}</kbd>}
              </li>
            ))
          )}
        </ul>
      </div>
    </div>
  );
}
