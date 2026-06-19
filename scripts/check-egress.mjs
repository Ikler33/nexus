#!/usr/bin/env node
// CI-grep-линт egress-chokepoint ядра (AC-EGR-1, ADR-005-ext): голый `reqwest::Client::builder` /
// `core_client_builder` ВНЕ `net/` запрещён — весь исходящий HTTP ядра обязан идти через
// `net::GuardedClient`. WHITELIST: (а) сам `net/` (зовёт приватизированный билдер); (б) `dispatch_net`
// (commands/plugin.rs — plugin net.fetch со СВОЕЙ политикой; миграция вне скоупа) — ТОЛЬКО с
// комментарием-маркером «egress-lint: allow» рядом с вызовом. Плюс AC-EGR-8: `fn is_private_host`
// определён ровно ОДИН раз (net/ импортирует ре-экспорт, не копию). Zero-dep (node:fs) — как
// check-ignored.mjs, гоняется в CI без pnpm install. Скрипт самопроверяется фейк-нарушением
// (AC-EGR-1: «тест добавляет фейк-нарушение → линт падает») перед сканом дерева.
//
// AGENT-3a (RunCtx): дополнительно проверяет, что egress-точки входа `net/mod.rs`
// (`record`/`authorize`/`get`/`post_json`) несут параметр `RunCtx` — процесс-глобального run_id-слота
// больше нет, корреляция эгресса с прогоном агента идёт ЯВНЫМ per-call контекстом; ни один будущий
// egress-путь не должен молча ронять её. Тоже zero-dep + самотестируется (good/bad фейк-сигнатуры).

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
// CORE-1: `net/` (core_client_builder) и `plugin/` (is_private_host) переехали в crates/nexus-core/src.
// Сканируем ОБА дерева, чтобы chokepoint-инвариант и единственность is_private_host продолжали
// покрывать перемещённый код. Пути нормализуем ОТНОСИТЕЛЬНО каждого корня (а не общего root), чтобы
// whitelist `net/` и маркер в `commands/plugin.rs` (остался в app) совпадали по тем же относительным
// путям, что и до извлечения ядра.
const SRC_ROOTS = [
  resolve(root, 'apps/desktop/src-tauri/src'),
  resolve(root, 'crates/nexus-core/src'),
  // CORE-2a: headless agent-service. Должен конструировать НОЛЬ сырых HTTP-клиентов (использует
  // только `GuardedClient` из ядра) — сканируем его, чтобы любой будущий raw-reqwest здесь поймал линт.
  resolve(root, 'crates/nexus-agentd/src'),
];

// Запрещённые конструкторы клиента (совпадение в КОДЕ; хвост строки после `//` отрезается,
// чтобы упоминания в комментариях/доках не давали ложных срабатываний).
const FORBIDDEN = [/reqwest::Client::builder/, /core_client_builder/];
// Маркер обоснованного исключения; честен ТОЛЬКО в файлах из WHITELIST_MARKER.
const ALLOW_MARKER = 'egress-lint: allow';
const WHITELIST_MARKER = new Set(['commands/plugin.rs']);

// AGENT-3a (RunCtx-корреляция): публичные точки входа эгресса И приватный audit-`record` ОБЯЗАНЫ нести
// run-контекст-параметр (`ctx: RunCtx`), чтобы никакой будущий egress-путь молча не «ронял» корреляцию
// эгресса с прогоном агента. Проверяем по сигнатуре `fn <name>(` в `net/mod.rs`: между объявлением и его
// `)` (закрытием списка параметров) должно встретиться `RunCtx`. Чисто текстовый grep-инвариант (zero-dep).
const RUNCTX_FNS = ['record', 'authorize', 'get', 'post_json'];

/**
 * Проверяет, что каждая из `RUNCTX_FNS` в тексте `net/mod.rs` несёт `RunCtx` в списке параметров.
 * Возвращает { missing: string[], checked: string[] } — `missing` для функций без RunCtx или ненайденных.
 * Многострочные сигнатуры: от строки с `fn <name>(` до первой строки, закрывающей список параметров
 * (`)` на нужном уровне; берём упрощённо — до строки, содержащей `) ->` или `) {` либо одиночный `)`).
 */
