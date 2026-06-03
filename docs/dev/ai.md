# AI-слой: эмбеддер (`src-tauri/src/ai`)

> Срез Ф1-3 (§4.3, **ADR-005**). Раздельные Chat/Embedding провайдеры (разные хосты/модели).
> Chat-провайдер + стриминг — Ф1-7.

## EmbeddingProvider (ADR-005)
Трейт: `embed_documents` / `embed_query` (асимметрия query/document), `dim()` (ИЗ модели, не
хардкод — §6.5), `model_id()` (для инвалидации векторов при смене модели).
- **`OpenAiEmbedder`**: `POST {base}/v1/embeddings` (llama.cpp-server, OpenAI-совместимый).
  Применяет task-префиксы (nomic: `search_query:` / `search_document:`) + L2-нормализацию
  (`l2_normalize`, идемпотентна). Проверка размерности → `AiError::DimMismatch`.
- **`MockEmbedder`** (`#[cfg(test)]`): детерминированный вектор из байт текста (тесты без сервера).

## Конфиг (`config.rs`)
`LocalConfig` ← `.nexus/local.json` (НЕ в git, ADR-002): `ai.chat {url, model, context_window}`,
`ai.embedding {url, model, dim}`. Толерантен к частичным/неизвестным полям.

## Серверы (dev-окружение)
- **embedding (RAG) `127.0.0.1:8083` — bge-m3 Q4_K_M, dim 1024, МУЛЬТИЯЗЫЧНЫЙ** (Ф1-12, основной).
  `default_prefixes("bge-m3")` → без префиксов (dense-ретрив bge-m3 симметричен).
- embedding `127.0.0.1:8081` — nomic-embed-text-v1.5 Q8, dim 768 (англоцентричный, исходный — оставлен).
- code-embedding `127.0.0.1:8082` — jina-embeddings-v2-base-code (для кода; не общий мультиязычный).
- chat `192.168.0.172:8080` — Qwen (Ф1-7).
- **РИСК ADR-005 СНЯТ (Ф1-12):** nomic был англоцентричен → кросс-язычный RAG проседал (eval recall@8
  0.8, 2 кросс-язычных промаха). Переключение на **bge-m3** закрыло **AC-EVAL-6** (recall@8 → 1.0, оба
  кросс-язычных кейса найдены). Смена модели прошла через переэмбеддизацию (§6.5), `dim` 768→1024.
  `.nexus/local.json`-пример: `ai.embedding {"url":"http://127.0.0.1:8083","model":"bge-m3","dim":1024}`.

## Тесты
`l2_normalize` (unit-norm, нулевой вектор), `MockEmbedder` (детерминизм/нормализация), парсинг
`local.json`; **живой smoke** nomic `:8081` (`#[ignore]`, `cargo test -- --ignored`) — dim 768,
семантический ранкинг (запрос про кошку ближе к doc про кошку, чем к физике). ✓ проверено вживую.

## Дальше
- usearch ANN (Ф1-4); индексация по чанкам + batch + семафор + переэмбеддизация (Ф1-5);
  ChatProvider + per-session стриминг (Ф1-7). Реальный токенайзер чанкера — из этого эмбеддера.
