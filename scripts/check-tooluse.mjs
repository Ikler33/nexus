#!/usr/bin/env node
// CI-grep-линт I-5 (ADR-005/ADR-009): tool-calling НЕ должен протекать в chat/web/news/websearch путь.
// Тип `OpenAiToolProvider` (tool-capable провайдер) разрешён ТОЛЬКО под:
//   (а) `ai/tools.rs` — его дом (определение + тесты этого же файла);
//   (б) `agent/…` — слой цикла агента;
//   (в) `nexus-agentd/…` — композиционный корень, единственный конструирующий его;
//   (г) тест-код (`#[cfg(test)]` / `*tests*.rs`) — мокам/тестам можно ссылаться.
// Любое упоминание `OpenAiToolProvider` ВНЕ этого whitelist (особенно в chat.rs/web/news/websearch
// модулях) → красный CI: значит tool-провайдер просочился в обычный chat/web-путь (отвергнутый I-5).
// Zero-dep (node:fs) — как check-egress.mjs; самопроверяется фейк-нарушениями перед сканом дерева.

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
// Сканируем те же деревья, что check-egress: оба крейта + headless agentd. Пути нормализуем
// ОТНОСИТЕЛЬНО каждого корня (а не общего root), чтобы whitelist по относительным путям совпадал.
const SRC_ROOTS = [
  resolve(root, 'apps/desktop/src-tauri/src'),
  resolve(root, 'crates/nexus-core/src'),
  resolve(root, 'crates/nexus-agentd/src'),
];

// Запрещённый-вне-whitelist символ (совпадение в КОДЕ; хвост строки после `//` отрезается, чтобы
// упоминания в комментариях/доках не давали ложных срабатываний).
const FORBIDDEN = /\bOpenAiToolProvider\b/;

/**
 * Путь принадлежит whitelist'у (где `OpenAiToolProvider` легален)?
 * - `ai/tools.rs` (дом типа) · любой файл под `agent/` · любой файл крейта `nexus-agentd`.
 * `agentdSrc` — относительный путь уже из дерева nexus-agentd (все его файлы разрешены).
 */
function isWhitelisted(path, agentdSrc) {
  if (agentdSrc) return true; // (в) весь nexus-agentd — корень, конструирующий провайдер
  if (path === 'ai/tools.rs') return true; // (а) дом типа
  if (path === 'agent.rs' || path.startsWith('agent/')) return true; // (б) слой агента
  return false;
}

/**
 * Файл — тест-код? `#[cfg(test)]` где-либо в файле ИЛИ имя вида `*tests*.rs` / в каталоге `tests/`.
 * Тестам/мокам можно ссылаться на провайдер вне whitelist (напр. интеграционный тест).
 */
function isTestFile(path, text) {
  if (/(^|\/)tests?\//.test(path) || /tests?\.rs$/.test(path)) return true;
  return /#\[cfg\(test\)\]/.test(text);
}

/**
 * Сканирует список файлов `{path, text, agentd}` (path — относительно src/, с '/').
 * Возвращает массив строк-нарушений.
 */
function scan(files) {
  const violations = [];
  for (const { path, text, agentd } of files) {
    if (isWhitelisted(path, agentd)) continue;
    if (isTestFile(path, text)) continue;
    text.split('\n').forEach((raw, i) => {
      const code = raw.split('//')[0];
      if (FORBIDDEN.test(code)) violations.push(`${path}:${i + 1}: ${raw.trim()}`);
    });
  }
  return violations;
}

// ── Самопроверка детектора (фейк-нарушения): линт обязан ловить и не давать ложных пропусков ──
const selftest = scan([
  // Просочился в chat-путь — НАРУШЕНИЕ.
  { path: 'ai/chat.rs', text: 'let p = OpenAiToolProvider::new(&c, f, u, m, None);', agentd: false },
  // Просочился в web-модуль — НАРУШЕНИЕ.
  { path: 'commands/websearch.rs', text: 'use crate::ai::tools::OpenAiToolProvider;', agentd: false },
  // Легально: дом типа.
  { path: 'ai/tools.rs', text: 'pub struct OpenAiToolProvider { … }', agentd: false },
  // Легально: слой агента.
  { path: 'agent/runner.rs', text: '// связан с OpenAiToolProvider через трейт', agentd: false },
  // Легально: nexus-agentd (корень).
  { path: 'main.rs', text: 'let p = OpenAiToolProvider::new(...);', agentd: true },
  // Легально: тест-код вне whitelist.
  { path: 'ai/chat.rs', text: '#[cfg(test)]\nmod t { use OpenAiToolProvider; }', agentd: false },
  // Комментарий вне кода — не нарушение.
  { path: 'commands/news.rs', text: '// НЕ используем OpenAiToolProvider тут', agentd: false },
]);
if (selftest.length !== 2) {
  console.error('❌ self-test линта провалился: детектор не ловит фейк-нарушения (I-5).');
  console.error(`   нарушений: ${selftest.length} (ожидалось 2):`);
  for (const v of selftest) console.error(`   - ${v}`);
  process.exit(2);
}

// ── Реальный скан дерева ──
const files = [];
const walk = (dir, srcRoot, agentd) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) walk(full, srcRoot, agentd);
    else if (e.name.endsWith('.rs')) {
      files.push({
        path: full.slice(srcRoot.length + 1).split('\\').join('/'),
        text: readFileSync(full, 'utf8'),
        agentd,
      });
    }
  }
};
for (const srcRoot of SRC_ROOTS) {
  walk(srcRoot, srcRoot, srcRoot.endsWith('nexus-agentd/src'));
}

const violations = scan(files);
if (violations.length > 0) {
  console.error('❌ check-tooluse:');
  console.error(
    'Тип OpenAiToolProvider (tool-capable провайдер) просочился ВНЕ дозволенного (I-5/ADR-005): ' +
      'разрешён только в ai/tools.rs, agent/, nexus-agentd/ и тест-коде. Tool-calling не должен ' +
      'протекать в chat/web/news/websearch путь. Места:'
  );
  for (const v of violations) console.error(`  - ${v}`);
  process.exit(1);
}
console.log(
  `✅ I-5 цел: OpenAiToolProvider только в ai/tools.rs + agent/ + nexus-agentd/ (+ тесты); ` +
    `tool-calling не протекает в chat/web (${files.length} .rs-файлов).`
);
