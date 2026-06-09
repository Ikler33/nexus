#!/usr/bin/env node
// Per-module coverage-гейт (ратчет «не ниже», TESTING_STRATEGY §6 / AC-Q-2). Читает JSON-отчёт
// cargo-llvm-cov и проверяет, что покрытие критичных модулей (indexer/chunker/search/broker/permission/
// watcher/eval) и глобальное — НЕ ниже floor'ов из `coverage-baseline.json` (с допуском `tolerance` п.п.
// на macOS↔Linux/шум). Просело → красный CI с дельтой. Принцип «no silent caps»: печатаем ВСЕ модули и
// факт. %, даже зелёные. Zero-dep (только node:fs) — гоняется в CI без pnpm install.
//
//   node scripts/check-coverage.mjs <path-to-llvm-cov.json>
//
// Отчёт генерится: cargo llvm-cov --locked --workspace --json --output-path coverage.json
// (или разом локально: bash scripts/coverage.sh).

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

// Маппинг «имя модуля → подстрока пути» (имена совпадают с ключами modules в coverage-baseline.json).
const MODULE_PATHS = {
  indexer: '/src/indexer/',
  chunker: '/src/chunker/',
  search: '/src/search/',
  'plugin/broker': '/src/plugin/broker.rs',
  'plugin/permission': '/src/plugin/permission.rs',
  watcher: '/src/watcher',
  eval: '/src/eval/',
};

const reportPath = process.argv[2];
if (!reportPath) {
  console.error('Использование: node scripts/check-coverage.mjs <llvm-cov.json>');
  process.exit(2);
}

let baseline;
let report;
try {
  baseline = JSON.parse(readFileSync(resolve(root, 'coverage-baseline.json'), 'utf8'));
} catch (e) {
  console.error(`FATAL: не прочитать coverage-baseline.json: ${e.message}`);
  process.exit(2);
}
try {
  report = JSON.parse(readFileSync(resolve(reportPath), 'utf8'));
} catch (e) {
  console.error(`FATAL: не прочитать отчёт ${reportPath}: ${e.message}`);
  process.exit(2);
}

const data = report?.data?.[0];
if (!data?.files || !data?.totals) {
  console.error('FATAL: неожиданный формат llvm-cov JSON (нет data[0].files/totals)');
  process.exit(2);
}

const tol = baseline.tolerance ?? 0.5;
const errors = [];
const rows = [];

// Глобальное покрытие строк.
const globalPct = data.totals.lines.percent;
rows.push(['(global)', globalPct, baseline.global]);
if (globalPct < baseline.global - tol) {
  errors.push(
    `(global): ${globalPct.toFixed(1)}% < floor ${baseline.global}% (допуск ${tol}). Покрытие просело.`,
  );
}

// Агрегация по модулям: суммируем covered/count по файлам, попавшим в путь модуля.
for (const [name, sub] of Object.entries(MODULE_PATHS)) {
  const floor = baseline.modules?.[name];
  if (floor == null) {
    errors.push(`Модуль "${name}" есть в скрипте, но НЕТ floor'а в coverage-baseline.json`);
    continue;
  }
  let covered = 0;
  let count = 0;
  for (const f of data.files) {
    if (f.filename.includes(sub)) {
      covered += f.summary.lines.covered;
      count += f.summary.lines.count;
    }
  }
  if (count === 0) {
    errors.push(`Модуль "${name}" (путь "${sub}") не нашёл ни файла — путь устарел/модуль переименован?`);
    rows.push([name, null, floor]);
    continue;
  }
  const pct = (covered / count) * 100;
  rows.push([name, pct, floor]);
  if (pct < floor - tol) {
    errors.push(
      `${name}: ${pct.toFixed(1)}% < floor ${floor}% (допуск ${tol}). Добавь тесты или (осознанно) понизь floor.`,
    );
  }
}

// Сводка (no silent caps): печатаем все модули.
const target = baseline._target ?? 70;
console.log(`Coverage (lines), floor из coverage-baseline.json, цель критичных ${target}%:`);
for (const [name, pct, floor] of rows) {
  const shown = pct == null ? ' n/a ' : `${pct.toFixed(1)}%`.padStart(6);
  const mark = pct == null ? '⚠' : pct < floor - tol ? '❌' : pct < target ? '·' : '✓';
  console.log(`  ${mark} ${name.padEnd(20)} ${shown}  (floor ${floor}%)`);
}

if (errors.length) {
  console.error(`\n❌ Coverage-гейт: ${errors.length} проблем(ы):`);
  for (const e of errors) console.error(`  - ${e}`);
  console.error(
    '\nРатчет «не ниже»: новый код без тестов роняет покрытие → CI красный. Покрой или обнови floor осознанно.',
  );
  process.exit(1);
}
console.log('\n✅ Coverage-гейт пройден (все модули ≥ floor).');
