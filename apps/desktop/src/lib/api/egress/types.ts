/**
 * DTO-типы egress-домена (F-2d): снимок политики эгресса ядра + идентификатор сетевой фичи (net.md
 * срез 2). Зеркала Rust-структур (`net::*`) — контракт провода `invoke`. Потребители импортируют
 * по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Снимок политики эгресса ядра (зеркалит Rust `net::EgressState`; срез 2 net.md). */
export interface EgressState {
  /** Kill-switch «офлайн» (E2): публичные хосты отрезаны, LAN/loopback живут. */
  offline: boolean;
  chat: boolean;
  embed: boolean;
  probe: boolean;
}
/** Сетевая фича ядра (E6); Web/NewsFeed/CloudFallback придут со срезами 3–4. */
export type EgressFeatureId = 'chat' | 'embed' | 'probe';
