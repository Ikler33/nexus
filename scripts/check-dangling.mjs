#!/usr/bin/env node
// Линт висячих упоминаний СНЯТЫХ решений (AC-Q-6): sqlite-vec как ANN (снят в пользу usearch,
// ADR-003*), petgraph-источник графа (снят, ADR-004), wasmtime-рантайм (отложен, AC-Б11-2/3),
// `currentFile` (заменён моделью групп/вкладок, Б12). Два скоупа:
//
// 1. КОД (`apps/desktop/src`, `src-tauri/src`): термин в НЕ-комментарии → красный CI всегда
//    (вернуть снятое решение в код можно только через ADR + правку этого линта). Комментарии —
//    «пояснительный контекст» (напр. «НЕ petgraph, ADR-004») — разрешены.
// 2. ДОКИ (ARCHITECTURE/ACCEPTANCE/dev/specs/design/BACKLOG): per-file СЧЁТЧИКИ заморожены в
//    INVENTORY (паттерн EXPECTED из check-ignored.mjs). Новое упоминание (или новый файл) →
//    красный CI, автор осознанно обновляет инвентарь. Исторические тексты (docs/reviews/,
//    ARCHITECTURE-v1.0-backup.md, CHANGELOG, NIGHT-PLAN) — вне скоупа: это архив анализов/журнал.
//
// Zero-dep (node:fs); self-test фейк-нарушениями перед сканом (как check-egress.mjs).

import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

const TERMS = [
  { key: 'sqlite-vec', re: /sqlite[-_]vec/gi },
  { key: 'petgraph', re: /petgraph/gi },
  { key: 'wasmtime', re: /wasmtime/gi },
  { key: 'currentFile', re: /\bcurrentFile\b/g },
];

// Замороженный инвентарь доков: file → { term: ожидаемое число упоминаний }.
// Менять ОСОЗНАННО вместе с правкой дока (новое упоминание снятого решения — только пояснительное).
const INVENTORY = {
  'docs/architecture/ARCHITECTURE.md': { 'sqlite-vec': 3, petgraph: 6, wasmtime: 2, currentFile: 4 },
  'docs/acceptance/ACCEPTANCE.md': { 'sqlite-vec': 1, petgraph: 1, currentFile: 1 },
  'docs/dev/TESTING_STRATEGY.md': { 'sqlite-vec': 1, petgraph: 1, currentFile: 1 },
  'docs/dev/graph.md': { petgraph: 4 },
  'docs/dev/workspace.md': { currentFile: 1 },
};

// Доки в скоупе инвентаря (рекурсивно по .md), минус исторические.
const DOC_DIRS = ['docs/architecture', 'docs/acceptance', 'docs/dev', 'docs/specs', 'docs/design'];
const DOC_FILES = ['docs/BACKLOG.md'];
const DOC_EXCLUDE = new Set(['docs/architecture/ARCHITECTURE-v1.0-backup.md']);
const CODE_DIRS = ['apps/desktop/src', 'apps/desktop/src-tauri/src'];
const CODE_EXT = /\.(rs|ts|tsx)$/;

/** Убирает комментарии (// … и /* … *​/), чтобы код-скоуп ловил только «живые» упоминания. */
function stripComments(text) {
  return text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

function countMatches(text, re) {
  return (text.match(re) ?? []).length;
}

/** Сканирует наборы файлов; возвращает список нарушений (строки для вывода). */
function scan(docFiles, codeFiles, inventory) {
  const errors = [];
  const seen = new Set();
  for (const { path, text } of docFiles) {
    for (const t of TERMS) {
      const n = countMatches(text, t.re);
      const expected = inventory[path]?.[t.key] ?? 0;
      if (n !== expected) {
        errors.push(
          `${path}: «${t.key}» ×${n} (инвентарь: ${expected}) — новое упоминание снятого решения? ` +
            `Поясни контекст и обнови INVENTORY в scripts/check-dangling.mjs`
        );
      }
      if (n > 0) seen.add(`${path}|${t.key}`);
    }
  }
  // Запись инвентаря без файла/термина — протухла (файл переименован/упоминания сняты).
  for (const [path, terms] of Object.entries(inventory)) {
    for (const key of Object.keys(terms)) {
      if (!seen.has(`${path}|${key}`)) {
        errors.push(`инвентарь устарел: ${path} больше не упоминает «${key}» — убери запись`);
      }
    }
  }
  for (const { path, text } of codeFiles) {
    const code = stripComments(text);
    for (const t of TERMS) {
      if (t.re.test(code)) {
        t.re.lastIndex = 0;
        errors.push(`${path}: «${t.key}» в КОДЕ вне комментария — снятое решение (AC-Q-6)`);
      }
      t.re.lastIndex = 0;
    }
  }
  return errors;
}

// ── Self-test детектора (фейк-нарушения) ──
const st = scan(
  [
    { path: 'docs/dev/fake.md', text: 'будем хранить векторы в sqlite-vec' }, // новый файл → 0 ожид.
    { path: 'docs/dev/graph.md', text: 'petgraph petgraph' }, // 2 ≠ 4 → поймать
  ],
  [
    { path: 'src/bad.rs', text: 'use petgraph::Graph; // граф' }, // код вне комментария
    { path: 'src/ok.rs', text: '// беклинки из SQLite, НЕ petgraph (ADR-004)\nfn f() {}' }, // ок
    { path: 'src/ok2.ts', text: '/** вместо одиночного currentFile */ const x = 1;' }, // ок
  ],
  { 'docs/dev/graph.md': { petgraph: 4 } }
);
if (st.length !== 3) {
  console.error(`❌ self-test check-dangling провалился: ${st.length} нарушений (ожидалось 3):`);
  for (const e of st) console.error(`  - ${e}`);
  process.exit(2);
}

// ── Реальный скан ──
const rel = (full) => full.slice(root.length + 1).split('\\').join('/');
const docFiles = [];
const walkDocs = (dir) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) walkDocs(full);
    else if (e.name.endsWith('.md') && !DOC_EXCLUDE.has(rel(full))) {
      docFiles.push({ path: rel(full), text: readFileSync(full, 'utf8') });
    }
  }
};
for (const d of DOC_DIRS) if (existsSync(resolve(root, d))) walkDocs(resolve(root, d));
for (const f of DOC_FILES) {
  if (existsSync(resolve(root, f))) docFiles.push({ path: f, text: readFileSync(resolve(root, f), 'utf8') });
}
const codeFiles = [];
const walkCode = (dir) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) walkCode(full);
    else if (CODE_EXT.test(e.name)) codeFiles.push({ path: rel(full), text: readFileSync(full, 'utf8') });
  }
};
for (const d of CODE_DIRS) if (existsSync(resolve(root, d))) walkCode(resolve(root, d));

const errors = scan(docFiles, codeFiles, INVENTORY);
if (errors.length > 0) {
  console.error('❌ check-dangling (AC-Q-6): висячие упоминания снятых решений:');
  for (const e of errors) console.error(`  - ${e}`);
  process.exit(1);
}
console.log(
  `✅ снятые решения не всплывают: ${docFiles.length} doc-файлов по инвентарю, ` +
    `${codeFiles.length} код-файлов чисто (AC-Q-6).`
);
