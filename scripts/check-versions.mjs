#!/usr/bin/env node
// Гейт синхронизации версии (кросс-план #7). Версия приложения дублируется в 4 местах — если
// бампнуть одно и забыть остальные, билд/crash-отчёт/updater сообщат рассинхрон. Здесь — ЕДИНАЯ
// проверка, что все 4 совпадают. Номер бампится release-срезом (M-β3 → 0.1.0).
// Zero-dep (node:fs) — гоняется в CI без pnpm install.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const json = (p) => JSON.parse(readFileSync(resolve(root, p), 'utf8')).version;
// Версия из [workspace.package] корневого Cargo.toml (первая строка `version = "..."`).
const cargoWs = (readFileSync(resolve(root, 'Cargo.toml'), 'utf8').match(/^\s*version\s*=\s*"([^"]+)"/m) ??
  [])[1];

const sources = {
  'package.json': json('package.json'),
  'apps/desktop/package.json': json('apps/desktop/package.json'),
  'apps/desktop/src-tauri/tauri.conf.json': json('apps/desktop/src-tauri/tauri.conf.json'),
  'Cargo.toml [workspace.package]': cargoWs,
};

console.log('Версия приложения по источникам:');
for (const [k, v] of Object.entries(sources)) console.log(`  ${(v ?? '??').padEnd(12)} ${k}`);

const distinct = [...new Set(Object.values(sources))];
if (distinct.length !== 1 || !distinct[0]) {
  console.error('\n❌ Версии рассинхронизированы (или не найдены) — должны совпадать во всех 4 источниках.');
  process.exit(1);
}
console.log(`\n✅ Версия консистентна: ${distinct[0]}`);
