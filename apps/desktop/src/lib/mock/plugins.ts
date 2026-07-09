import type { PluginAuditRecord, PluginInfo } from '../tauri-api';
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
  ai: boolean;
  net: string[];
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
    ai: true, // право ai:embed (эмбеддинг + семантический поиск)
    net: ['api.github.com'], // net-allowlist (egress только на эти хосты)
  },
};

interface MockSession {
  id: string;
  read: string[];
  write: string[];
  ui: string[];
  ai: boolean;
  net: string[];
}
const sessions = new Map<string, MockSession>();
let seq = 0;

/** Состояние управления (зеркало backend `plugins.<dir>.enabled` + remove-в-корзину). */
const disabled = new Set<string>();
const removed = new Set<string>();

/**
 * Durable-журнал доступа брокера (зеркало Rust `plugin_audit`, PLUG-1): каждый `invoke` append-only
 * записывает исход авторизации (allow/deny) — так превью/тесты видят РЕАЛЬНУЮ durable-историю
 * (mock-must-match-backend, урок MEM-5). Свежие первыми при чтении (как `ORDER BY id DESC` на бэке).
 */
const auditRecords: PluginAuditRecord[] = [];
let auditSeq = 0;

/** Пишет запись в мок-журнал доступа (append-only, как backend record_durable). */
function recordAudit(
  pluginId: string,
  method: string,
  target: string | null,
  allowed: boolean,
  deniedReason: string | null,
): void {
  auditRecords.push({
    id: ++auditSeq,
    pluginId,
    method,
    target,
    allowed,
    deniedReason,
    createdAt: Math.floor(Date.now() / 1000),
  });
}

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
  return Object.entries(MANIFESTS)
    .filter(([dir]) => !removed.has(dir))
    .map(([dir, m]) => ({
    dir,
    id: m.id,
    name: m.name,
    version: m.version,
    compatible: true,
    error: null,
    enabled: !disabled.has(dir),
    // Чипы прав как у Rust `permission_chips` (DP-8): уровни safe/caution/sensitive.
    permissions: [
      { kind: 'vault:read', detail: m.read.join(', '), level: 'safe' as const },
      { kind: 'vault:write', detail: m.write.join(', '), level: 'caution' as const },
      ...(m.ai ? [{ kind: 'ai:embed', detail: '', level: 'safe' as const }] : []),
      ...(m.net.length
        ? [{ kind: 'net', detail: m.net.join(', '), level: 'sensitive' as const }]
        : []),
      { kind: 'ui', detail: m.ui.join(', '), level: 'safe' as const },
    ],
  }));
}

/** Тест-хелпер: сброс модульного состояния (изоляция тестов — без него remove/disable «протекали» бы). */
export function __resetForTests(): void {
  disabled.clear();
  removed.clear();
  sessions.clear();
  auditRecords.length = 0;
  auditSeq = 0;
}

/**
 * Durable-журнал доступа брокера (PLUG-1): последние `limit` записей, свежие первыми (как backend
 * `list_plugin_audit` → `ORDER BY id DESC LIMIT`). Персист «переживает» в пределах сессии страницы —
 * достаточно, чтобы превью/тесты видели реальную историю allow/deny.
 */
export async function auditLog(limit: number): Promise<PluginAuditRecord[]> {
  const n = Math.max(1, Math.min(limit, 500));
  return auditRecords.slice(-n).reverse();
}

export async function setEnabled(dir: string, on: boolean): Promise<void> {
  if (on) disabled.delete(dir);
  else disabled.add(dir);
}

export async function remove(dir: string): Promise<void> {
  removed.add(dir);
  disabled.delete(dir);
}

export async function openSession(dir: string): Promise<string> {
  const m = MANIFESTS[dir];
  if (!m || removed.has(dir)) throw new Error(`плагин '${dir}' не найден`);
  // Зеркалит backend-гард: выключенный плагин не открывает сессию.
  if (disabled.has(dir)) throw new Error(`плагин выключен: ${dir}`);
  const token = `mock-tok-${++seq}`;
  sessions.set(token, {
    id: m.id,
    read: m.read,
    write: m.write,
    ui: m.ui,
    ai: m.ai,
    net: m.net,
  });
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
  // Цель audit-записи = `path`. Для vault-методов это ПАРИТЕТНО бэку (Rust `req.path.or(req.host)` →
  // path). РАСХОЖДЕНИЕ только для `net.fetch`: бэк пишет ХОСТ (req.host = parsed URL host), а мок здесь
  // пишет полный URL (path). Не чиним намеренно: audit-parity-тест на net.fetch отсутствует, а
  // vault-методы (единственные с parity-тестами) совпадают. Если такой тест появится — писать сюда host.
  const target = path ?? null;
  const pluginId = s?.id ?? '<unknown>';
  try {
    const out = await invokeInner(s, method, path, content);
    // Durable-audit allow (write-before-act паритет: запись на КАЖДЫЙ авторизованный вызов).
    recordAudit(pluginId, method, target, true, null);
    return out;
  } catch (e) {
    // Durable-audit deny — исход отказа тоже персистится (как backend на обоих путях).
    recordAudit(pluginId, method, target, false, e instanceof Error ? e.message : String(e));
    throw e;
  }
}

/** Внутренняя логика авторизации+dispatch мока (без audit-обёртки). */
async function invokeInner(
  s: MockSession | undefined,
  method: string,
  path?: string,
  content?: string,
): Promise<unknown> {
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
    case 'ui.addTranslations': {
      // Любая объявленная ui-точка достаточна; сами строки кладёт фронт-релей в i18n.
      if (s.ui.length === 0) throw new Error('нет права ui');
      return true;
    }
    case 'ai.embed': {
      if (!s.ai) throw new Error('нет права ai:embed');
      if (content == null) throw new Error('нет аргумента content');
      // Детерминированный фейковый вектор (dim 16) для превью.
      const text = content;
      return Array.from({ length: 16 }, (_, i) => ((text.length * (i + 1)) % 17) / 17);
    }
    case 'ai.searchSemantic': {
      if (!s.ai) throw new Error('нет права ai:embed');
      if (content == null) throw new Error('нет аргумента content');
      return vault.searchContent(content, { limit: 8 });
    }
    case 'net.fetch': {
      if (path == null) throw new Error('нет аргумента path (url)');
      let host: string;
      try {
        host = new URL(path).host;
      } catch {
        throw new Error('некорректный URL');
      }
      if (!s.net.includes(host)) throw new Error(`хост не в allowlist: ${host}`);
      return { status: 200, body: `(mock fetch ${host})` };
    }
    default:
      throw new Error(`метод не поддержан host-стороной: ${method}`);
  }
}
