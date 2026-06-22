#!/usr/bin/env node
// CI-grep-линт инверсии §5.2 «host РЕШАЕТ, контейнер ИСПОЛНЯЕТ» (SANDBOX-6c-2, INV-CMD-SITE):
// конструкция process-`Command` для exec-команды агента разрешена ТОЛЬКО в sandbox/exec_child.rs
// (исполнитель ВНУТРИ `--network=none` контейнера). Любой `std::process::Command` / `tokio::process::Command`
// / `Command::new` в ЛЮБОМ ДРУГОМ host-sandbox-модуле (exec_host/act/runner/child/event/provider/proxy/mod)
// → красный CI: host НИКОГДА не должен спавнить команду агента (иначе джейлбрейк-команда бежала бы с
// полными правами хоста мимо kernel-EROFS/ENETUNREACH/cap-deny песочницы).
//
// ИСКЛЮЧЕНИЯ: (а) exec_child.rs целиком — легитимный in-container исполнитель; (б) runner.rs запускает САМ
// podman (запуск песочницы, не команды агента) — строка помечена маркером «sandbox-exec-lint: allow
// podman-launch». Zero-dep (node:fs), как check-egress.mjs; самопроверяется фейк-нарушением перед сканом.

import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SANDBOX_DIR = resolve(root, 'crates/nexus-core/src/sandbox');

// Запрещённые конструкции (совпадение в КОДЕ; хвост после `//` отрезается, чтобы упоминания в
// комментариях/доках не давали ложных срабатываний). `process::Command\b` ловит и std:: и tokio::-формы
// конструктора; `\b` после `Command` ИСКЛЮЧАЕТ `process::CommandExt` (трейт хардненинга `.pre_exec()/.uid()`
// — НЕ спавн), но матчит `process::Command::new`/`process::Command;`/`process::Command {`. `Command::new`
// ловит вызов через `use ...::Command;`-импорт.
const FORBIDDEN = [/process::Command\b/, /\bCommand::new\b/];
// Файл-исключение целиком (относительный путь внутри sandbox/): in-container исполнитель.
const WHOLE_FILE_EXEMPT = new Set(['exec_child.rs']);
// Маркер обоснованного построчного исключения (только запуск САМОГО podman в runner.rs).
const ALLOW_MARKER = 'sandbox-exec-lint: allow';

/**
 * Сканирует список файлов `{path, text}` (path — относительно sandbox/, с '/').
 * Возвращает массив строк-нарушений.
 */
function scan(files) {
  const violations = [];
  for (const { path, text } of files) {
    if (WHOLE_FILE_EXEMPT.has(path)) continue; // (а) in-container исполнитель
    const lines = text.split('\n');
    lines.forEach((raw, i) => {
      const code = raw.split('//')[0];
      if (!FORBIDDEN.some((re) => re.test(code))) return;
      // Маркер ищем на самой строке и до 3 строк выше (многострочное обоснование).
      const near = lines.slice(Math.max(0, i - 3), i + 1).join('\n');
      if (near.includes(ALLOW_MARKER)) return; // (б) marked podman-launch
      violations.push(`${path}:${i + 1}: ${raw.trim()}`);
    });
  }
  return violations;
}

// ── Самопроверка детектора (фейк-нарушения): линт обязан ловить и не давать ложных пропусков ──
const selftest = scan([
  // Нарушения (host-модули, без маркера):
  { path: 'exec_host.rs', text: 'let c = std::process::Command::new("x");' },
  { path: 'runner.rs', text: 'let c = tokio::process::Command::new("y");' },
  { path: 'child.rs', text: 'use tokio::process::Command;\nlet c = Command::new("z");' },
  // Легально:
  { path: 'exec_child.rs', text: 'let c = tokio::process::Command::new("real");' }, // (а) whole-file exempt
  { path: 'runner.rs', text: '// sandbox-exec-lint: allow podman-launch\ntokio::process::Command::new("podman");' }, // (б) marker
  { path: 'mod.rs', text: '//! упоминание process::Command в доке — не код' }, // комментарий
  // CommandExt (трейт хардненинга `.pre_exec()/.uid()`, НЕ спавн) НЕ ловится (word-boundary): 0 отсюда.
  { path: 'event.rs', text: 'use std::os::unix::process::CommandExt;' },
]);
// Ожидаем РОВНО 4 нарушения: exec_host (1) + runner-без-маркера (1) + child (use-строка + Command::new = 2);
// exec_child whole-file-exempt (0), marked podman (0), doc-комментарий (0), CommandExt-импорт (0 — \b).
// Пересчёт: 1+1+2 = 4.
if (selftest.length !== 4) {
  console.error('❌ self-test линта провалился: детектор не ловит фейк-нарушения (INV-CMD-SITE).');
  console.error(`   нарушений: ${selftest.length} (ожидалось 4):`);
  for (const v of selftest) console.error(`   - ${v}`);
  process.exit(2);
}

// ── Реальный скан sandbox-дерева ──
const files = [];
const walk = (dir) => {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) walk(full);
    else if (e.name.endsWith('.rs')) {
      files.push({
        path: full.slice(SANDBOX_DIR.length + 1).split('\\').join('/'),
        text: readFileSync(full, 'utf8'),
      });
    }
  }
};
walk(SANDBOX_DIR);

const violations = scan(files);
if (violations.length > 0) {
  console.error('❌ check-sandbox-exec (INV-CMD-SITE, §5.2 инверсия):');
  console.error(
    '   process-Command для команды агента разрешён ТОЛЬКО в sandbox/exec_child.rs (in-container ' +
      'исполнитель). host НИКОГДА не спавнит команду агента. Для запуска САМОГО podman — маркер ' +
      `«${ALLOW_MARKER} podman-launch». Места:`
  );
  for (const v of violations) console.error(`   - ${v}`);
  process.exit(1);
}
console.log(
  `✅ INV-CMD-SITE цел: process::Command для exec агента только в exec_child.rs ` +
    `(+ marked podman-launch); ${files.length} sandbox-файлов.`
);
