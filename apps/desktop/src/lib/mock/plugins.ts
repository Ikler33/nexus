import type { PluginInfo } from '../tauri-api';
import * as vault from './vault';

/**
 * Мок capability-брокера для превью/тестов (зеркалит Rust `PluginBroker` + `dispatch_vault`):
 * `openSession` выдаёт токен, привязанный к scoped-правам мок-манифеста; `invoke` проверяет scope
 * (glob с deny-override, как Rust `path_in_scope`) и затем делает мок-I/O по `./vault`. Так превью
 * показывает РЕАЛЬНОЕ поведение границы прав (включая отказы), не дожидаясь нативного брокера.
 */

interface MockManifest {
  id: string;
  name: string;
  version: string;
  read: string[];
  write: string[];
  ui: string[];
}

/** «Установленные» плагины превью-vault (соответствуют `.nexus/plugins/<dir>`). */
const MANIFESTS: Record<string, MockManifest> = {
  hello: {
    id: 'hello-reader',
    name: 'Hello Reader (demo)',
    version: '0.1.0',
    read: ['**'], // читает весь vault
    write: ['Notes/**'], // пишет только в Notes/ (демонстрация границы)
    ui: ['command'], // право регистрировать команды в палитре
  },
};

interface MockSession {
  read: string[];
  write: string[];
  ui: string[];
}
const sessions = new Map<string, MockSession>();
let seq = 0;

// ─── glob (сегментный, зеркало Rust `glob_match`: `**`=0..N сегментов, `*`=в пределах сегмента) ───

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function segMatch(pat: string, seg: string): boolean {
  if (pat === '*') return true;
  if (!pat.includes('*')) return pat === seg;
  const re = new RegExp(`^${pat.split('*').map(escapeRe).join('[^/]*')}$`);
  return re.test(seg);
}

function matchFrom(g: string[], gi: number, p: string[], pi: number): boolean {
  if (gi === g.length) return pi === p.length;
  if (g[gi] === '**') {
    for (let k = pi; k <= p.length; k++) if (matchFrom(g, gi + 1, p, k)) return true;
    return false;
  }
  if (pi === p.length) return false;
  return segMatch(g[gi], p[pi]) && matchFrom(g, gi + 1, p, pi + 1);
}

function globMatch(glob: string, path: string): boolean {
  return matchFrom(glob.split('/'), 0, path === '' ? [] : path.split('/'), 0);
}

/** Разрешено ли по scope: любой `!`-паттерн (deny) перекрывает allow (как Rust `path_in_scope`). */
function inScope(scope: string[], path: string): boolean {
  let allowed = false;
  for (const g of scope) {
    if (g.startsWith('!')) {
      if (globMatch(g.slice(1), path)) return false;
    } else if (globMatch(g, path)) {
      allowed = true;
    }
  }
  return allowed;
}

// ─── Контракт `tauriApi.plugins` (мок) ───────────────────────────────────────────────────────────

export async function list(): Promise<PluginInfo[]> {
  return Object.entries(MANIFESTS).map(([dir, m]) => ({
    dir,
    id: m.id,
    name: m.name,
    version: m.version,
    compatible: true,
    error: null,
  }));
}

export async function openSession(dir: string): Promise<string> {
  const m = MANIFESTS[dir];
  if (!m) throw new Error(`плагин '${dir}' не найден`);
  const token = `mock-tok-${++seq}`;
  sessions.set(token, { read: m.read, write: m.write, ui: m.ui });
  return token;
}

export async function closeSession(token: string): Promise<void> {
  sessions.delete(token);
}

export async function invoke(
  token: string,
  method: string,
  path?: string,
  content?: string,
): Promise<unknown> {
  const s = sessions.get(token);
  if (!s) throw new Error('сессия не найдена (отозвана?)');

  switch (method) {
    case 'vault.readFile': {
      if (path == null) throw new Error('нет аргумента path');
      if (!inScope(s.read, path)) throw new Error(`нет права vault:read на «${path}»`);
      return vault.readFile(path);
    }
    case 'vault.listFiles': {
      const dirPath = path ?? '';
      if (dirPath !== '' && !inScope(s.read, dirPath))
        throw new Error(`нет права vault:read на «${dirPath}»`);
      return vault.listDir(dirPath);
    }
    case 'vault.writeFile': {
      if (path == null) throw new Error('нет аргумента path');
      if (content == null) throw new Error('нет аргумента content');
      if (!inScope(s.write, path)) throw new Error(`нет права vault:write на «${path}»`);
      await vault.writeFile(path, content);
      return { ok: true, bytes: content.length };
    }
    case 'ui.registerCommand': {
      // Брокер только авторизует (ui:command); саму команду регистрирует фронт-релей.
      if (!s.ui.includes('command')) throw new Error('нет права ui:command');
      return true;
    }
    default:
      throw new Error(`метод не поддержан host-стороной: ${method}`);
  }
}
