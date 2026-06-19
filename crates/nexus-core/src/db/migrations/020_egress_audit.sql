-- Durable egress-audit (P0-b): неотключаемый append-only журнал исходящего HTTP ядра в БД vault.
-- До P0-b журнал жил только в памяти (Mutex<Vec<..>>) и терялся при краше — плохо для подотчётности
-- always-on агента. Эта таблица — ДОЛГОВЕЧНЫЙ слой того же журнала: запись append-only ПЕРЕД сокетом
-- (write-before-act) — egress::authorize awaits record() до отправки. In-memory Vec остаётся для чтений,
-- pre-vault эгресса (БД ещё не открыта) и тестов; БД — durable-зеркало.
--
-- Append-only by design (AC-EGR-4): нет DELETE/UPDATE-пути в коде; это журнал подотчётности, не кэш.
-- host хранится РЕАЛЬНЫЙ (не Redacted): локальная БД vault = собственный аудит пользователя; Redacted —
-- про утечку в Debug/логи, не про хранение на своём диске.
CREATE TABLE egress_audit (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    feature       TEXT    NOT NULL,            -- chat|embed|probe|news_feed|web (EgressFeature::Display)
    host          TEXT    NOT NULL,            -- реальный хост назначения (или сырой URL для BadUrl)
    bytes_out     INTEGER,                     -- размер тела ЗАПРОСА (best-effort): NULL для GET
    allowed       INTEGER NOT NULL,            -- 1 = пропущено политикой+гардом, 0 = отказ
    denied_reason TEXT,                        -- текст EgressDenied при отказе; NULL при успехе
    run_id        INTEGER,                     -- AgentRun correlation-id (SCAFFOLD): NULL для всех текущих
                                               --   вызывающих, пока нет run-контекста агента
    created_at    INTEGER NOT NULL             -- unix-сек метки записи
);
-- Хронологический срез журнала (последние N эгрессов) — основной паттерн чтения.
CREATE INDEX idx_egress_audit_created ON egress_audit(created_at DESC);
