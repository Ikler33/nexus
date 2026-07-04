//! Chat-транспорт (**ADR-005**): [`ChatProvider`] + OpenAI-совместимый [`OpenAiChatProvider`].
//! Стриминг токенов из `POST /v1/chat/completions` (`stream: true`, SSE).
//!
//! Поток читаем `Response::chunk()` (без фичи `stream`/`futures`): копим байты, режем по `\n`,
//! каждую строку `data: …` парсим в дельту. Прерывание — флагом `cancel` (проверяется по чанкам).
//! Ретрай инициации (P0-d) + cold-start-aware таймауты первого токена/idle. Промпт-билдеры и
//! wire-типы сообщений — в `super::chat` (этот модуль их лишь ПОТРЕБЛЯЕТ через [`ChatMessage`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use super::chat::ChatMessage;
use super::{AiError, AiResult};
use crate::net::{EgressFeature, GuardedClient, RunCtx};

/// Провайдер чата (ADR-005). Стримит ответ модели токенами.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Стримит ответ: каждую текстовую дельту отдаёт в `on_token` (по значению — обходит HRTB-
    /// лайфтайм под `async_trait`), возвращает полный текст. При `cancel == true` — прекращает.
    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
    ) -> AiResult<String>;

    /// Как [`stream_chat`], но ДОПОЛНИТЕЛЬНО отдаёт «размышление» reasoning-модели (gemma) в
    /// `on_reasoning` — для живого 💭-индикатора чата (R1). Контент ответа идёт в `on_token`, возвращается
    /// тоже только контент (reasoning в результат НЕ попадает). Дефолт игнорирует reasoning (делегирует
    /// в `stream_chat`) → моки и не-чат вызыватели (inline/дайджест/судья) НЕ трогаются. Реальный
    /// провайдер переопределяет.
    async fn stream_chat_reasoning(
        &self,
        messages: &[ChatMessage],
        on_token: &mut (dyn FnMut(String) + Send),
        on_reasoning: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
    ) -> AiResult<String> {
        let _ = on_reasoning;
        self.stream_chat(messages, on_token, cancel).await
    }

    /// Идентификатор модели (для истории/диагностики).
    fn model_id(&self) -> &str;

    /// R-3a (характеризация bootstrap-канона): конфиг-наблюдаемые параметры провайдера одной строкой.
    /// Нужен, потому что вызыватели держат `Arc<dyn ChatProvider>` (конкретный тип не достать), а
    /// характеризационные тесты обязаны сравнивать ВСЕ параметры сборки. Дефолт пуст — мокам/стабам
    /// нечего показывать; переопределяет только [`OpenAiChatProvider`] (Debug-снимок).
    #[doc(hidden)]
    fn debug_params(&self) -> String {
        String::new()
    }
}

/// Idle-таймаут стрима модели ПОСЛЕ первого байта (steady-state): если сервер не прислал чанк за это
/// время (залип / отдал битый ответ) — рвём запрос с ошибкой, чтобы чат/джоба не висели вечно (а
/// фоновая джоба не блокировала весь воркер). Каждый пришедший чанк сбрасывает таймер — легитимный
/// долгий стрим не обрывается. INFER-CFG: дефолт; конфигурируется `ChatConfig::idle_timeout()`.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Таймаут ПЕРВОГО токена (INFER-CFG, cold-start-aware): применяется к ИНИЦИАЦИИ стрима И ко всем
/// `resp.chunk()` ДО первого полученного байта. Крупная модель на холодном GPU (V100: компиляция
/// CUDA-ядер 1–3 мин на первом запросе) отдаёт 200+headers быстро, но первый `data:`-чанк задержан —
/// 90-секундный idle убил бы прогрев. Этот таймаут (дефолт 300 с) его переживает; ПОСЛЕ первого байта
/// действует уже [`STREAM_IDLE_TIMEOUT`] (детект зависшего steady-state). Конфигурируется
/// `ChatConfig::first_token_timeout()`.
const STREAM_FIRST_TOKEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Политика ретрая ИНИЦИАЦИИ запроса к модели (P0-d, ADR-009): мигающий локальный LLM не должен ронять
/// весь вызов на транзиентном сбое коннекта/статуса. Ретраится ТОЛЬКО установка стрима (post + проверка
/// HTTP-статуса) ДО чтения первого байта тела: частично прочитанный SSE-поток переиграть нельзя (дублит/
/// бьёт вывод), поэтому после старта стрима ретрая нет ВООБЩЕ. Идиома backoff зеркалит планировщик
/// (`base * 2^n`, потолок), но с интерактивно-чатовыми константами: чат — не фоновая джоба, суммарная
/// добавленная латентность держится скромной.
#[derive(Debug, Clone, Copy)]
struct RetryPolicy {
    /// Всего попыток (включая первую). Малое число — интерактивный чат, не фон.
    max_attempts: u32,
    /// База экспоненциального backoff. Задержка перед попыткой `n` (n=1 после 1-го провала) — `base*2^(n-1)`.
    base: std::time::Duration,
    /// Потолок одной задержки backoff (чтобы 2-я/3-я пауза не растягивали чат).
    cap: std::time::Duration,
}