function checkRunCtxParams(text) {
  const lines = text.split('\n');
  const missing = [];
  const checked = [];
  for (const name of RUNCTX_FNS) {
    // Ищем объявление `fn <name>(` или `async fn <name>(` (в КОДЕ, не в комментарии).
    const declRe = new RegExp(`\\bfn\\s+${name}\\s*\\(`);
    let found = false;
    for (let i = 0; i < lines.length; i++) {
      if (!declRe.test(lines[i].split('//')[0])) continue;
      found = true;
      checked.push(name);
      // Собираем текст сигнатуры до закрытия списка параметров.
      let sig = '';
      for (let j = i; j < lines.length && j < i + 30; j++) {
        const code = lines[j].split('//')[0];
        sig += code + '\n';
        // Конец списка параметров: строка с `)` (для наших сигнатур — `) ->`/`) {`/`)`).
        if (/\)\s*(->|\{|$)/.test(code.trimEnd())) break;
      }
      if (!/\bRunCtx\b/.test(sig)) {
        missing.push(`net/mod.rs: fn ${name}(...) без параметра RunCtx (AGENT-3a: корреляция эгресса)`);
      }
      break; // первое объявление каждой fn (их по одному в net/mod.rs вне тестов)
    }
    if (!found) {
      missing.push(`net/mod.rs: не найдено объявление fn ${name}( (RUNCTX_FNS — egress chokepoint)`);
    }
  }
  return { missing, checked };
}

/**
 * Сканирует список файлов `{path, text}` (path — относительно src/, с '/').
 * Возвращает { violations: string[], privateHostDefs: string[] }.
 */
function scan(files) {
  const violations = [];
  const privateHostDefs = [];
  for (const { path, text } of files) {
    const inNet = path === 'net.rs' || path.startsWith('net/');
    const lines = text.split('\n');
    lines.forEach((raw, i) => {
      if (/\bfn is_private_host\b/.test(raw.split('//')[0])) {
        privateHostDefs.push(`${path}:${i + 1}`);
      }
      const code = raw.split('//')[0];
      if (!FORBIDDEN.some((re) => re.test(code))) return;
      if (inNet) return; // (а) сам net/ — единственный дом билдера
      // Маркер ищем на самой строке и до 3 строк выше (многострочный комментарий-обоснование).
      const near = lines.slice(Math.max(0, i - 3), i + 1).join('\n');
      if (WHITELIST_MARKER.has(path) && near.includes(ALLOW_MARKER)) return; // (б) dispatch_net
      violations.push(`${path}:${i + 1}: ${raw.trim()}`);
    });
  }
  return { violations, privateHostDefs };
}

// ── Самопроверка детектора (фейк-нарушения): линт обязан ловить и не давать ложных пропусков ──
const selftest = scan([
  { path: 'commands/evil.rs', text: 'let c = reqwest::Client::builder().build();' },
  { path: 'ai/sneaky.rs', text: 'let b = core_client_builder();' },
  // Маркер вне whitelist-файла НЕ освобождает.
  { path: 'commands/evil2.rs', text: '// egress-lint: allow\nlet c = reqwest::Client::builder();' },
  // net/ и комментарии — легальны.
  { path: 'net/mod.rs', text: 'reqwest::Client::builder().redirect(none)' },
  { path: 'ai/mod.rs', text: '//! `core_client_builder` — приватная деталь net/' },
  // Дубль is_private_host должен быть виден счётчику.
  { path: 'plugin/permission.rs', text: 'pub fn is_private_host(h: &str) -> bool {' },
  { path: 'net/dup.rs', text: 'fn is_private_host(h: &str) -> bool {' },
]);
if (selftest.violations.length !== 3 || selftest.privateHostDefs.length !== 2) {
  console.error('❌ self-test линта провалился: детектор не ловит фейк-нарушения (AC-EGR-1).');
  console.error(`   нарушений: ${selftest.violations.length} (ожидалось 3):`);
  for (const v of selftest.violations) console.error(`   - ${v}`);
  process.exit(2);
}

