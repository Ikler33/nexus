-- NF-6 (reader): кэш полного RU-текста статьи — перевод on-demand при первом открытии,
-- повторное открытие мгновенно. Абзацы хранятся одной строкой через пустую строку.
ALTER TABLE news_items ADD COLUMN body_ru TEXT;
ALTER TABLE news_items ADD COLUMN body_fetched_at INTEGER;
-- Текст был усечён потолком символов при извлечении (no silent caps — флаг виден в reader).
ALTER TABLE news_items ADD COLUMN body_truncated INTEGER NOT NULL DEFAULT 0;