impl RetryPolicy {
    /// Дефолт интерактивного чата: 3 попытки, base 300 мс, cap 2 с → паузы 300 мс, 600 мс (capped 2 с
    /// не достигается при 3 попытках) — суммарно <1 с добавленной латентности в худшем (но успешном) случае.
    const fn chat_default() -> Self {
        Self {
            max_attempts: 3,
            base: std::time::Duration::from_millis(300),
            cap: std::time::Duration::from_secs(2),
        }
    }

    /// INFER-CFG: переопределяет число попыток (из `ChatConfig::retry_attempts()`), сохраняя base/cap.
    /// `<1` нормализуется в 1 (хотя бы одна попытка — иначе запрос не отправится вообще).
    fn with_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }

    /// Задержка backoff ПЕРЕД попыткой с индексом `attempt` (0 = первая, без паузы). `base*2^(attempt-1)`,
    /// насыщающе (без переполнения сдвига) и под потолком `cap`. Зеркалит идиому планировщика.
    fn backoff(&self, attempt: u32) -> std::time::Duration {
        if attempt == 0 {
            return std::time::Duration::ZERO;
        }
        let shift = (attempt - 1).min(20); // защита от переполнения сдвига
        let factor = 1u32.checked_shl(shift).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap)
    }
}

/// Исход ОДНОЙ попытки инициации (seam для офлайн-теста политики): `Retryable` — транзиентный сбой,
/// можно повторить; `Fatal` — повторять НЕЛЬЗЯ (отказ политики эгресса / не-ретраибл 4xx). Классификация
/// (что транзиентно) живёт в [`classify_attempt_error`] и тестируется в изоляции от сети.
enum AttemptOutcome<T> {
    Ok(T),
    Retryable(AiError),
    Fatal(AiError),
}

/// Ретраибл-статусы HTTP (транзиентные): таймаут запроса, троттлинг, перегрузка/сбои апстрима.
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Классифицирует ошибку инициации запроса: транзиентная (ретраибл) vs фатальная.
///
/// - [`AiError::Denied`] — отказ ПОЛИТИКИ эгресса (до сокета): НИКОГДА не ретраим (повтор бессмыслен).
/// - [`AiError::Http`] из transport-reqwest (коннект/таймаут/сброс) — транзиентно, ретраим.
///
/// Статусные ошибки классифицируются раздельно (см. [`status_outcome`]) — здесь только транспорт/политика.
fn classify_attempt_error(err: AiError) -> AttemptOutcome<reqwest::Response> {
    match err {
        // Отказ политики эгресса — детерминированный, повтор не поможет (и плодил бы audit-записи).
        AiError::Denied(_) => AttemptOutcome::Fatal(err),
        // Транспорт (reqwest): коннект отказан/сброшен, таймаут, DNS — типично транзиентно для
        // локального LLM, который перезапускается/мигает. Idle-таймаут инициации тоже попадает сюда.
        AiError::Http(_) => AttemptOutcome::Retryable(err),
        // Прочее (Config/BadResponse/Dim…) на этапе инициации появиться не должно — на всякий случай
        // фатально (повтор бессмыслен).
        other => AttemptOutcome::Fatal(other),
    }
}

/// Классифицирует HTTP-статус успешно установленного коннекта: 2xx — ок (Response отдаётся дальше),
/// ретраибл-5xx/429/408 — `Retryable`, прочие (400/401/403/404/422…) — `Fatal`. Текст ошибки сохраняем
/// прежний (`статус {…}`) — поведение наружу не меняется.
fn status_outcome(resp: reqwest::Response) -> AttemptOutcome<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        AttemptOutcome::Ok(resp)
    } else if is_retryable_status(status.as_u16()) {
        AttemptOutcome::Retryable(AiError::Http(format!("статус {status}")))
    } else {
        AttemptOutcome::Fatal(AiError::Http(format!("статус {status}")))
    }
}

/// Cancel-aware сон: спит `dur`, но КАЖДЫЕ ~50 мс проверяет флаг отмены — взведённый `cancel` обрывает
/// паузу немедленно (Stop остаётся отзывчивым, даже если cap-backoff = 2 с). Возвращает `true`, если
/// проснулись по таймеру (можно продолжать), `false` — если отменены во сне.
async fn cancel_aware_sleep(dur: std::time::Duration, cancel: &Arc<AtomicBool>) -> bool {
    const TICK: std::time::Duration = std::time::Duration::from_millis(50);
    let deadline = std::time::Instant::now() + dur;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return true;
        }
        let step = (deadline - now).min(TICK);
        tokio::time::sleep(step).await;
    }
}

