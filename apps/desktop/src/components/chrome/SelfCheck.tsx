import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Check, RefreshCw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi, type AiConfigDto, type AiEndpoint } from '../../lib/tauri-api';
import { useVaultStore } from '../../stores/vault';
import styles from './SelfCheck.module.css';

/**
 * W-21 — dev self-check при старте (анти-рецидив 2026-06-24). Пингует сконфигурированные
 * LLM-эндпоинты (chat/embedding/fast) и показывает активный конфиг (URL + модель). Цель:
 * дрейф вида «.31 вместо .28» виден СРАЗУ при запуске, а не через молча сломанную фичу
 * (новости/чат). Монтируется только в dev-сборке (`import.meta.env.DEV`) — это аид разработки.
 *
 * Реюз существующих команд: `settings.getAiConfig` (чтение `.nexus/local.json`) +
 * `settings.testConnection` (GET `/v1/models` через GuardedClient `Probe`).
 */

type Probe = 'checking' | 'ok' | 'fail' | 'unset';

interface Row {
  key: 'chat' | 'embedding' | 'fast';
  /** `fast` не обязателен (падает на chat-модель) → его «не задан» нейтрален, не ошибка. */
  optional: boolean;
  ep: AiEndpoint | null;
  status: Probe;
  error?: string;
}

const ROWS: Array<Pick<Row, 'key' | 'optional'>> = [
  { key: 'chat', optional: false },
  { key: 'embedding', optional: false },
  { key: 'fast', optional: true },
];

export function SelfCheck() {
  const { t } = useTranslation();
  // Перепроверять при СМЕНЕ vault (новый vault — свой `.nexus/local.json`, дрейф мог появиться).
  const vaultRoot = useVaultStore((s) => s.info?.root ?? null);
  const [open, setOpen] = useState(true);
  const [rows, setRows] = useState<Row[] | null>(null);
  const [running, setRunning] = useState(false);
  // Latest-wins: при перезапуске (vault-switch / ручной re-check) старый прогон не должен затирать новый.
  const reqId = useRef(0);

  const run = useCallback(async () => {
    const my = ++reqId.current;
    setRunning(true);
    let cfg: AiConfigDto;
    try {
      cfg = await tauriApi.settings.getAiConfig();
    } catch {
      // Vault не открыт / конфига нет — нечего проверять, прячемся.
      if (my === reqId.current) {
        setRows([]);
        setRunning(false);
      }
      return;
    }
    const eps: Record<Row['key'], AiEndpoint | null> = {
      chat: cfg.chat,
      embedding: cfg.embedding,
      fast: cfg.fast,
    };
    if (my === reqId.current) {
      setRows(ROWS.map((r) => ({ ...r, ep: eps[r.key], status: 'checking' as Probe })));
    }
    const results = await Promise.all(
      ROWS.map(async (r): Promise<Row> => {
        const ep = eps[r.key];
        const url = ep?.url?.trim();
        if (!url) return { ...r, ep, status: 'unset' };
        try {
          await tauriApi.settings.testConnection(url);
          return { ...r, ep, status: 'ok' };
        } catch (e) {
          // Tauri-команда reject'ит строкой; мок — Error. Оба покрываем.
          const msg = e instanceof Error ? e.message : String(e);
          return { ...r, ep, status: 'fail', error: msg };
        }
      }),
    );
    if (my === reqId.current) {
      setRows(results);
      setRunning(false);
    }
  }, []);

  // На монтировании и при смене vault — перепроверить и показать (даже если ранее скрыли).
  useEffect(() => {
    setOpen(true);
    void run();
  }, [run, vaultRoot]);

  // null — первый прогон ещё идёт; пустой массив — проверять нечего (vault не открыт).
  if (!open || !rows || rows.length === 0) return null;

  // Проблема = недостижимый эндпоинт ИЛИ обязательный (chat/embedding) не задан.
  const problem = rows.some((r) => r.status === 'fail' || (r.status === 'unset' && !r.optional));

  return (
    <div
      className={`${styles.card} ${problem ? styles.cardWarn : ''}`}
      role="status"
      aria-live="polite"
    >
      <div className={`${styles.head} ${problem ? styles.headWarn : ''}`}>
        <span className={styles.title}>
          {problem ? (
            <AlertTriangle size={13} aria-hidden className={styles.iconWarn} />
          ) : (
            <Check size={13} aria-hidden className={styles.iconOk} />
          )}
          {t('selfCheck.title')}
        </span>
        <span className={styles.actions}>
          <button
            type="button"
            className={styles.iconBtn}
            title={t('selfCheck.recheck')}
            onClick={() => void run()}
            disabled={running}
          >
            <RefreshCw size={12} aria-hidden className={running ? styles.spin : undefined} />
          </button>
          <button
            type="button"
            className={styles.iconBtn}
            title={t('selfCheck.dismiss')}
            onClick={() => setOpen(false)}
          >
            <X size={13} aria-hidden />
          </button>
        </span>
      </div>
      <ul className={styles.list}>
        {rows.map((r) => (
          <li key={r.key} className={styles.row}>
            <span className={`${styles.badge} ${styles[`badge_${r.status}`]}`} aria-hidden>
              {r.status === 'ok' ? '✓' : r.status === 'fail' ? '✗' : r.status === 'unset' ? '—' : '…'}
            </span>
            <span className={styles.rowLabel}>{t(`selfCheck.${r.key}`)}</span>
            <span className={styles.rowMeta} title={r.ep?.url ?? ''}>
              {r.ep?.url
                ? `${r.ep.url}${r.ep.model ? ` · ${r.ep.model}` : ''}`
                : r.optional
                  ? t('selfCheck.unsetOptional')
                  : t('selfCheck.unsetRequired')}
            </span>
            {r.status === 'fail' && r.error && (
              <span className={styles.rowErr} title={r.error}>
                {r.error}
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
