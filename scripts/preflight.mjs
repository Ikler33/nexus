#!/usr/bin/env node
// Предполётная гигиена рабочего дерева (кросс-план #2). Zero-dep.
//
// Ищет iCloud-каталоги синк-конфликтов вида «<имя> 2» (пустые тени реальных каталогов),
// которые загрязняют каждый grep/find и искажают навигацию. Они не отслеживаются git
// (см. .gitignore «* 2/»), поэтому CI их не видит — это ЛОКАЛЬНЫЙ чек, первый шаг среза:
//   node scripts/preflight.mjs
// Exit 0 — чисто; exit 1 — найдены тени (вывод списка). Можно `--fix` для удаления пустых.

import { readdirSync, statSync, rmSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = process.cwd();
const SKIP = new Set(['node_modules', 'target', 'dist', '.git', 'gen', 'coverage']);
const fix = process.argv.includes('--fix');

/** Рекурсивно собирает каталоги, чьё имя оканчивается на « 2» (пробел + 2). */
function findShadows(dir, out) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const e of entries) {
    if (!e.isDirectory() || SKIP.has(e.name)) continue;
    const full = join(dir, e.name);
    if (/ 2$/.test(e.name)) {
      out.push(full);
      continue; // не спускаемся внутрь тени
    }
    findShadows(full, out);
  }
}

/** Число файлов в поддереве (для безопасного удаления только пустых). */
function fileCount(dir) {
  let n = 0;
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    n += e.isDirectory() ? fileCount(join(dir, e.name)) : 1;
  }
  return n;
}

const shadows = [];
findShadows(ROOT, shadows);

if (shadows.length === 0) {
  console.log('✅ preflight: теневых « 2»-каталогов нет.');
  process.exit(0);
}

console.error(`⚠️  preflight: найдено теневых каталогов: ${shadows.length}`);
let removed = 0;
for (const d of shadows) {
  const files = fileCount(d);
  const rel = d.slice(ROOT.length + 1);
  if (fix && files === 0) {
    rmSync(d, { recursive: true, force: true });
    removed++;
    console.error(`  removed (пустой): ${rel}`);
  } else {
    console.error(`  ${rel}${files ? ` — ⚠️ НЕ пуст (${files} файлов), удалять вручную` : ' (пуст)'}`);
  }
}
if (fix) {
  console.error(`Удалено пустых: ${removed}. ${shadows.length - removed} осталось.`);
  process.exit(shadows.length - removed === 0 ? 0 : 1);
}
console.error('Запусти `node scripts/preflight.mjs --fix` для удаления пустых.');
process.exit(1);
