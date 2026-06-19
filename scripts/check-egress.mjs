#!/usr/bin/env node
// CI-grep-линт egress-chokepoint ядра (AC-EGR-1, ADR-005-ext): голый `reqwest::Client::builder` /
// `core_client_builder` ВНЕ `net/` запрещён — весь исходящий HTTP ядра обязан идти через
// `net::GuardedClient`. WHITELIST: (а) сам `net/` (зовёт приватизированный билдер); (б) `dispatch_net`
// (commands/plugin.rs — plugin net.fetch со СВОЕЙ политикой; миграция вне скоупа) — ТОЛЬКО с
// комментарием-маркером «egress-lint: allow» рядом с вызовом. Плюс AC-EGR-8: `fn is_private_host`
// определён ровно ОДИН раз (net/ импортирует ре-экспорт, не копию). Zero-dep (node:fs) — как
// check-ignored.mjs, гоняется в CI без pnpm install. Скрипт самопроверяется фейк-нарушением
// (AC-EGR-1: «тест добавляет фейк-нарушение → линт падает») перед сканом дерева.

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
    `is_private_host один (${files.length} .rs-файлов).`
);
