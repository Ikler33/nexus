# RAG-чат: провайдер + стриминг (`src-tauri/src/ai/chat.rs` + `commands/chat.rs`)

> Срез Ф1-7 (§4.1, §4.3, **ADR-005**). Chat-провайдер отдельен от эмбеддера (другой хост/модель).
> Стриминг через Tauri `Channel` (§4.1). Закрывает транспорт для **AC-Б10** и backend AC-DOD-Ф1.

## ChatProvider (ADR-005)
- `stream_chat(messages, on_token, cancel) -> String`: стримит ответ, каждую дельту отдаёт в
  `on_token` (по значению `String` — обходит HRTB-лайфтайм `dyn FnMut` под `async_trait`), копит и
  возвращает полный текст. Прерывание — флаг `cancel: Arc<AtomicBool>` (проверяется на каждом чанке).
- `OpenAiChatProvider`: `POST {base}/v1/chat/completions`, `stream: true`. Поток читаем
  `Response::chunk()` (без фичи `stream`/`futures-util` — **новых зависимостей нет**): копим байты,
  режем по `\n` (ASCII-граница не рвёт UTF-8), каждую строку `data: …` → `parse_sse_delta`. `[DONE]`
  завершает. Клиент без общего timeout (стрим долгий), только connect-timeout.
- `build_rag_messages(question, contexts, marker)`: system (отвечать ТОЛЬКО по контексту, цитаты `[n]`,
  язык вопроса, не выдумывать) + user с пронумерованным контекстом. `contexts` = `(метка-источник, текст)`.
  **Анти-инъекция (AC-SEC-7, V4.3):** каждый фрагмент обёрнут случайным `marker` (`injection_marker` на
  `getrandom`, генерируется per-request командой `chat_rag`); система предупреждена, что текст между
  маркерами — ДАННЫЕ заметок, а НЕ инструкции. Неугадываемость маркера не даёт заметке «закрыть» блок
  и перехватить управление («ignore previous instructions» / поддельный `</note>` остаётся данными).

## Команда `chat_rag` (Channel-стрим)
`chat_rag(channel, question, k?, center?, grounded?)` (§4.1, поток событий в `Channel<ChatStreamEvent>`):
1. `Sources { sources }` — `search::hybrid_search` (Ф1-6) → найденные чанки (приходит первым).
2. `Token { text }` — дельты ответа модели (контекст = полное содержимое топ-`k` чанков через
   `search::fetch_chunk_contexts`, в порядке релевантности; `k` дефолт 8, clamp 1..20).
3. `Done { full }` (полный текст в историю) **или** `Error { message }`.

**Режим `grounded` (V4.4).** По умолчанию `true` — «по vault»: ретрив → источники → `build_rag_messages`.
При `grounded=false` — **общий чат**: ретрив НЕ выполняется (`hybrid_search` не вызывается), `Sources`
шлётся пустым (UI очищает прежние), промпт = `build_chat_messages` (system без vault-грунтинга + чистый
вопрос). Web-search/tool-use — НЕ здесь (требует ADR egress-контроля, BACKLOG).

Лок vault снимается ДО сетевых вызовов (эмбеддинг запроса + LLM-стрим не держат `RwLock`).
**Отмена:** `AppState::begin_chat` регистрирует токен (отменяя предыдущий стрим — UI ведёт один чат);
`chat_cancel` его взводит → `stream_chat` выходит с накопленным текстом. Один активный чат за раз.

## Конфиг и фронт
- `.nexus/local.json → ai.chat { url, model }` (ADR-005, не в git). `build_chat` в `open_vault`;
  `None`, если секции нет → команда вернёт ошибку «chat не сконфигурирован». Доступность сервера
  проверяется при первом стриме (не на открытии).
- Контракт: `tauriApi.chat.streamRag(question, onEvent, {k?, center?, grounded?}) -> cancelFn` (создаёт
  `Channel`, вешает `onmessage`, вызывает `chat_rag`; возвращает функцию отмены → `chat_cancel`). Вне
  Tauri — мок `mock/vault.streamChat` (в `grounded`-режиме sources → токены → done; в общем — пустые
  sources + прямой ответ; поддерживает отмену).

