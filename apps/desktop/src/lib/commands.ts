/**
 * Единый реестр команд (§4.6): ядро И плагины регистрируют команды одним путём; Command
 * Palette, context-menu и keymap работают поверх него. Спроектирован в Ф0-8, чтобы плагинный
 * `registerCommand` (Ф2) не проектировался вслепую.
 *
 * Конфликты хоткеев разрешаются по приоритету **пользователь > плагин > ядро**.
 */

import i18n from '../i18n/setup';
import { logUi } from './debug-log';
import { useToastStore } from '../stores/toast';

export type CommandSource = 'core' | 'plugin' | 'user';

export interface CommandCtx {
  // Расширяется по мере надобности (активный файл, выделение и т.п.).
  [key: string]: unknown;
}

export interface Command {
  id: string;
  /** Человекочитаемое название (fallback, если нет `titleKey`). */
  title: string;
  /** Ключ i18n для названия (предпочтительнее `title` в UI). */
  titleKey?: string;
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

/** Произносимая метка сочетания для скринридера: ⌘⇧P читается как «Cmd Shift P» (не «⌘⇧P»). */
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
export function spellCombo(combo: string): string {
  return combo
    .split('+')
    .map((p) => SPELL[p.trim().toLowerCase()] ?? p.trim().toUpperCase())
    .join(' ');
}

/** Ключ localStorage для пользовательского ремапа хоткеев (combo → id). */
const HOTKEYS_KEY = 'nexus.hotkeys.v1';

/** Алиасы переименованных id команд (старый → новый). При вырезании фичи в модуль (F-9+) id команды
 *  префиксуется (`view.news` → `news:view.news`); ручной хоткей пользователя хранит СТАРЫЙ id и без
 *  ремапа no-op'ил бы. Каждый срез F-10 дописывает сюда свою пару. */
const COMMAND_ID_ALIASES: Record<string, string> = {
  'view.news': 'news:view.news',
  'view.goals': 'goals:view.goals', // F-10b (оверлей-модуль)
  'view.memory': 'memory:view.memory', // F-10b
  'view.tasks': 'tasks:view.tasks', // F-10b
  'view.inbox': 'inbox:view.inbox', // F-10b
};

class CommandRegistry {
  private commands = new Map<string, Command>();
  private userKeymap = new Map<string, string>(); // combo → id (пользовательский ремап)
  private listeners = new Set<() => void>();

  constructor() {
    this.loadUserKeymap();
  }

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
    const cmd = this.commands.get(id);
    if (!cmd) return;
    // F-8 (ErrorBoundary для команд): вклад-команда упала → тост, НЕ белый экран/висящий reject
    // (`commands.run` зовут через `void` в useKeymap/CommandPalette). Логируем в бэкенд-журнал
    // (не console.error — тот ловит e2e-гейт). Цель владельца: «ИИ правит модуль → app не падает».
    try {
      await cmd.run(ctx);
    } catch (err) {
      logUi('command-error', `${id}: ${err instanceof Error ? err.message : String(err)}`.slice(0, 300));
      const name = cmd.titleKey ? i18n.t(cmd.titleKey) : cmd.title;
      useToastStore.getState().addToast(i18n.t('connector.commandFailed', { name }), {
        kind: 'error',
      });
    }
  }

  /** Пользовательский ремап combo → команда (наивысший приоритет). Персист. */
  setUserKey(combo: string, id: string): void {
    this.userKeymap.set(normalizeCombo(combo), id);
    this.persistUserKeymap();
    this.emit();
  }

  /** Текущий пользовательский combo команды (если переопределён), нормализованный. */
  userKeyFor(id: string): string | undefined {
    for (const [combo, cmdId] of this.userKeymap) if (cmdId === id) return combo;
    return undefined;
  }

  /** Эффективный combo команды для UI: пользовательский оверрайд → дефолт (оба нормализованы). */
  effectiveKey(id: string): string | undefined {
    const user = this.userKeyFor(id);
    if (user) return user;
    const def = this.commands.get(id)?.defaultKey;
    return def ? normalizeCombo(def) : undefined;
  }

  /**
   * Назначить команде хоткей: снимает прежний пользовательский бинд ЭТОЙ команды, затем ставит новый
   * (combo → id). Конфликт с другой командой не блокируется — резолв даст приоритет пользователю, а UI
   * подсветит совпадение. Персист.
   */
  remap(id: string, combo: string): void {
    const norm = normalizeCombo(combo);
    for (const [c, cmdId] of [...this.userKeymap]) if (cmdId === id) this.userKeymap.delete(c);
    this.userKeymap.set(norm, id);
    this.persistUserKeymap();
    this.emit();
  }

  /** Сбросить команду к дефолтному хоткею (убрать пользовательский оверрайд). Персист. */
  resetKey(id: string): void {
    let changed = false;
    for (const [c, cmdId] of [...this.userKeymap]) {
      if (cmdId === id) {
        this.userKeymap.delete(c);
        changed = true;
      }
    }
    if (changed) {
      this.persistUserKeymap();
      this.emit();
    }
  }

  private persistUserKeymap(): void {
    try {
      localStorage.setItem(HOTKEYS_KEY, JSON.stringify(Object.fromEntries(this.userKeymap)));
    } catch {
      /* localStorage недоступен */
    }
  }

  private loadUserKeymap(): void {
    try {
      const raw = localStorage.getItem(HOTKEYS_KEY);
      if (!raw) return;
      const obj = JSON.parse(raw) as Record<string, unknown>;
      for (const [combo, id] of Object.entries(obj)) {
        if (typeof id === 'string') this.userKeymap.set(normalizeCombo(combo), id);
      }
    } catch {
      /* битый JSON / нет localStorage — игнорируем */
    }
  }

  /** Резолвит combo в id команды: пользователь > плагин > ядро. Сохранённый пользователем биндинг
   *  на СТАРЫЙ id (переименован при вырезании фичи в модуль, F-9+) ремапится через COMMAND_ID_ALIASES —
   *  иначе ручной хоткей молча no-op'ил бы на несуществующий id. Серия F-10 расширяет карту одной строкой. */
  resolve(combo: string): string | undefined {
    const norm = normalizeCombo(combo);
    const user = this.userKeymap.get(norm);
    if (user) return COMMAND_ID_ALIASES[user] ?? user;
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
    try {
      localStorage.removeItem(HOTKEYS_KEY);
    } catch {
      /* ignore */
    }
    this.emit();
  }

  private emit(): void {
    this.listeners.forEach((f) => f());
  }
}

export const commands = new CommandRegistry();
