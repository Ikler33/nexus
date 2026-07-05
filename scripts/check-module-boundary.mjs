#!/usr/bin/env node
// F-1b — negative-check границы «ядро/чужой-модуль ⇏ вырезанный модуль».
//
// ДОКАЗЫВАЕТ, что eslint-правило РЕАЛЬНО enforce'ит инвариант в CI (а не держится grep-ом в ревью,
// как до F-1b: F-10b-adversarial вскрыл именно это). Два шага-оракула:
//   1) NEGATIVE — кладём ВРЕМЕННЫЙ «ядровой» файл (src/lib/**, вне components/** и вне
//      connector/modules/**) с ЗАПРЕЩЁННЫМ импортом вырезанного модуля (`components/news` + его
//      манифест) → eslint ОБЯЗАН упасть (exit≠0) с сообщением F-1b. Прошёл бы — правило сломано.
//   2) POSITIVE — линтим реальный `src/lib/connector/modules/**` (манифесты легитимно тянут СВОЮ
//      зону/манифест, тесты — свой манифест) → eslint чист по no-restricted-imports (нет ложных
//      срабатываний границы на существующем зелёном коде).
// Времянка всегда удаляется (finally). Zero-dep (node:*). Запускается из scripts/test-all.sh.

import { spawnSync } from 'node:child_process';
import { writeFileSync, rmSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const appDir = resolve(root, 'apps/desktop');
const eslintBin = resolve(appDir, 'node_modules/.bin/eslint');
const probeRel = 'src/lib/__f1b_boundary_probe.ts';
const probeAbs = resolve(appDir, probeRel);

// Запрещённый импорт из «ядрового» пути src/lib/**: относительно него зона фичи = ../components/news,
// а манифест = ./connector/modules/news (сегмента `lib` в пути НЕТ — покрыто паттерном без `lib/`).
const PROBE = [
  '// F-1b negative-check probe — ВРЕМЕННЫЙ файл (создаётся/удаляется scripts/check-module-boundary.mjs).',
  '// Ядровой путь с ЗАПРЕЩЁННЫМ импортом вырезанного модуля: eslint ОБЯЗАН его отклонить.',
  "import { NewsView } from '../components/news/NewsView';",
  "import { newsModule } from './connector/modules/news';",
  'export const _f1bProbe = [NewsView, newsModule];',
  '',
].join('\n');

function runEslint(targetRel) {
  const useBin = existsSync(eslintBin);
  const cmd = useBin ? eslintBin : 'pnpm';
  const args = useBin ? [targetRel] : ['exec', 'eslint', targetRel];
  const r = spawnSync(cmd, args, { cwd: appDir, encoding: 'utf8' });
  return { code: r.status, out: `${r.stdout || ''}${r.stderr || ''}` };
}

let ok = true;

// 1) NEGATIVE — правило ДОЛЖНО отклонить запрещённый импорт (доказательство enforcement).
try {
  writeFileSync(probeAbs, PROBE);
  const neg = runEslint(probeRel);
  const failedAsExpected = neg.code !== 0 && /F-1b/.test(neg.out) && /no-restricted-imports/.test(neg.out);
  const hitComponent = /components\/news.*import is restricted/.test(neg.out);
  const hitManifest = /connector\/modules\/news.*import is restricted/.test(neg.out);
  if (failedAsExpected && hitComponent && hitManifest) {
    console.log(`  ✓ NEGATIVE: eslint отклонил ядро→components/news И ядро→manifest (exit ${neg.code})`);
  } else {
    ok = false;
    console.error(`  ✗ NEGATIVE ПРОВАЛЕН: правило НЕ enforce'ит границу (exit ${neg.code}).`);
    console.error(`    component-ban=${hitComponent}  manifest-ban=${hitManifest}`);
    console.error(neg.out.slice(0, 2000));
  }
} finally {
  rmSync(probeAbs, { force: true });
}

// 2) POSITIVE — реальные манифесты (легитимные self-импорты) НЕ должны ложно срабатывать.
const pos = runEslint('src/lib/connector/modules');
if (pos.code === 0 && !/no-restricted-imports/.test(pos.out)) {
  console.log('  ✓ POSITIVE: src/lib/connector/modules чист (границы без ложных срабатываний)');
} else {
  ok = false;
  console.error('  ✗ POSITIVE ПРОВАЛЕН: правило даёт ложное срабатывание на легитимном коде.');
  console.error(pos.out.slice(0, 2000));
}

if (!ok) {
  console.error('\n✗ F-1b negative-check ПРОВАЛЕН — граница модуль/ядро НЕ доказана в CI.');
  process.exit(1);
}
console.log("✅ F-1b negative-check пройден: граница модуль/ядро enforce'ится eslint-ом (не grep-ом).");
