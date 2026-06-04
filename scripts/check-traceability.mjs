#!/usr/bin/env node
// Traceability-гейт (V1.3, TESTING_STRATEGY §4): каждый AC из ACCEPTANCE.md обязан иметь запись в
// матрице docs/acceptance/traceability.json со статусом и (для covered/partial) ссылками на тесты.
// Падает, если: AC из спеки отсутствует в матрице (новый AC без теста), запись-сирота (нет в спеке),
// неизвестный статус, или covered/partial без тестов. Принцип «no silent caps»: pending/manual/deferred
// допустимы, но видимы в сводке. Zero-dep (только node:fs) — гоняется без pnpm install.

import { readFileSync } from 'node:fs';
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
