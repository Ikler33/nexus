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
- `build_rag_messages(question, contexts)`: system (отвечать ТОЛЬКО по контексту, цитаты `[n]`, язык
  вопроса, не выдумывать) + user с пронумерованным контекстом. `contexts` = `(метка-источник, текст)`.

## Команда `chat_rag` (Channel-стрим)
`chat_rag(channel, question, k?)` (§4.1, поток событий в `Channel<ChatStreamEvent>`):
1. `Sources { sources }` — `search::hybrid_search` (Ф1-6) → найденные чанки (приходит первым).
2. `Token { text }` — дельты ответа модели (контекст = полное содержимое топ-`k` чанков через
   `search::fetch_chunk_contexts`, в порядке релевантности; `k` дефолт 8, clamp 1..20).
3. `Done { full }` (полный текст в историю) **или** `Error { message }`.

Лок vault снимается ДО сетевых вызовов (эмбеддинг запроса + LLM-стрим не держат `RwLock`).
**Отмена:** `AppState::begin_chat` регистрирует токен (отменяя предыдущий стрим — UI ведёт один чат);
`chat_cancel` его взводит → `stream_chat` выходит с накопленным текстом. Один активный чат за раз.

## Конфиг и фронт
- `.nexus/local.json → ai.chat { url, model }` (ADR-005, не в git). `build_chat` в `open_vault`;
  `None`, если секции нет → команда вернёт ошибку «chat не сконфигурирован». Доступность сервера
  проверяется при первом стриме (не на открытии).
- Контракт: `tauriApi.chat.streamRag(question, onEvent, {k?}) -> cancelFn` (создаёт `Channel`, вешает
  `onmessage`, вызывает `chat_rag`; возвращает функцию отмены → `chat_cancel`). Вне Tauri — мок
  `mock/vault.streamChat` (sources → токены по словам → done; поддерживает отмену).

## Тесты
- Rust: `parse_sse_delta` (content/`[DONE]`/role-only/keep-alive/мусор), `build_rag_messages`
  (нумерация источников, вопрос, пустой контекст). **Живой** (`#[ignore]`, Qwen :8080): стрим токенов,
  непустой ответ «Париж». ✓ проверено вживую.
- Фронт: мок `streamChat` (порядок sources→token→done; отмена прекращает до done).

## UI чата (Ф1-8, фронт)
Правая панель (DESIGN §«AI Chat»; layout `FileTree | Editor | Chat`, колонка по `.withChat`).
- **`stores/chat.ts`** (`useChatStore`): лента `ChatMessage[]` (сессия в памяти), `streaming`, `send`/
  `stop`/`clear`. `send` пушит user + пустой assistant(streaming), затем `tauriApi.chat.streamRag`:
  `sources` → `sources` сообщения, `token` → дописывает `content`, `done`/`error` → финализация.
  Один стрим за раз; `stop` зовёт cancel-fn (→ backend `chat_cancel`).
- **`components/chat/ChatPanel.tsx`**: пустое состояние-подсказка; лента (user/assistant); каретка при
  стриминге; кнопка **Стоп** во время стрима, иначе **Отправить** (Enter — отправка, Shift+Enter — перенос);
  источники — кликабельные (`→ openFile`); бейдж «локально»; очистка/закрытие. Контекст retrieval —
  открытый файл (`activePath` → `center`, граф-ранг).
- Интеграция: `ui.chatOpen` + команда `view.chat` (`mod+j`) + кнопка в шапке; i18n namespace `chat`.
- Тесты: стор (стрим через мок → ответ+источники, stop, clear, пустой ввод), панель (пустое состояние,
  рендер ответа + клик источника → `openFile`, Enter-отправка, disabled-кнопка). **Проверено в превью**:
  вопрос → стрим + источники → клик открывает файл. Закрывает **AC-DOD-Ф1** (ответ с источниками).

## Дальше
- **Виртуализация ленты чата** (DESIGN §«лента виртуализирована») — при длинной истории; сейчас простой скролл.
- Индикатор «☁ облако» + cloud-fallback chat-only opt-in (ADR-005); сейчас бейдж всегда «локально».
- Throttling рендера токенов; персист истории сессий (`ChatSession`); граф-ранг уже включён (Ф1-6+).
- Reranker над контекстом (под eval-гейтом, Ф1-10); реальный токенайзер модели.