## Тесты
- Rust: `parse_sse_delta` (content/`[DONE]`/role-only/keep-alive/мусор), `build_rag_messages`
  (нумерация источников, вопрос, пустой контекст), **`build_chat_messages` (V4.4: общий — без грунтинга,
  чистый вопрос)**, **анти-инъекция (V4.3, AC-SEC-7): фрагменты обёрнуты маркером, инъекция-текст —
  данные внутри, система предупреждена; `injection_marker` случаен**. **Живой** (`#[ignore]`, Qwen :8080):
  стрим токенов, непустой ответ «Париж». ✓ вживую.
- Фронт: мок `streamChat` (порядок sources→token→done; отмена прекращает до done); **V4.4 стор —
  `grounded:true/false` прокидывается в `streamRag`; общий режим → ответ без источников; `setGrounded`
  игнорируется во время стрима.**

## UI чата (Ф1-8, фронт)
Правая панель (DESIGN §«AI Chat»; layout `FileTree | Editor | Chat`, колонка по `.withChat`).
- **`stores/chat.ts`** (`useChatStore`): лента `ChatMessage[]` (сессия в памяти), `streaming`, `send`/
  `stop`/`clear`. `send` пушит user + пустой assistant(streaming), затем `tauriApi.chat.streamRag`:
  `sources` → `sources` сообщения, `token` → дописывает `content`, `done`/`error` → финализация.
  Один стрим за раз; `stop` зовёт cancel-fn (→ backend `chat_cancel`). **V4.4:** `grounded` (дефолт
  `true`) + `setGrounded` (нельзя на лету во время стрима) → `send` прокидывает режим в `streamRag`.
- **Переключатель «По заметкам / Общий» (V4.4)** — сегмент над композером (`ChatView`, `role=radiogroup`),
  отключён во время стрима. «Общий» → ответ напрямую от модели без RAG (источников нет).
- **`components/chat/ChatPanel.tsx`**: пустое состояние-подсказка; лента (user/assistant); каретка при
  стриминге; кнопка **Стоп** во время стрима, иначе **Отправить** (Enter — отправка, Shift+Enter — перенос);
  источники — кликабельные (`→ openFile`); бейдж «локально»; очистка/закрытие. Контекст retrieval —
  открытый файл (`activePath` → `center`, граф-ранг).
- Интеграция: `ui.chatOpen` + команда `view.chat` (`mod+j`) + кнопка в шапке; i18n namespace `chat`.
- Тесты: стор (стрим через мок → ответ+источники, stop, clear, пустой ввод), панель (пустое состояние,
  рендер ответа + клик источника → `openFile`, Enter-отправка, disabled-кнопка). **Проверено в превью**:
  вопрос → стрим + источники → клик открывает файл. Закрывает **AC-DOD-Ф1** (ответ с источниками).

## Виртуализация ленты (сделано)
`ChatView` рендерит ленту через `@tanstack/react-virtual` (только видимые сообщения; высота переменная →
`measureElement`, `initialRect` для jsdom). **Умный автоскролл**: следим за низом только если пользователь
уже там (`atBottom`-ref по `onScroll`), иначе чтение истории не дёргается во время стрима. Свой вопрос →
снова следим. Проверено в превью (виртуализация прозрачна — выглядит как было).

**Throttling токенов (V2.4, AC-Б10-4):** стор (`stores/chat.ts`) не делает `set()` на каждый token-эвент
(O(токенов) ре-рендеров) — копит текст в буфер и применяет одним апдейтом на кадр (`requestAnimationFrame`,
≤~60/сек). `done`/`error`/`stop` синхронно сбрасывают хвост буфера (токены не теряются даже без срабатывания
кадра). Тест считает rAF-вызовы: 200 токенов → 1 кадр.

## Дальше
- Индикатор «☁ облако» + cloud-fallback chat-only opt-in (ADR-005); сейчас бейдж всегда «локально».
- Персист истории сессий (`ChatSession`, AC-Б10-2 — миграция `chat_*`); граф-ранг уже включён (Ф1-6+).
- Reranker над контекстом (под eval-гейтом, Ф1-10); реальный токенайзер модели.
