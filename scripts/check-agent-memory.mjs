#!/usr/bin/env node
// CI-grep-линт воронки записи памяти агента (AGENT-MEM-1): автономная запись памяти агента — ТОЛЬКО
// Add (`memory::add`), ТОЛЬКО через адаптер `agent/memory.rs` (`VaultAgentMemory::remember`). Цикл
// агента / хендлер / инструменты НЕ должны звать сырой `memory::add` — иначе обходят Add-only воронку
// (и завтра кто-то так же позовёт `memory::update`/`delete` = автономная консолидация, отвергнутая:
// это ГЕЙТЕД эпик MEM, вне скоупа агента).
//
// Правило: в модуле `agent/` упоминание `memory::add` (в КОДЕ, не комментарии) разрешено РОВНО в
// `agent/memory.rs`. Где угодно ещё под `agent/` → красный CI. НЕ баним `memory::add` глобально —
// эпик MEM (`memory/`, `commands/memory.rs`, консолидация) зовёт его легально; скоуп линта = `agent/`.
//
// Сканируем оба rust-дерева (ядро + headless agentd), как check-egress/check-tooluse. Zero-dep
// (node:fs); самопроверяется фейк-нарушениями ДО скана дерева.

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SRC_ROOTS = [
  resolve(root, 'apps/desktop/src-tauri/src'),
  resolve(root, 'crates/nexus-core/src'),
  resolve(root, 'crates/nexus-agentd/src'),
];

// Дом воронки: единственный файл под agent/, где сырой `memory::add` легален (это и есть адаптер).
const FUNNEL_FILE = 'agent/memory.rs';
// Запрещённый-вне-дома вызов записи памяти в модуле agent/. Ловим И `memory::add`, И любые
// мутирующие `memory::update*`/`memory::delete*`/`memory::supersede*` (упреждаем будущий обход
// воронки в сторону консолидации). Хвост после `//` отрезаем (комментарии не считаются).
const FORBIDDEN = /\bmemory::(add|update|delete|supersede|unindex_fact)\w*\s*\(/;

/** Путь под модулем agent/ ядра? (нормализован относительно src-корня, разделитель '/'). */
function inAgentModule(path) {
  return path === 'agent.rs' || path.startsWith('agent/');
}

/**
 * Сканирует список файлов `{path, text}` (path — относительно src/, с '/').
 * Возвращает массив нарушений (строки `path:line: код`).
 */
function scan(files) {
  const violations = [];
  for (const { path, text } of files) {
    if (!inAgentModule(path)) continue; // линт скоупится на agent/
    if (path === FUNNEL_FILE) continue; // дом воронки — здесь memory::add легален
    text.split('\n').forEach((raw, i) => {
      const code = raw.split('//')[0];
      if (FORBIDDEN.test(code)) violations.push(`${path}:${i + 1}: ${raw.trim()}`);
    });
  }
  return violations;
}

// ── Самопроверка детектора (фейк-нарушения): линт обязан ловить и не давать ложных пропусков ──
const selftest = scan([
  // Сырой add в цикле агента (вне дома воронки) — нарушение.
  { path: 'agent/runner.rs', text: 'let r = memory::add(writer, text, "agent").await;' },
  // Мутирующий вызов в хендлере — нарушение (упреждаем обход в сторону консолидации).
  { path: 'agent/job.rs', text: 'memory::delete(writer, id).await?;' },
  // Дом воронки — легально (НЕ нарушение).
  { path: 'agent/memory.rs', text: 'crate::memory::add(&self.writer, text, SOURCE_AGENT).await' },
  // Упоминание в КОММЕНТАРИИ — не нарушение.
  { path: 'agent/runner.rs', text: '// раньше тут был memory::add(...) — теперь через remember' },
  // memory::add ВНЕ agent/ (эпик MEM) — легально, линт его не трогает.
  { path: 'commands/memory.rs', text: 'memory::add(w, t, "explicit").await' },
  { path: 'memory/extract.rs', text: 'memory::add(w, t, "auto").await' },
]);
if (selftest.length !== 2) {
  console.error('❌ self-test линта провалился: детектор не ловит фейк-нарушения (AGENT-MEM-1).');
  console.error(`   нарушений: ${selftest.length} (ожидалось 2):`);
  for (const v of selftest) console.error(`   - ${v}`);
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

const violations = scan(files);
if (violations.length > 0) {
  console.error('❌ check-agent-memory:');
  console.error(
    'Сырая запись памяти в модуле agent/ вне воронки (AGENT-MEM-1). Память агента пишется ТОЛЬКО ' +
      `через AgentMemory::remember (адаптер ${FUNNEL_FILE}); update/delete/supersede агенту запрещены ` +
      '(консолидация — гейтед эпик MEM). Места:'
  );
  for (const v of violations) console.error(`  - ${v}`);
  process.exit(1);
}
console.log(
  `✅ воронка памяти агента цела: memory::add/мутации в agent/ только в ${FUNNEL_FILE} ` +
    `(${files.length} .rs-файлов).`
);