/// Ретрай-цикл инициации запроса (seam, P0-d). `attempt` — асинхронная «одна попытка»: возвращает уже
/// классифицированный [`AttemptOutcome`] (Ok/Retryable/Fatal). Цикл: проверяет `cancel` перед каждой
/// попыткой и перед/во время backoff-сна; на `Retryable` спит экспоненциальный backoff и пробует снова,
/// пока не исчерпаны `max_attempts`; на `Fatal` — возвращает сразу; по исчерпании — ПОСЛЕДНЮЮ ошибку
/// без изменений. ВАЖНО: ретраится только то, что делает `attempt` — у реального вызывающего это лишь
/// post + проверка статуса ДО первого чанка (стрим после старта не переигрывается).
async fn retry_request<T, F, Fut>(
    policy: RetryPolicy,
    cancel: &Arc<AtomicBool>,
    mut attempt: F,
) -> AiResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = AttemptOutcome<T>>,
{
    let mut last_err: Option<AiError> = None;
    for n in 0..policy.max_attempts {
        // Отмена до попытки: уважаем Stop, не открываем лишний сокет.
        if cancel.load(Ordering::Relaxed) {
            return Err(AiError::Http("запрос отменён".into()));
        }
        // Backoff ПЕРЕД повторной попыткой (n>=1); прерывается отменой во сне.
        if n > 0 && !cancel_aware_sleep(policy.backoff(n), cancel).await {
            return Err(AiError::Http("запрос отменён".into()));
        }
        match attempt().await {
            AttemptOutcome::Ok(v) => return Ok(v),
            AttemptOutcome::Fatal(e) => return Err(e),
            AttemptOutcome::Retryable(e) => last_err = Some(e),
        }
    }
    // Исчерпаны попытки — последняя ошибка БЕЗ изменений (типизированная).
    Err(last_err.unwrap_or_else(|| AiError::Http("ретрай исчерпан без ошибки".into())))
}

/// Chat через OpenAI-совместимый `POST {base}/v1/chat/completions` (llama.cpp-server, напр. Gemma).
pub struct OpenAiChatProvider {
    /// Guarded-клиент ядра (ADR-005-ext): политика+audit на каждый запрос, провайдер своего
    /// `reqwest::Client` не строит (AC-EGR-1/6).
    client: GuardedClient,
    /// Feature-тег эгресса — задаёт composition-root (обычно [`EgressFeature::Chat`]).
    feature: EgressFeature,
    endpoint: String,
    model: String,
    temperature: f32,
    /// Таймаут ПЕРВОГО токена (INFER-CFG): инициация стрима + чанки ДО первого байта (переживает
    /// cold-start). По умолчанию [`STREAM_FIRST_TOKEN_TIMEOUT`]; конфигурируется из `ChatConfig`.
    first_token_timeout: std::time::Duration,
    /// Idle-таймаут стрима ПОСЛЕ первого байта (по умолчанию [`STREAM_IDLE_TIMEOUT`]); короче — в тестах.
    idle_timeout: std::time::Duration,
    /// Политика ретрая ИНИЦИАЦИИ запроса (P0-d): post + проверка статуса ДО первого чанка. По
    /// умолчанию [`RetryPolicy::chat_default`]; в тестах — крошечные константы (быстрый backoff).
    retry: RetryPolicy,
    /// Включать ли «размышление» reasoning-модели (gemma). `true` для RAG-чата (точнее на сложных
    /// выводах), `false` для примитивов (inline/дайджест/судья) — там CoT только жрёт латентность/бюджет
    /// без выигрыша в качестве (замер 2026-06-09). При `false` шлём `chat_template_kwargs.enable_thinking`.
    enable_thinking: bool,
}

impl OpenAiChatProvider {
    /// Таймауты — у guarded-клиента (профиль [`GuardedClient::for_chat`]: connect-timeout без
    /// общего); здесь остаётся только idle-таймаут стрима (см. `stream_chat`).
    pub fn new(
        client: &GuardedClient,
        feature: EgressFeature,
        base_url: &str,
        model: &str,
        temperature: Option<f32>,
    ) -> Self {
        Self {
            client: client.clone(),
            feature,
            endpoint: format!("{}/v1/chat/completions", crate::ai::api_base(base_url)),
            model: model.to_string(),
            temperature: temperature.unwrap_or(0.3),
            first_token_timeout: STREAM_FIRST_TOKEN_TIMEOUT,
            idle_timeout: STREAM_IDLE_TIMEOUT,
            retry: RetryPolicy::chat_default(),
            enable_thinking: true,
        }
    }

    /// «Быстрый» вариант провайдера БЕЗ reasoning (для примитивов: inline/дайджест/судья). Тот же
    /// сервер/модель, но в запрос идёт `chat_template_kwargs.enable_thinking=false` → нет CoT-паузы.
    pub fn without_reasoning(mut self) -> Self {
        self.enable_thinking = false;
        self
    }

    /// INFER-CFG: таймаут первого токена (cold-start). Из `ChatConfig::first_token_timeout()`.
    pub fn with_first_token_timeout(mut self, d: std::time::Duration) -> Self {
        self.first_token_timeout = d;
        self
    }

    /// INFER-CFG: idle-таймаут стрима после первого байта. Из `ChatConfig::idle_timeout()`. Также
    /// используется тестами для быстрого обрыва залипшего сервера.
    pub fn with_idle_timeout(mut self, d: std::time::Duration) -> Self {
        self.idle_timeout = d;
        self
    }

