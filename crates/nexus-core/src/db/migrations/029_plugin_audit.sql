-- Durable plugin-broker audit (PLUG-1, THREAT_MODEL T1/§3): неотключаемый append-only журнал
-- capability-брокера плагинов в БД vault. До PLUG-1 журнал жил только в памяти (`AuditLog` —
-- `Vec<AuditEntry>`) и терялся при дропе брокера/рестарте — это блокировало включение недоверенной
-- плагин-экосистемы (пробел T1). Эта таблица — ДОЛГОВЕЧНЫЙ слой того же журнала: запись append-only
-- ПЕРЕД возвратом из `authorize` (write-before-act — зеркало egress_audit, миграция 020). In-memory
-- Vec остаётся для снимков `entries()`, pre-vault авторизаций (БД ещё не открыта) и тестов; БД —
-- durable-зеркало.
--
-- Ось `{plugin_id, method, target, allowed, denied_reason, created_at}` — своя (plugin/метод/цель),
-- ОТДЕЛЬНАЯ от egress_audit (там feature/host/bytes). Схема повторяет брокерский `AuditEntry`.
--
-- Append-only by design (инвариант как у egress_audit AC-EGR-4 и брокерского `AuditLog`): нет
-- DELETE/UPDATE-пути в коде — это журнал подотчётности, не кэш.
--
-- БОНУС-контейнмент: файл `.nexus/nexus.db` автоматически НЕ входит в glob-скоупы плагинов
-- (`.nexus` вне vault-путей + `is_escaping`/anti-traversal в permission.rs) → журнал недоступен
-- плагину на запись/чтение через брокер бесплатно.
CREATE TABLE plugin_audit (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id     TEXT    NOT NULL,            -- id плагина из сессии (или '<unknown>' для отозванного токена)
    method        TEXT    NOT NULL,            -- host-метод (vault.readFile|writeFile|net.fetch|ai.embed|…)
    target        TEXT,                        -- путь/хост цели (NULL, если метод без цели)
    allowed       INTEGER NOT NULL,            -- 1 = авторизовано (scope+право), 0 = отказ
    denied_reason TEXT,                        -- текст Denied при отказе; NULL при успехе
    created_at    INTEGER NOT NULL             -- unix-сек метки записи
);
-- Индекса НЕТ намеренно: горячий запрос UI — `ORDER BY id DESC LIMIT N` (id = INTEGER PRIMARY KEY =
-- rowid), которому служит встроенный B-tree первичного ключа. Отдельный индекс по created_at не нужен
-- (запрос по нему не идёт) и лишь тормозил бы append-INSERT. created_at — для отображения/сортировки
-- на клиенте (id монотонен по времени в пределах одной БД).
