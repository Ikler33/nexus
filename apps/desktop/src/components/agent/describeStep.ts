/**
 * Человекочитаемая подпись шага агента для «ленты шагов» (вместо голого глагола `fetch`/`note.edit`).
 *
 * Приоритет:
 *  1) `title` от агента (ACP `tool_call.title` — Hermes присылает готовый текст, напр. «Fetching docs.rs»);
 *  2) детерминированный i18n-шаблон по `(kind, args)` — нативный Кастор (глагол + path/query/url);
 *  3) сырой `kind` — неизвестный/будущий инструмент (никогда не падаем).
 *
 * Честность: шаблон строится ТОЛЬКО из реальных `args` шага (path/query/command/url) — без выдумки.
 * Сырой stdout (`result`) сюда НЕ попадает (приватность §5.6 — у нас тут только args).
 *
 * Единая таблица `ACT` покрывает И Castor-точечные глаголы (`note.edit`), И Hermes-ACP-строки
 * (`edit`/`fetch`/…) — паритет тот же, что у `classifyKind` в flow-graph.
 */
import type { AgentStep } from '../../stores/agent';

/** Минимальный тип i18next-`t` (ключ + опц. интерполяция) — без импорта тяжёлого `TFunction`. */
type T = (key: string, opts?: Record<string, unknown>) => string;

type ArgKey = 'path' | 'query' | 'command' | 'url';

interface ActSpec {
  /** i18n-ключ с `{{q}}` (когда деталь из args есть). */
  key: string;
  /** i18n-ключ без аргумента (деталь не распарсилась / инструмент без args). */
  bare: string;
  /** Предпочитаемый ключ args для детали; отсутствует → действие без аргумента (think/plan). */
  arg?: ArgKey;
}

// kind → {ключ, bare, какой arg показать}. Покрывает Castor (точечные) + Hermes-ACP (короткие).
const ACT: Record<string, ActSpec> = {
  // web
  'web.search': { key: 'agent.act.search', bare: 'agent.act.searchBare', arg: 'query' },
  search: { key: 'agent.act.search', bare: 'agent.act.searchBare', arg: 'query' },
  'web.fetch': { key: 'agent.act.fetch', bare: 'agent.act.fetchBare', arg: 'url' },
  fetch: { key: 'agent.act.fetch', bare: 'agent.act.fetchBare', arg: 'url' },
  // file: create / write
  'note.create': { key: 'agent.act.create', bare: 'agent.act.createBare', arg: 'path' },
  create: { key: 'agent.act.create', bare: 'agent.act.createBare', arg: 'path' },
  write: { key: 'agent.act.create', bare: 'agent.act.createBare', arg: 'path' },
  // file: edit
  'note.edit': { key: 'agent.act.edit', bare: 'agent.act.editBare', arg: 'path' },
  edit: { key: 'agent.act.edit', bare: 'agent.act.editBare', arg: 'path' },
  // file: delete
  'note.delete': { key: 'agent.act.delete', bare: 'agent.act.deleteBare', arg: 'path' },
  delete: { key: 'agent.act.delete', bare: 'agent.act.deleteBare', arg: 'path' },
  // file: move
  move: { key: 'agent.act.move', bare: 'agent.act.moveBare', arg: 'path' },
  // read
  'note.read': { key: 'agent.act.read', bare: 'agent.act.readBare', arg: 'path' },
  read: { key: 'agent.act.read', bare: 'agent.act.readBare', arg: 'path' },
  'vault.read': { key: 'agent.act.read', bare: 'agent.act.readBare', arg: 'path' },
  'fs.read': { key: 'agent.act.read', bare: 'agent.act.readBare', arg: 'path' },
  // память
  recall: { key: 'agent.act.recall', bare: 'agent.act.recallBare', arg: 'query' },
  // поиск по волту
  grep: { key: 'agent.act.grep', bare: 'agent.act.grepBare', arg: 'query' },
  // команда / шелл
  shell: { key: 'agent.act.command', bare: 'agent.act.commandBare', arg: 'command' },
  execute: { key: 'agent.act.command', bare: 'agent.act.commandBare', arg: 'command' },
  exec: { key: 'agent.act.command', bare: 'agent.act.commandBare', arg: 'command' },
  process: { key: 'agent.act.command', bare: 'agent.act.commandBare', arg: 'command' },
  // git
  git: { key: 'agent.act.git', bare: 'agent.act.gitBare', arg: 'command' },
  // размышление / план (без аргумента)
  think: { key: 'agent.act.think', bare: 'agent.act.think' },
  reason: { key: 'agent.act.think', bare: 'agent.act.think' },
  plan: { key: 'agent.act.plan', bare: 'agent.act.plan' },
};

const DETAIL_MAX = 64;
const TITLE_MAX = 120;

function truncate(s: string, max: number): string {
  const t = s.trim();
  return t.length <= max ? t : t.slice(0, max - 1).trimEnd() + '…';
}

/**
 * Деталь из JSON-`args`: сперва предпочитаемый ключ, затем path|query|command|url. Кривой JSON или
 * отсутствие строковых значений → undefined (тогда вызывающий берёт `bare`-форму).
 */
export function argDetail(args: string, prefer?: ArgKey): string | undefined {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(args) as Record<string, unknown>;
  } catch {
    return undefined;
  }
  if (parsed == null || typeof parsed !== 'object') return undefined;
  const order: ArgKey[] = ['path', 'query', 'command', 'url'];
  const keys = prefer ? [prefer, ...order.filter((k) => k !== prefer)] : order;
  for (const k of keys) {
    const v = parsed[k];
    if (typeof v === 'string' && v.trim().length > 0) return truncate(v, DETAIL_MAX);
  }
  return undefined;
}

/** Подпись шага для ленты (см. приоритет в шапке файла). */
export function describeStep(step: Pick<AgentStep, 'kind' | 'args' | 'title'>, t: T): string {
  // 1. Подпись от агента — уже человекочитаемая, выигрывает.
  const title = step.title?.trim();
  if (title) return truncate(title, TITLE_MAX);

  // 2. Шаблон по kind + args.
  const spec = ACT[step.kind];
  if (spec) {
    if (!spec.arg) return t(spec.bare); // think/plan — без аргумента
    const detail = argDetail(step.args, spec.arg);
    return detail ? t(spec.key, { q: detail }) : t(spec.bare);
  }

  // 3. Неизвестный/будущий инструмент — сырой kind (не падаем).
  return step.kind;
}