    /// INFER-CFG: число попыток инициации запроса (P0-d). Из `ChatConfig::retry_attempts()`.
    pub fn with_retry_attempts(mut self, attempts: u32) -> Self {
        self.retry = self.retry.with_attempts(attempts);
        self
    }

    /// Тело запроса `/v1/chat/completions`. Вынесено отдельно для offline-теста переключателя reasoning:
    /// при `enable_thinking=false` добавляется `chat_template_kwargs.enable_thinking=false` (gemma глушит
    /// CoT — для примитивов; замер: rewrite ON=6.9с/пусто vs OFF=3.8с/ответ).
    fn request_body(&self, messages: &[ChatMessage]) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "temperature": self.temperature,
        });
        if !self.enable_thinking {
            body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
        }
        body
    }

    /// Тест-хелпер: переопределить политику ретрая (крошечный backoff в офлайн-тестах).
    #[cfg(test)]
    fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }
}

/// R-3a (характеризация bootstrap-канона): ВСЕ конфиг-наблюдаемые параметры провайдера. Вместо
/// объекта `client` печатается его профиль (`GuardedClient::debug_profile` — фабрика + таймауты;
/// сами значения живут в tune-замыкании и иначе не наблюдаемы). Снимок пинается характеризационными
/// тестами agentd (байт-идентичность канона старым строителям): менять формат = осознанно перепинать
/// фикстуры.
impl std::fmt::Debug for OpenAiChatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiChatProvider")
            .field("client", &self.client.debug_profile())
            .field("feature", &self.feature)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("first_token_timeout", &self.first_token_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("retry", &self.retry)
            .field("enable_thinking", &self.enable_thinking)
            .finish()
    }
}

