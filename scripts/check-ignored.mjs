#!/usr/bin/env node
// Гейт #[ignore]-тестов (анти-false-green, кросс-план #4(б)). Число #[ignore] в Rust должно
// совпадать с EXPECTED. Если тест ТИХО отключили (`#[ignore]`) — счётчик вырастет → красный CI,
// и автор обязан осознанно обновить EXPECTED (видимое решение, а не молчаливая дыра в покрытии).
// Zero-dep (node:fs) — гоняется в CI без pnpm install.

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

// Осознанно #[ignore]: живые-серверные (embedder/chat/eval) и keychain-роундтрип тесты. Менять — только
// вместе с объяснением, ПОЧЕМУ тест отключён (а не «чтобы CI позеленел»).
// 11→12: + `regen_eval_fixture` (разовая регенерация реальной eval-фикстуры на живом bge-m3; сам гейт
// качества `eval_fixture_meets_baseline` НЕ ignored — гоняется в CI на замороженных векторах).
// 12→16: + 4 live-smoke LLM-этапов (`live_smoke.rs`, 2026-06-11): news-этап (RU-резюме+сводка дня),
// web-агент целиком (план→SearXNG→ответ), decide «веб не нужен», чат-стрим 26B — всем нужны живые
// LLM-сервер/SearXNG, в CI принципиально не исполняются; запуск `cargo test live_ -- --ignored`.
const EXPECTED = 16;

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = resolve(root, 'apps/desktop/src-tauri/src');

const hits = [];
const walk = (dir) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) walk(full);
    else if (e.name.endsWith('.rs')) {
      readFileSync(full, 'utf8')
        .split('\n')
        .forEach((line, i) => {
          if (/#\[ignore\b/.test(line)) hits.push(`${full.slice(root.length + 1)}:${i + 1}`);
        });
    }
  }
};
walk(SRC);

console.log(`#[ignore]-тестов: ${hits.length} (ожидается ${EXPECTED})`);
if (hits.length !== EXPECTED) {
  console.error(`\n❌ Число #[ignore] изменилось (${hits.length} ≠ ${EXPECTED}).`);
  console.error('Осознанно? Обнови EXPECTED в scripts/check-ignored.mjs и опиши причину. Места:');
  for (const h of hits) console.error(`  - ${h}`);
  process.exit(1);
}
console.log('✅ #[ignore] под контролем (нет тихо отключённых тестов).');
