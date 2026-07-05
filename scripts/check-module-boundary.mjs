#!/usr/bin/env node
// F-1b — negative-check границы «ядро/чужой-модуль ⇏ вырезанный модуль».
//
// ДОКАЗЫВАЕТ, что eslint-правило РЕАЛЬНО enforce'ит инвариант в CI (а не держится grep-ом в ревью,
// как до F-1b: F-10b-adversarial вскрыл именно это). Оракулы:
//   1) NEGATIVE «ядро»       — ВРЕМЕННЫЙ «ядровой» файл (src/lib/**, вне components/** и вне
//        connector/modules/**) с ЗАПРЕЩЁННЫМ импортом вырезанного модуля (`components/news` + его
//        манифест) → eslint ОБЯЗАН упасть (exit≠0). Прошёл бы — правило сломано.
//   2) NEGATIVE «laundering» — ВРЕМЕННЫЙ стрэй-файл ВНУТРИ `connector/modules/**` (НЕ манифест, НЕ в
//        MODULE_FEATURES) с импортом чужой зоны/манифеста → eslint ОБЯЗАН упасть. Это доказывает, что
//        floor-блок закрыл дыру coverage (adversarial F-1b): без floor стрэй-файл проваливался бы
//        сквозь все блоки = 0 правил → laundering в обход границы.
//   3) POSITIVE — линтим реальный `src/lib/connector/modules/**` (манифесты легитимно тянут СВОЮ
//        зону/манифест, тесты — свой манифест, index — все манифесты) → eslint чист по
//        no-restricted-imports (нет ложных срабатываний на существующем зелёном коде).
// Времянки всегда удаляются (finally). Zero-dep (node:*). Запускается из scripts/test-all.sh.

import { spawnSync } from 'node:child_process';
import { writeFileSync, rmSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const appDir = resolve(root, 'apps/desktop');
const eslintBin = resolve(appDir, 'node_modules/.bin/eslint');

function runEslint(targetRel) {
  const useBin = existsSync(eslintBin);
  const cmd = useBin ? eslintBin : 'pnpm';
  const args = useBin ? [targetRel] : ['exec', 'eslint', targetRel];
  const r = spawnSync(cmd, args, { cwd: appDir, encoding: 'utf8' });
  return { code: r.status, out: `${r.stdout || ''}${r.stderr || ''}` };
}

// Кладём probe → линтим → всегда удаляем. Ждём: exit≠0 + сообщение F-1b + оба ожидаемых бана.
function expectRejected(label, rel, source, expectRe) {
  const abs = resolve(appDir, rel);
  try {
    writeFileSync(abs, source);
    const r = runEslint(rel);
    const failed = r.code !== 0 && /F-1b/.test(r.out) && /no-restricted-imports/.test(r.out);
    const hits = expectRe.every((re) => re.test(r.out));
    if (failed && hits) {
      console.log(`  ✓ NEGATIVE ${label}: eslint отклонил запрещённый импорт (exit ${r.code})`);
      return true;
    }
    console.error(`  ✗ NEGATIVE ${label} ПРОВАЛЕН: правило НЕ ловит (exit ${r.code}).`);
    console.error(`    failed=${failed}  hits=${expectRe.map((re) => re.test(r.out)).join(',')}`);
    console.error(r.out.slice(0, 2000));
    return false;
  } finally {
    rmSync(abs, { force: true });
  }
}

let ok = true;

// 1) NEGATIVE «ядро»: src/lib/** → components/news + манифест (относит. путь без сегмента `lib`).
ok =
  expectRejected(
    'ядро→модуль',
    'src/lib/__f1b_boundary_probe.ts',
    [
      '// F-1b negative-check probe — ВРЕМЕННЫЙ файл (создаётся/удаляется check-module-boundary.mjs).',
      '// Ядровой путь с ЗАПРЕЩЁННЫМ импортом вырезанного модуля: eslint ОБЯЗАН его отклонить.',
      "import { NewsView } from '../components/news/NewsView';",
      "import { newsModule } from './connector/modules/news';",
      'export const _f1bProbe = [NewsView, newsModule];',
      '',
    ].join('\n'),
    [/components\/news.*import is restricted/, /connector\/modules\/news.*import is restricted/],
  ) && ok;

// 2) NEGATIVE «laundering»: стрэй-файл ВНУТРИ modules/ (не манифест) тянет чужую зону + манифест.
ok =
  expectRejected(
    'laundering (стрэй в modules/)',
    'src/lib/connector/modules/__f1b_stray_probe.ts',
    [
      '// F-1b negative-check probe — ВРЕМЕННЫЙ стрэй-файл (НЕ манифест, НЕ в MODULE_FEATURES).',
      '// Без floor-блока проваливался бы сквозь все блоки = 0 правил (laundering). floor ловит:',
      "import { GoalsPanel } from '../../../components/goals/GoalsPanel';",
      "import { goalsModule } from './goals';",
      'export const _laundering = [GoalsPanel, goalsModule];',
      '',
    ].join('\n'),
    [/components\/goals.*import is restricted/, /'\.\/goals' import is restricted/],
  ) && ok;

// 3) POSITIVE — реальные манифесты/тесты/index (легитимные self-импорты) НЕ должны ложно срабатывать.
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
