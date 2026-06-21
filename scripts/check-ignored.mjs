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
// 16→17: + `live_eval_llm_rerank_experiment` (eval-гейт LLM-реранка на живых bge+E4B; прод-гейт
// качества по-прежнему `eval_fixture_meets_baseline` в CI на замороженных векторах).
// 17→19: + `live_chat_memory_recall_end_to_end` (живая проверка N4: gemma вспоминает факт из
// прошлой сессии через врезку памяти) + `bench_local_pipeline_scale` (cold-bench #19: масштаб
// локального пайплайна на моке без сети — ловит O(N²) в индексации; запускать через NEXUS_BENCH_FILES).
// 19→20: + `live_classify_tags_meets_gate` (AI-2c: реальный chat_util Qwen3-4B :8084 классифицирует
// `tag_golden.json` closed-vocab → eval-гейт `out_of_vocab==0 && precision≥0.8/recall≥0.5`; в CI не
// исполняется — нужен живой LLM; запуск `NEXUS_FAST_URL=… cargo test live_classify_tags_meets_gate -- --ignored`).
// 20→21: + `live_consolidation_meets_gate` (MEM-8c-a: реальный consolidate::decide основной модели :8080
// по `consolidation_eval.json` → гейт DELETE-precision≥0.9/UPDATE-quality≥0.8 разблокирует авто-DELETE;
// нужен живой LLM; запуск `NEXUS_CHAT_URL=… cargo test live_consolidation_meets_gate -- --ignored`).
// 21→22: + `live_episode_summary_meets_gate` (EP-2: реальный episode::summarize модели саммари по
// `episode_eval.json` → гейт faithfulness≥0.85 разблокирует ретривал эпизодов в чат; нужен живой LLM;
// запуск `NEXUS_CHAT_URL=… cargo test live_episode_summary_meets_gate -- --ignored`).
// 22→23: + `live_tokenizer_matches_server` (P0-c: встроенный QwenTokenizer == /tokenize задеплоенной
// модели Qwen3.6-27B на golden-строках; нужен живой сервер :8080; офлайн-гейт качества —
// `ai::tokenizer::tests::embedded_matches_deployed_model_counts` (в CI на встроенном ассете, без сети);
// запуск `NEXUS_CHAT_URL=… cargo test live_tokenizer_matches_server -- --ignored`).
// 23→24: + `live_tool_call_smoke` (AGENT-1 адверсариал-ревью C: ПОЛНЫЙ run_agent_loop против живой
// tool-capable модели :8080 — модель эмитит tool_call → цикл исполняет EchoTool → feed-back в СТРОГОЙ
// OpenAI-форме (assistant{tool_calls}+tool{tool_call_id}, Part A) → Final; валидирует реальную форму
// протокола end-to-end; нужен живой LLM; запуск `NEXUS_CHAT_URL=… cargo test live_tool_call_smoke -- --ignored`).
// 24→25: + `live_connect_tool_loop_on_rig` (AGENT-CONNECT P0b-2b: ConnectAgentHandler драйвит реальную
// tool-capable модель :8080 ЧЕРЕЗ протокол коннектора — agent/run → run_agent_session → agent/event-стрим
// (toolCall→final); валидирует коннектор end-to-end на живой модели; офлайн-гейт — 4 теста
// `agent::connect::handler::tests::*` на фейк-провайдере; запуск `NEXUS_LIVE_CHAT=1 cargo test
// live_connect_tool_loop_on_rig -- --ignored`).
// 25→26: + `live_actuator_create_and_undo_on_rig` (AGENT-CONNECT валидация: реальная модель :8080 создаёт
// заметку через ГЕЙТ актуатора [autonomy=auto → Auto-тир apply] → файл на диске → undo_run восстанавливает;
// полный стек вживую модель→tool-call→dispatch_action→apply→undo; нужна tool-capable модель; запуск
// `NEXUS_LIVE_CHAT=1 cargo test live_actuator_create_and_undo_on_rig -- --ignored`).
// 26→27: + `live_agent_web_search_on_rig` (EGR-AGENT: реальная модель :8080 исследует веб через web.search
// → GuardedClient(EgressFeature::Web, allowlist) → SearXNG на VPS :8888 → результаты → финальный ответ;
// нужны SearXNG + tool-capable модель; запуск `NEXUS_LIVE_CHAT=1 cargo test live_agent_web_search_on_rig
// -- --ignored`).
const EXPECTED = 27;

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
// CORE-1: часть #[ignore]-тестов (live-серверные ai/chat, ai/embedder) переехала в crates/nexus-core/src
// вместе с модулями. Считаем по ОБОИМ деревьям, чтобы суммарный счётчик (EXPECTED) не «потерял» их при
// извлечении ядра (иначе тихая дыра в покрытии прошла бы как зелёный CI — ровно то, от чего этот гейт).
const SRC_ROOTS = [
  resolve(root, 'apps/desktop/src-tauri/src'),
  resolve(root, 'crates/nexus-core/src'),
];

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
for (const src of SRC_ROOTS) walk(src);

console.log(`#[ignore]-тестов: ${hits.length} (ожидается ${EXPECTED})`);
if (hits.length !== EXPECTED) {
  console.error(`\n❌ Число #[ignore] изменилось (${hits.length} ≠ ${EXPECTED}).`);
  console.error('Осознанно? Обнови EXPECTED в scripts/check-ignored.mjs и опиши причину. Места:');
  for (const h of hits) console.error(`  - ${h}`);
  process.exit(1);
}
console.log('✅ #[ignore] под контролем (нет тихо отключённых тестов).');
