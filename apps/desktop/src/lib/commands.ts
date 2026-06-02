/**
 * Единый реестр команд (§4.6): ядро И плагины регистрируют команды одним путём; Command
 * Palette, context-menu и keymap работают поверх него. Спроектирован в Ф0-8, чтобы плагинный
 * `registerCommand` (Ф2) не проектировался вслепую.
 *
 * Конфликты хоткеев разрешаются по приоритету **пользователь > плагин > ядро**.
 */

export type CommandSource = 'core' | 'plugin' | 'user';

export interface CommandCtx {
  // Расширяется по мере надобности (активный файл, выделение и т.п.).
  [key: string]: unknown;
}

export interface Command {
  id: string;
  title: string;
  source?: CommandSource;
  /** Хоткей по умолчанию, напр. `"mod+p"` (`mod` = ⌘ на macOS, Ctrl иначе). */
  defaultKey?: string;
  run: (ctx?: CommandCtx) => void | Promise<void>;
}

export interface Disposable {
  dispose: () => void;
}

function isMac(): boolean {
  return typeof navigator !== 'undefined' && /mac/i.test(navigator.platform || '');
}

/** Нормализует combo к виду `ctrl+meta+alt+shift+<key>` (фиксированный порядок модификаторов). */
export function normalizeCombo(combo: string): string {
  const mods = new Set<string>();
  let key = '';
  for (const raw of combo.toLowerCase().split('+')) {
    const p = raw.trim();
    if (!p) continue;
    if (p === 'mod') mods.add(isMac() ? 'meta' : 'ctrl');
    else if (p === 'ctrl' || p === 'control') mods.add('ctrl');
    else if (p === 'meta' || p === 'cmd' || p === 'command') mods.add('meta');
    else if (p === 'alt' || p === 'option') mods.add('alt');
    else if (p === 'shift') mods.add('shift');
    else key = p;
  }
  return [...['ctrl', 'meta', 'alt', 'shift'].filter((m) => mods.has(m)), key].join('+');
}

/** combo из события клавиатуры (тот же порядок модификаторов, что у `normalizeCombo`). */
export function eventToCombo(e: KeyboardEvent): string {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push('ctrl');
  if (e.metaKey) mods.push('meta');
  if (e.altKey) mods.push('alt');
  if (e.shiftKey) mods.push('shift');
  return [...mods, e.key.toLowerCase()].join('+');
}

/** Человекочитаемый хоткей для UI (⌘/Ctrl, ⇧, ⌥). */
export function formatCombo(combo: string): string {
  const mac = isMac();
  return combo
    .split('+')
    .map((p) => {
      const x = p.trim().toLowerCase();
      if (x === 'mod') return mac ? '⌘' : 'Ctrl';
      if (x === 'meta' || x === 'cmd' || x === 'command') return mac ? '⌘' : 'Win';
      if (x === 'ctrl' || x === 'control') return 'Ctrl';
      if (x === 'shift') return '⇧';
      if (x === 'alt' || x === 'option') return mac ? '⌥' : 'Alt';
      return x.length === 1 ? x.toUpperCase() : x;
    })
    .join(mac ? '' : '+');
}

class CommandRegistry {
  private commands = new Map<string, Command>();
  private userKeymap = new Map<string, string>(); // combo → id (пользовательский ремап)
  private listeners = new Set<() => void>();

  register(cmd: Command): Disposable {
    this.commands.set(cmd.id, cmd);
    this.emit();
    return {
      dispose: () => {
        this.commands.delete(cmd.id);
        this.emit();
      },
    };
  }

  list(): Command[] {
    return [...this.commands.values()];
  }

  get(id: string): Command | undefined {
    return this.commands.get(id);
  }

  async run(id: string, ctx?: CommandCtx): Promise<void> {
    await this.commands.get(id)?.run(ctx);
  }

  /** Пользовательский ремап combo → команда (наивысший приоритет). */
  setUserKey(combo: string, id: string): void {
    this.userKeymap.set(normalizeCombo(combo), id);
    this.emit();
  }

  /** Резолвит combo в id команды: пользователь > плагин > ядро. */
  resolve(combo: string): string | undefined {
    const norm = normalizeCombo(combo);
    const user = this.userKeymap.get(norm);
    if (user) return user;
    const matches = this.list().filter(
      (c) => c.defaultKey && normalizeCombo(c.defaultKey) === norm,
    );
    if (matches.length === 0) return undefined;
    const rank = (s?: CommandSource) => (s === 'user' ? 0 : s === 'plugin' ? 1 : 2);
    matches.sort((a, b) => rank(a.source) - rank(b.source));
    return matches[0].id;
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Только для тестов: полный сброс. */
  _reset(): void {
    this.commands.clear();
    this.userKeymap.clear();
    this.emit();
  }

  private emit(): void {
    this.listeners.forEach((f) => f());
  }
}

export const commands = new CommandRegistry();
