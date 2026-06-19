#!/usr/bin/env node
// Traceability-гейт (V1.3, TESTING_STRATEGY §4): каждый AC из ACCEPTANCE.md обязан иметь запись в
// матрице docs/acceptance/traceability.json со статусом и (для covered/partial) ссылками на тесты.
// Падает, если: AC из спеки отсутствует в матрице (новый AC без теста), запись-сирота (нет в спеке),
// неизвестный статус, или covered/partial без тестов. Принцип «no silent caps»: pending/manual/deferred
// допустимы, но видимы в сводке. Zero-dep (только node:fs) — гоняется без pnpm install.

import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ACCEPTANCE = resolve(root, 'docs/acceptance/ACCEPTANCE.md');
const MATRIX = resolve(root, 'docs/acceptance/traceability.json');

const VALID_STATUS = ['covered', 'partial', 'pending', 'manual', 'deferred'];
// AC-ID: Latin/Cyrillic алфавит+цифры (AC-Б1-1, AC-EVAL-3, AC-DOD-Ф0, AC-I18N-1).
const AC_RE = /AC-[A-ZА-Я0-9]+-[A-ZА-Я0-9]+/gu;

const errors = [];

// 1. AC из спеки
const specText = readFileSync(ACCEPTANCE, 'utf8');
const specAcs = new Set(specText.match(AC_RE) ?? []);
if (specAcs.size === 0) errors.push('ACCEPTANCE.md: не найдено ни одного AC-ID (сломан парсер?)');

// 2. Матрица
let matrix;
try {
  matrix = JSON.parse(readFileSync(MATRIX, 'utf8'));
} catch (e) {
  console.error(`FATAL: не прочитать/распарсить ${MATRIX}: ${e.message}`);
  process.exit(2);
}
const acs = matrix.acs ?? {};
const matrixIds = new Set(Object.keys(acs));

// 3. Полнота: каждый AC спеки — в матрице
for (const id of specAcs) {
  if (!matrixIds.has(id)) errors.push(`AC ${id} есть в ACCEPTANCE.md, но НЕТ в traceability.json (новый AC без записи о тесте)`);
}
// 4. Сироты: каждая запись матрицы — реальный AC
for (const id of matrixIds) {
  if (!specAcs.has(id)) errors.push(`Запись ${id} в traceability.json не соответствует ни одному AC в ACCEPTANCE.md (опечатка/устарело)`);
}
// 5-6. Валидность записей
const counts = Object.fromEntries(VALID_STATUS.map((s) => [s, 0]));
for (const [id, e] of Object.entries(acs)) {
  if (!VALID_STATUS.includes(e.status)) {
    errors.push(`AC ${id}: неизвестный статус "${e.status}" (допустимо: ${VALID_STATUS.join('/')})`);
    continue;
  }
  counts[e.status]++;
  if ((e.status === 'covered' || e.status === 'partial') && !(Array.isArray(e.tests) && e.tests.length > 0)) {
    errors.push(`AC ${id}: статус ${e.status}, но не указаны tests[]`);
  }
}

// 6b. Существование имён tests[] (анти-false-green, кросс-план #4(а)): rust-тест-модуль реально есть
//     (есть `mod tests`), фронт-тест-файл существует. CI-описания/нераспознанное — пропускаем.
function rustTestModules(srcDir) {
  const set = new Set();
  const walk = (dir, prefix) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const full = resolve(dir, e.name);
      if (e.isDirectory()) {
        walk(full, [...prefix, e.name]);
      } else if (e.name.endsWith('.rs') && /\bmod tests\b/.test(readFileSync(full, 'utf8'))) {
        const stem = e.name.replace(/\.rs$/, '');
        const modPath =
          stem === 'lib' || stem === 'main' ? [] : stem === 'mod' ? prefix : [...prefix, stem];
        set.add([...modPath, 'tests'].join('::'));
      }
    }
  };
  walk(srcDir, []);
  return set;
}
// CORE-1: модули db/parser/vector/plugin/vault/redact/chunker/net/ai (с их `mod tests`) переехали в
// crates/nexus-core/src. traceability.json ссылается на их тест-модули (net::tests, vault::tests,
// ai::chat::tests, …) — собираем имена тест-модулей из ОБОИХ деревьев. Имена модулей считаются
// относительно каждого src-корня (как и до извлечения: `net::tests`, не `nexus_core::net::tests`),
// поэтому существующие записи матрицы продолжают совпадать без правки.
const RUST_SRCS = [
  resolve(root, 'apps/desktop/src-tauri/src'),
  resolve(root, 'crates/nexus-core/src'),
];
const FE_ROOT = resolve(root, 'apps/desktop');
const rustMods = new Set();
for (const src of RUST_SRCS) {
  if (existsSync(src)) for (const m of rustTestModules(src)) rustMods.add(m);
}
for (const [id, e] of Object.entries(acs)) {
  for (const raw of Array.isArray(e.tests) ? e.tests : []) {
    const ref = String(raw).trim();
    if (ref.startsWith('CI:') || ref.startsWith('CI ')) continue; // CI-описания, не имена тестов
    const head = ref.split(/[\s(]/)[0]; // отрезаем примечания/парентетику
    if (/\.test\.tsx?$/.test(head)) {
      if (!existsSync(resolve(FE_ROOT, head))) errors.push(`AC ${id}: фронт-тест "${head}" не найден`);
      continue;
    }
    const m = head.match(/^(.+?::tests)(::.+)?$/);
    const modTests = m ? m[1] : /^tests(::.+)?$/.test(head) ? 'tests' : null;
    if (modTests && !rustMods.has(modTests)) {
      errors.push(`AC ${id}: тест-модуль "${modTests}" (из "${ref}") не найден в src (нет mod tests)`);
    }
  }
}

// 7. Сводка
const total = specAcs.size;
console.log('Traceability AC ↔ тест:');
console.log(`  всего AC в спеке: ${total}, записей в матрице: ${matrixIds.size}`);
for (const s of VALID_STATUS) console.log(`  ${s.padEnd(9)}: ${counts[s]}`);
const notAuto = counts.pending + counts.manual + counts.deferred;
console.log(`  → автотестами покрыто (covered+partial): ${counts.covered + counts.partial}/${total}; вне автотестов: ${notAuto}`);

// 8. Вердикт
if (errors.length) {
  console.error(`\n❌ traceability: ${errors.length} проблем:`);
  for (const e of errors) console.error(`  - ${e}`);
  process.exit(1);
}
console.log('\n✅ traceability: матрица полна и согласована со спекой.');