#[async_trait]
impl ChatProvider for OpenAiChatProvider {
    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        on_token: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
    ) -> AiResult<String> {
        // Контентный путь = reasoning-путь с no-op обработчиком размышления (единый цикл, без дублей).
        self.stream_chat_reasoning(messages, on_token, &mut |_| {}, cancel)
            .await
    }

    async fn stream_chat_reasoning(
        &self,
        messages: &[ChatMessage],
        on_token: &mut (dyn FnMut(String) + Send),
        on_reasoning: &mut (dyn FnMut(String) + Send),
        cancel: &Arc<AtomicBool>,
    ) -> AiResult<String> {
        let body = self.request_body(messages);
        let start = std::time::Instant::now();
        // P0-d: ретрай ТОЛЬКО инициации стрима (post + проверка HTTP-статуса) ДО чтения первого байта.
        // После старта стрима (ниже, `resp.chunk()`) ретрая НЕТ: частично прочитанный SSE переиграть
        // нельзя. Каждая попытка — заново post через guarded-клиент (политика+audit ДО сокета; отказ —
        // типизированный `AiError::Denied`, который классификатор помечает Fatal → не ретраится).
        let mut resp = retry_request(self.retry, cancel, || async {
            // chat-стрим вне прогона агента (интерактивный/util-чат) → RunCtx::NONE (не коррелируется).
            let send_fut = self
                .client
                .post_json(&self.endpoint, self.feature, &body, RunCtx::NONE);
            // INFER-CFG: инициация = ДО первого байта → first_token_timeout (переживает cold-start
            // V100, 1–3 мин компиляции ядер). Залипший на коннекте сервер → транзиентная (ретраибл).
            match tokio::time::timeout(self.first_token_timeout, send_fut).await {
                Err(_) => AttemptOutcome::Retryable(AiError::Http(
                    "таймаут ответа модели (сервер не отвечает)".into(),
                )),
                Ok(Err(e)) => classify_attempt_error(AiError::from(e)),
                Ok(Ok(resp)) => status_outcome(resp),
            }
        })
        .await?;

        let mut full = String::new();
        // Цепочка «размышления» reasoning-модели в `full` НЕ идёт (только живой 💭-индикатор через
        // `on_reasoning`), но КОПИМ её здесь для отладочного лога: видеть, не зацикливается ли модель
        // на одних и тех же выводах (диагностика латентности RAG-чата, 2026-06-18).
        let mut reasoning = String::new();
        // По завершении потока: счётчики reasoning/ответа + время — всегда в INFO (приватно-безопасно;
        // по соотношению reasoning≫ответа видно зацикливание/over-thinking без текста заметок). Сам ТЕКСТ
        // цепочки содержит данные заметок → по AC-SEC-6 в лог по умолчанию НЕ идёт; пишем его ТОЛЬКО при
        // явном опте `NEXUS_TRACE_REASONING=1` (локальная отладка латентности reasoning; на INFO, чтобы
        // реально печаталось — глобальный фильтр прибит к INFO). Эта строка срабатывает на ВСЕХ LLM-стримах
        // (поле `model` различает источник); провайдеры без reasoning (новости/дайджест/судья) reasoning
        // не шлют → строка пуста, текст не пишется.
        let trace_reasoning = std::env::var_os("NEXUS_TRACE_REASONING").is_some();
        let model = self.model.as_str();
        let log_done = |reasoning: &str, content: &str| {
            tracing::info!(
                model,
                reasoning_chars = reasoning.chars().count(),
                content_chars = content.chars().count(),
                elapsed_ms = start.elapsed().as_millis() as u64,
                "llm: поток завершён"
            );
            if trace_reasoning && !reasoning.is_empty() {
                tracing::info!(
                    reasoning = %reasoning,
                    "llm: полная цепочка рассуждений (NEXUS_TRACE_REASONING)"
                );
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        // INFER-CFG cold-start стейт-машина: ДО первого полученного байта таймаут чанка =
        // first_token_timeout (200+headers пришли, но первый `data:`-чанк задержан компиляцией ядер
        // V100); ПОСЛЕ первого байта → idle_timeout (детект зависшего steady-state стрима). Каждый
        // байт сбрасывает таймер; легитимный долгий стрим не обрывается.
        let mut got_first_byte = false;
        loop {
            let chunk_timeout = if got_first_byte {
                self.idle_timeout
            } else {
                self.first_token_timeout
            };
            let Some(chunk) = tokio::time::timeout(chunk_timeout, resp.chunk())
                .await
                .map_err(|_| AiError::Http("таймаут стрима модели (нет данных)".into()))?
                .map_err(|e| AiError::Http(e.to_string()))?
            else {
                break;
            };
            // Первый НЕПУСТОЙ чанк переключает таймаут на idle (steady-state).
            if !chunk.is_empty() {
                got_first_byte = true;
            }
            if cancel.load(Ordering::Relaxed) {
                log_done(&reasoning, &full);
                return Ok(full);
            }
            buf.extend_from_slice(&chunk);
            // Обрабатываем все полные строки (граница `\n` — ASCII, кодпойнты не рвутся).
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                match parse_sse_delta(line.trim_end()) {
                    SseEvent::Content(s) => {
                        full.push_str(&s);
                        on_token(s);
                    }
                    // Размышление reasoning-модели: в `full` НЕ копим (живой 💭-индикатор R1) — но
                    // накапливаем в `reasoning` для отладочного лога завершения.
                    SseEvent::Reasoning(s) => {
                        reasoning.push_str(&s);
                        on_reasoning(s);
                    }
                    SseEvent::Done => {
                        log_done(&reasoning, &full);
                        return Ok(full);
                    }
                    SseEvent::Other => {}
                }
            }
        }
        log_done(&reasoning, &full);
        Ok(full)
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    fn debug_params(&self) -> String {
        format!("{self:?}")
    }
}

/// Событие одной SSE-строки потока чата.
enum SseEvent {
    /// Дельта контента ответа.
    Content(String),
    /// Дельта «размышления» reasoning-модели (`delta.reasoning_content`) — для 💭-индикатора (R1).
    Reasoning(String),
    Done,
    Other,
}

/// Парсит строку SSE (`data: …`) в дельту. Контент приоритетнее reasoning (в одном чанке обычно одно из
/// двух). Не-`data` строки и нераспознанный JSON → `Other`.
fn parse_sse_delta(line: &str) -> SseEvent {
    let Some(data) = line.strip_prefix("data:") else {
        return SseEvent::Other;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return SseEvent::Done;
    }
    #[derive(Deserialize)]
    struct StreamChunk {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        delta: Delta,
    }
    #[derive(Deserialize)]
    struct Delta {
        content: Option<String>,
        /// Поле reasoning-моделей (gemma/qwen-thinking): ход мысли отдельно от ответа.
        reasoning_content: Option<String>,
    }
    match serde_json::from_str::<StreamChunk>(data) {
        Ok(c) => {
            let Some(delta) = c.choices.into_iter().next().map(|ch| ch.delta) else {
                return SseEvent::Other;
            };
            if let Some(s) = delta.content.filter(|s| !s.is_empty()) {
                return SseEvent::Content(s);
            }
            if let Some(s) = delta.reasoning_content.filter(|s| !s.is_empty()) {
                return SseEvent::Reasoning(s);
            }
            SseEvent::Other
        }
        Err(_) => SseEvent::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_delta_extracts_content_and_done() {
        let line = r#"data: {"choices":[{"delta":{"content":"Привет"}}]}"#;
        assert!(matches!(parse_sse_delta(line), SseEvent::Content(s) if s == "Привет"));
        // R1: дельта reasoning-модели → SseEvent::Reasoning (отдельно от контента).
        let think = r#"data: {"choices":[{"delta":{"reasoning_content":"прикидываю"}}]}"#;
        assert!(matches!(parse_sse_delta(think), SseEvent::Reasoning(s) if s == "прикидываю"));
        assert!(matches!(parse_sse_delta("data: [DONE]"), SseEvent::Done));
        // первый кусок обычно несёт роль без content → Other
        let role = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(matches!(parse_sse_delta(role), SseEvent::Other));
        assert!(matches!(parse_sse_delta(": keep-alive"), SseEvent::Other));
        assert!(matches!(parse_sse_delta("data: not-json"), SseEvent::Other));
        assert!(matches!(parse_sse_delta(""), SseEvent::Other));
    }

    /// R2: `without_reasoning()` добавляет `chat_template_kwargs.enable_thinking=false` в тело запроса;
    /// обычный провайдер — без этого ключа (reasoning по умолчанию ON). Offline, без сервера.
    #[test]
    fn request_body_toggles_reasoning() {
        let guarded = GuardedClient::unchecked();
        let p = OpenAiChatProvider::new(&guarded, EgressFeature::Chat, "http://x", "gemma", None);
        let on = p.request_body(&[]);
        assert!(
            on.get("chat_template_kwargs").is_none(),
            "по умолчанию reasoning ON — без флага enable_thinking"
        );
        let off = OpenAiChatProvider::new(&guarded, EgressFeature::Chat, "http://x", "gemma", None)
            .without_reasoning()
            .request_body(&[]);
        assert_eq!(
            off["chat_template_kwargs"]["enable_thinking"],
            serde_json::json!(false)
        );
    }

    /// Залипший сервер (принял коннект, прочитал запрос, не отвечает — НЕТ даже первого байта) →
    /// `stream_chat` рвётся по таймауту с ошибкой, а НЕ висит вечно (регресс: дайджест-джоба зависала
    /// и блокировала воркер). INFER-CFG: ответа/первого байта нет → срабатывает `first_token_timeout`
    /// (НЕ idle — тот действует ПОСЛЕ первого байта); ставим его коротким, чтобы тест был
    /// детерминирован кросс-платформенно (раньше полагался на закрытие сокета сервером — флейк на Windows).
    #[tokio::test]
    async fn stream_chat_times_out_on_hung_server() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf); // запрос прочитали и «зависли» — не отвечаем
                std::thread::sleep(std::time::Duration::from_secs(2)); // дольше таймаута теста
            }
        });
        let provider = OpenAiChatProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &format!("http://{addr}"),
            "gemma",
            Some(0.0),
        )
        .with_first_token_timeout(std::time::Duration::from_millis(250))
        .with_idle_timeout(std::time::Duration::from_millis(250));
        let msgs = vec![ChatMessage::user("привет")];
        let mut sink = |_t: String| {};
        let cancel = Arc::new(AtomicBool::new(false));
        let start = std::time::Instant::now();
        let res = provider.stream_chat(&msgs, &mut sink, &cancel).await;
        assert!(res.is_err(), "залипший сервер → ошибка таймаута");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "оборвалось быстро по idle-таймауту, не повисло"
        );
        let _ = server.join();
    }

    /// AC-EGR-5/14 на уровне провайдера: отказ политики (выключенная фича) доходит до вызывающего
    /// ТИПИЗИРОВАННЫМ `AiError::Denied` (не reqwest-строкой) и не открывает сокет.
    #[tokio::test]
    async fn stream_chat_surfaces_typed_egress_denial() {
        use crate::net::{EgressAudit, EgressDenied, EgressPolicy};
        use std::sync::atomic::AtomicBool;

        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        policy.set_feature_enabled(EgressFeature::Chat, false);
        let guarded = GuardedClient::new(policy, Arc::new(EgressAudit::default()), |b| b).unwrap();
        let provider = OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            "http://127.0.0.1:9",
            "gemma",
            None,
        );
        let msgs = vec![ChatMessage::user("привет")];
        let cancel = Arc::new(AtomicBool::new(false));
        let res = provider.stream_chat(&msgs, &mut |_| {}, &cancel).await;
        assert!(
            matches!(
                res,
                Err(AiError::Denied(EgressDenied::FeatureNotEnabled(
                    EgressFeature::Chat
                )))
            ),
            "ожидали типизированный отказ политики: {res:?}"
        );
    }

    /// Живой стриминг против Gemma (`cargo test -- --ignored`; `NEXUS_CHAT_URL` — оверрайд хоста).
    #[tokio::test]
    #[ignore = "нужен chat-сервер (NEXUS_CHAT_URL, default 192.168.0.31:8080)"]
    async fn live_chat_streams_tokens() {
        let url =
            std::env::var("NEXUS_CHAT_URL").unwrap_or_else(|_| "http://192.168.0.31:8080".into());
        let provider = OpenAiChatProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &url,
            "gemma-4-26B-A4B-it",
            Some(0.0),
        );
        let msgs = vec![ChatMessage::user("Ответь одним словом: столица Франции?")];
        let mut tokens = 0usize;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut on_token = |_: String| tokens += 1;
        let full = provider
            .stream_chat(&msgs, &mut on_token, &cancel)
            .await
            .unwrap();
        assert!(tokens > 0, "должны прийти токены");
        assert!(!full.trim().is_empty(), "накопленный ответ непуст");
        assert!(full.to_lowercase().contains("париж") || full.to_lowercase().contains("paris"));
    }

    // --- P0-d: ретрай инициации запроса (политика в изоляции через seam) ---

    use std::cell::Cell;

    /// Крошечная политика для офлайн-тестов: 3 попытки, base 1 мс, cap 4 мс — backoff почти мгновенный.
    fn tiny_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            base: std::time::Duration::from_millis(1),
            cap: std::time::Duration::from_millis(4),
        }
    }

    /// (e) Backoff экспоненциальный и ограничен потолком: 0 → нет паузы, далее base*2^(n-1), но ≤ cap.
    #[test]
    fn backoff_is_exponential_and_capped() {
        let p = RetryPolicy {
            max_attempts: 5,
            base: std::time::Duration::from_millis(100),
            cap: std::time::Duration::from_millis(250),
        };
        assert_eq!(p.backoff(0), std::time::Duration::ZERO);
        assert_eq!(p.backoff(1), std::time::Duration::from_millis(100)); // base
        assert_eq!(p.backoff(2), std::time::Duration::from_millis(200)); // base*2
        assert_eq!(p.backoff(3), std::time::Duration::from_millis(250)); // base*4=400 → cap
        assert_eq!(p.backoff(4), std::time::Duration::from_millis(250)); // cap
                                                                         // Большой attempt не переполняет сдвиг и не превышает cap.
        assert_eq!(p.backoff(100), std::time::Duration::from_millis(250));
    }

    /// Прод-дефолт: суммарный добавленный backoff при 3 попытках скромен (<1 с) — это интерактивный чат.
    #[test]
    fn chat_default_total_backoff_is_modest() {
        let p = RetryPolicy::chat_default();
        assert_eq!(p.max_attempts, 3);
        // Паузы перед попытками 1 и 2 (попытка 0 — без паузы).
        let total = p.backoff(1) + p.backoff(2);
        assert!(
            total < std::time::Duration::from_secs(1),
            "суммарный backoff {total:?} должен быть < 1 с (интерактивный чат)"
        );
    }

    /// Классификатор: отказ политики эгресса — Fatal (не ретраим); transport-http — Retryable.
    #[test]
    fn classify_attempt_error_policy_vs_transport() {
        let denied = AiError::Denied(crate::net::EgressDenied::FeatureNotEnabled(
            EgressFeature::Chat,
        ));
        assert!(matches!(
            classify_attempt_error(denied),
            AttemptOutcome::Fatal(_)
        ));
        // BadResponse/Config на этапе инициации — фатально (повтор бессмыслен).
        assert!(matches!(
            classify_attempt_error(AiError::Config("x".into())),
            AttemptOutcome::Fatal(_)
        ));
    }

    /// Статусный классификатор: 2xx → Ok-маркер недоступен без Response, проверяем булеву таблицу статусов.
    #[test]
    fn retryable_status_table() {
        for s in [408u16, 429, 500, 502, 503, 504] {
            assert!(is_retryable_status(s), "{s} должен быть ретраибл");
        }
        for s in [400u16, 401, 403, 404, 422, 200, 201, 301] {
            assert!(!is_retryable_status(s), "{s} НЕ должен быть ретраибл");
        }
    }

    /// (a) Транзиентная ошибка ретраится до cap и в итоге успех; число попыток = (провалы + 1).
    #[tokio::test]
    async fn retry_succeeds_after_transient_failures() {
        let calls = Cell::new(0u32);
        let cancel = Arc::new(AtomicBool::new(false));
        let res: AiResult<&str> = retry_request(tiny_policy(), &cancel, || {
            let n = calls.get() + 1;
            calls.set(n);
            async move {
                if n < 3 {
                    AttemptOutcome::Retryable(AiError::Http("конн. сброшен".into()))
                } else {
                    AttemptOutcome::Ok("ответ")
                }
            }
        })
        .await;
        assert_eq!(res.unwrap(), "ответ");
        assert_eq!(calls.get(), 3, "2 провала + 1 успех = 3 попытки");
    }

    /// (b) Граница max_attempts: всё транзиентно → ровно `max_attempts` попыток, наружу — ПОСЛЕДНЯЯ ошибка.
    #[tokio::test]
    async fn retry_exhausts_and_returns_last_error() {
        let calls = Cell::new(0u32);
        let cancel = Arc::new(AtomicBool::new(false));
        let res: AiResult<&str> = retry_request(tiny_policy(), &cancel, || {
            let n = calls.get() + 1;
            calls.set(n);
            async move { AttemptOutcome::Retryable(AiError::Http(format!("сбой #{n}"))) }
        })
        .await;
        match res {
            Err(AiError::Http(msg)) => assert_eq!(msg, "сбой #3", "вернулась ПОСЛЕДНЯЯ ошибка"),
            other => panic!("ожидали последнюю Http-ошибку, получили {other:?}"),
        }
        assert_eq!(calls.get(), 3, "ровно max_attempts попыток");
    }

    /// (c) Fatal (отказ политики / не-ретраибл 4xx) НЕ ретраится: ровно ОДНА попытка.
    #[tokio::test]
    async fn retry_does_not_retry_fatal() {
        // Отказ политики эгресса.
        let calls = Cell::new(0u32);
        let cancel = Arc::new(AtomicBool::new(false));
        let res: AiResult<&str> = retry_request(tiny_policy(), &cancel, || {
            calls.set(calls.get() + 1);
            async move {
                AttemptOutcome::Fatal(AiError::Denied(
                    crate::net::EgressDenied::FeatureNotEnabled(EgressFeature::Chat),
                ))
            }
        })
        .await;
        assert!(matches!(res, Err(AiError::Denied(_))));
        assert_eq!(calls.get(), 1, "Fatal → без ретрая (1 попытка)");

        // Не-ретраибл 4xx (через status_outcome-эквивалент Fatal).
        let calls2 = Cell::new(0u32);
        let res2: AiResult<&str> = retry_request(tiny_policy(), &cancel, || {
            calls2.set(calls2.get() + 1);
            async move { AttemptOutcome::Fatal(AiError::Http("статус 404 Not Found".into())) }
        })
        .await;
        assert!(matches!(res2, Err(AiError::Http(_))));
        assert_eq!(calls2.get(), 1, "не-ретраибл 4xx → без ретрая");
    }

    /// (d) Взведённый cancel обрывает ретрай немедленно. Подслучай 1: отмена ДО первой попытки —
    /// 0 попыток. Подслучай 2: отмена ВО ВРЕМЯ backoff после транзиентного провала — обрыв на паузе.
    #[tokio::test]
    async fn retry_aborts_on_cancel() {
        // Отмена до старта.
        let cancel = Arc::new(AtomicBool::new(true));
        let calls = Cell::new(0u32);
        let res: AiResult<&str> = retry_request(tiny_policy(), &cancel, || {
            calls.set(calls.get() + 1);
            async move { AttemptOutcome::Ok("не должно вызваться") }
        })
        .await;
        assert!(res.is_err(), "отмена до старта → ошибка");
        assert_eq!(
            calls.get(),
            0,
            "ни одной попытки при заранее взведённом cancel"
        );

        // Отмена во время backoff: первая попытка транзиентно падает, попытка взводит cancel перед
        // тем как уйти в сон → cancel_aware_sleep обрывает паузу, второй попытки нет.
        let cancel2 = Arc::new(AtomicBool::new(false));
        let cancel2_for_closure = cancel2.clone();
        let calls2 = Cell::new(0u32);
        // Долгий backoff (cap 5 с) — если cancel НЕ уважается во сне, тест зависнет/упадёт по времени.
        let policy = RetryPolicy {
            max_attempts: 3,
            base: std::time::Duration::from_secs(5),
            cap: std::time::Duration::from_secs(5),
        };
        let start = std::time::Instant::now();
        let res2: AiResult<&str> = retry_request(policy, &cancel2, || {
            let n = calls2.get() + 1;
            calls2.set(n);
            let c = cancel2_for_closure.clone();
            async move {
                // Первая попытка проваливается транзиентно и взводит cancel → backoff должен оборваться.
                c.store(true, Ordering::Relaxed);
                AttemptOutcome::Retryable(AiError::Http(format!("сбой #{n}")))
            }
        })
        .await;
        assert!(res2.is_err(), "отмена во сне → ошибка");
        assert_eq!(
            calls2.get(),
            1,
            "вторая попытка не запускалась (отмена в backoff)"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "backoff оборвался по cancel быстро, не ждал 5 с (фактически {:?})",
            start.elapsed()
        );
    }

    /// Интеграция через провайдер (seam в проде): TCP-сервер сначала рвёт коннект (1 раз), затем отдаёт
    /// валидный SSE-стрим. Ретрай инициации должен пережить транзиентный сбой и собрать ответ.
    /// NB: моков фронта на контракт чат-стрима нет (стрим читается в Rust по Response::chunk) → зеркалить
    /// нечего; контракт проверяем этим integ-тестом + seam-тестами политики выше.
    #[tokio::test]
    async fn provider_retries_initiation_then_streams() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            // 1-е соединение: принять и СРАЗУ закрыть (transport-сбой инициации → ретраибл).
            if let Ok((sock, _)) = listener.accept() {
                drop(sock);
            }
            // 2-е соединение: отдать валидный SSE-ответ.
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Привет\"}}]}\n\n\
                            data: [DONE]\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
            }
        });
        let provider = OpenAiChatProvider::new(
            &GuardedClient::unchecked(),
            EgressFeature::Chat,
            &format!("http://{addr}"),
            "gemma",
            Some(0.0),
        )
        .with_idle_timeout(std::time::Duration::from_secs(2))
        .with_retry(tiny_policy());
        let msgs = vec![ChatMessage::user("привет")];
        let mut got = String::new();
        let mut on_token = |t: String| got.push_str(&t);
        let cancel = Arc::new(AtomicBool::new(false));
        let full = provider
            .stream_chat(&msgs, &mut on_token, &cancel)
            .await
            .expect("ретрай инициации пережил сброс коннекта и собрал ответ");
        assert_eq!(full, "Привет");
        assert_eq!(got, "Привет");
        let _ = server.join();
    }
}