// ── Самопроверка RunCtx-детектора (AGENT-3a): фейк-net/mod.rs без RunCtx обязан дать N нарушений ──
const runCtxSelftestBad = checkRunCtxParams(
  // record/authorize/post_json БЕЗ RunCtx (нарушение); get С RunCtx (ОК).
  'async fn record(&self, feature: F, host: String) {}\n' +
    'async fn authorize(&self, url: &str) -> X {}\n' +
    'pub async fn get(&self, url: &str, ctx: RunCtx) -> R {}\n' +
    'pub async fn post_json(&self, url: &str, body: &V) -> R {}\n'
);
// 3 без RunCtx (record/authorize/post_json) → 3 missing; get с RunCtx → не в missing.
if (runCtxSelftestBad.missing.length !== 3) {
  console.error('❌ self-test RunCtx-детектора провалился: ожидалось 3 нарушения (AGENT-3a).');
  for (const m of runCtxSelftestBad.missing) console.error(`   - ${m}`);
  process.exit(2);
}
const runCtxSelftestGood = checkRunCtxParams(
  'async fn record(&self, f: F, ctx: RunCtx) {}\n' +
    'async fn authorize(&self, url: &str, ctx: RunCtx) -> X {}\n' +
    'pub async fn get(&self, url: &str, ctx: RunCtx) -> R {}\n' +
    'pub async fn post_json(&self, url: &str, body: &V, ctx: RunCtx) -> R {}\n'
);
if (runCtxSelftestGood.missing.length !== 0) {
  console.error('❌ self-test RunCtx-детектора: чистый случай дал ложные нарушения (AGENT-3a).');
  for (const m of runCtxSelftestGood.missing) console.error(`   - ${m}`);
  process.exit(2);
}

// ── Реальный скан дерева ──
const files = [];
const walk = (dir, srcRoot) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) walk(full, srcRoot);
    else if (e.name.endsWith('.rs')) {
      files.push({
        path: full.slice(srcRoot.length + 1).split('\\').join('/'),
        text: readFileSync(full, 'utf8'),
      });
    }
  }
};
for (const srcRoot of SRC_ROOTS) walk(srcRoot, srcRoot);

const { violations, privateHostDefs } = scan(files);
const errors = [];

// AGENT-3a: net/mod.rs egress-точки входа обязаны нести RunCtx (корреляция эгресса с прогоном).
const netModFile = files.find((f) => f.path === 'net/mod.rs');
if (!netModFile) {
  errors.push('AGENT-3a: net/mod.rs не найден в дереве — невозможно проверить RunCtx-инвариант.');
} else {
  const { missing } = checkRunCtxParams(netModFile.text);
  if (missing.length > 0) {
    errors.push(
      'AGENT-3a: egress-точки входа ОБЯЗАНЫ нести RunCtx (per-call корреляция эгресса с прогоном; ' +
        'процесс-глобального run_id-слота больше нет — ни один путь не должен молча ронять корреляцию):',
      ...missing.map((m) => `  - ${m}`)
    );
  }
}

if (violations.length > 0) {
  errors.push(
    'Голое построение HTTP-клиента ядра вне net/ (AC-EGR-1). Эгресс ядра — ТОЛЬКО через ' +
      'net::GuardedClient; для plugin net.fetch (dispatch_net) — маркер «egress-lint: allow» ' +
      'с обоснованием. Места:',
    ...violations.map((v) => `  - ${v}`)
  );
}
if (privateHostDefs.length !== 1) {
  errors.push(
    `fn is_private_host определён ${privateHostDefs.length} раз(а), ожидается ровно 1 (AC-EGR-8: ` +
      'одна правда SSRF-логики; net/ импортирует ре-экспорт plugin::is_private_host, не копию):',
    ...privateHostDefs.map((d) => `  - ${d}`)
  );
}

if (errors.length > 0) {
  console.error('❌ check-egress:');
  for (const e of errors) console.error(e);
  process.exit(1);
}
console.log(
  `✅ egress-chokepoint цел: билдеры клиента только в net/ (+ dispatch_net с обоснованием), ` +
    `is_private_host один, egress-точки входа несут RunCtx (${files.length} .rs-файлов).`
);
