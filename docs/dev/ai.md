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
- embedding `127.0.0.1:8081` — **nomic-embed-text-v1.5 Q8**, dim **768**, `/v1/embeddings`, сервер сам L2-нормализует.
- chat `192.168.0.172:8080` — **Qwen3.6-27B** (для Ф1-7).
- **РИСК (ADR-005):** nomic — англоцентричная; спека требует МУЛЬТИЯЗЫЧНЫЙ эмбеддер (bge-m3 /
  multilingual-e5) под кросс-язычный RAG (AC-EVAL-6). Зафиксировано; `embedding.model`/`dim` пойдут
  в settings → смена на bge-m3 позже триггерит переэмбеддизацию (§6.5), код к этому готов.

## Тесты
`l2_normalize` (unit-norm, нулевой вектор), `MockEmbedder` (детерминизм/нормализация), парсинг
`local.json`; **живой smoke** nomic `:8081` (`#[ignore]`, `cargo test -- --ignored`) — dim 768,
семантический ранкинг (запрос про кошку ближе к doc про кошку, чем к физике). ✓ проверено вживую.

## Дальше
- usearch ANN (Ф1-4); индексация по чанкам + batch + семафор + переэмбеддизация (Ф1-5);
  ChatProvider + per-session стриминг (Ф1-7). Реальный токенайзер чанкера — из этого эмбеддера.
