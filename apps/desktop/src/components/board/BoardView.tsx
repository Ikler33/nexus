import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, CalendarClock, FolderClosed, LayoutGrid, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { tauriApi, type BoardData } from '../../lib/tauri-api';
import { useToastStore } from '../../stores/toast';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import { type DragData, planMove } from './board-dnd';
import { TaskPeek } from './TaskPeek';
import {
  applyOrder,
  basename,
  groupIntoColumns,
  isOverdue,
  knownPriority,
  OTHER_COLUMN_ID,
  todayIsoLocal,
} from './board-model';
import styles from './BoardView.module.css';

/** MIME-тип DnD-карточки (изолирует от перетаскивания вкладок редактора). */
const CARD_MIME = 'application/x-nexus-board-card';

/** Дефолтные id колонок, для которых есть локализованная метка `board.col.*` (пустой label → локализуем). */
const LOCALIZED_COL_IDS = new Set(['todo', 'doing', 'done']);

/** Класс цвета приоритета (известный набор → свой; прочее → нейтральный). */
function prioClass(priority: string | null): string {
  switch (knownPriority(priority)) {
    case 'low':
      return styles.prioLow;
    case 'medium':
      return styles.prioMedium;
    case 'high':
      return styles.prioHigh;
    case 'urgent':
      return styles.prioUrgent;
    default:
      return styles.prioOther;
  }
}

/**
 * «Доска» (BOARD-4 + BOARD-3, спека `docs/specs/kanban-board.md`): канбан-вью заметок-задач. Колонки и
 * ручной порядок — из персист-конфига `.nexus/boards/<id>.json` (`get_board`); статусы вне набора колонок
 * → виртуальная «Прочее» (§12, задачи не теряются). DnD/реордер/редактор колонок — BOARD-5. Состояния:
 * загрузка, ошибка (последняя валидная доска цела, §14.6), битый конфиг (пилюля), пусто. Клик по карточке
 * открывает заметку (превью — BOARD-6). Refetch на фокус окна (`.nexus` невидим watcher).
 */
export function BoardView() {
  const { t } = useTranslation();
  const closeBoard = useUIStore((s) => s.closeBoard);
  const addToast = useToastStore((s) => s.addToast);
  const [data, setData] = useState<BoardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  // DnD: что тащим (ref — читается в onDrop без устаревания) + подсветка целевой колонки.
  const dragRef = useRef<DragData | null>(null);
  const [dropCol, setDropCol] = useState<string | null>(null);
  const [busy, setBusy] = useState(false); // идёт persist хода — блокируем повторный DnD
  // Ref-зеркало busy: focus-эффект захватывает busy по замыканию (устаревал бы) — читаем актуальное (R3).
  const busyRef = useRef(false);
  const setMoving = (v: boolean) => {
    busyRef.current = v;
    setBusy(v);
  };
  // BOARD-6: путь карточки в превью-панели (peek). Клик по карточке открывает превью, не уводит с доски.
  const [peekPath, setPeekPath] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const next = await tauriApi.board.get();
      setData(next);
      setError(false);
    } catch {
      // Не обнуляем data — последняя валидная доска остаётся видимой (§14.6).
      setError(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // §14.6: `.nexus` невидим watcher'у → инвалидация доски на фокус окна (+ кнопка «Обновить»).
  // R3 (ревью): НЕ рефетчим во время хода — иначе load() перетрёт optimistic, а откат потом затрёт
  // свежие данные снапшотом. После завершения хода следующий фокус догонит.
  useEffect(() => {
    const onFocus = () => {
      if (!busyRef.current) void load();
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [load]);

  const openNote = (path: string) => {
    void useWorkspaceStore.getState().openFile(path);
    closeBoard();
  };
  // Клик по `[[ссылке]]` в превью — резолв вики-цели через openLink (та сама закроет доску → редактор).
  const openLink = (target: string) => {
    void useWorkspaceStore.getState().openLink(target);
    closeBoard();
  };

  const today = todayIsoLocal();
  const total = data?.cards.length ?? 0;
  const config = data?.config;
  // Колонки из конфига; карточки — группировка по статусу + ручной порядок колонки.
  const columns = data
    ? groupIntoColumns(
        data.cards,
        config!.columns.map((c) => c.id),
      ).map((col) => ({ ...col, cards: applyOrder(col.cards, config!.order[col.id]) }))
    : [];
  // Карточка в превью (если открыта и ещё существует — иначе панель просто не рендерится, напр. после удаления).
  const peekCard = peekPath ? (data?.cards.find((c) => c.path === peekPath) ?? null) : null;
  const labelById = new Map((config?.columns ?? []).map((c) => [c.id, c.label]));
  const columnLabel = (id: string): string => {
    if (id === OTHER_COLUMN_ID) return t('board.col.other');
    const label = labelById.get(id);
    if (label) return label; // пользовательская метка (переименование без правки файлов)
    if (LOCALIZED_COL_IDS.has(id)) return t(`board.col.${id}`);
    return id; // кастомный id без метки — показываем как есть
  };

  const clearDrag = () => {
    dragRef.current = null;
    setDropCol(null);
  };

  /**
   * Стейт-машина хода (BOARD-5, §14.6): optimistic-апдейт → persist (статус через `set_frontmatter_field`
   * + baseHash-sync, порядок через `save_board`) → откат на ТОЧНЫЙ снапшот при ошибке. MalformedFrontmatter
   * = карточка не двигается. Частичный сбой (статус записан, порядок нет) — статус НЕ откатываем.
   */
  const performMove = async (toCol: string, toIndex: number) => {
    const drag = dragRef.current;
    clearDrag();
    if (!drag || !data || !config || busy) return;
    const displayed: Record<string, string[]> = Object.fromEntries(
      columns.map((c) => [c.id, c.cards.map((card) => card.path)]),
    );
    const plan = planMove(displayed, drag, toCol, toIndex);
    if (!plan) return;

    const snapshot = data;
    const nextCards = plan.statusChange
      ? data.cards.map((c) =>
          c.path === plan.statusChange!.path ? { ...c, status: plan.statusChange!.status } : c,
        )
      : data.cards;
    const nextConfig = { ...config, order: { ...config.order, ...plan.order } };
    setData({ ...data, cards: nextCards, config: nextConfig });
    setMoving(true);
    try {
      if (plan.statusChange) {
        const path = plan.statusChange.path;
        const ws = useWorkspaceStore.getState();
        // Не теряем несохранённые правки тела: сперва флашим открытый грязный буфер на диск.
        if (ws.buffers[path]?.dirty) {
          await ws.saveBuffer(path, true);
          // R1 (ревью): saveBuffer ГЛОТАЕТ ошибку записи (остаётся dirty). Если флаш не удался —
          // НЕ трогаем frontmatter: set_frontmatter_field прочитал бы старый диск без правок тела, а
          // syncBufferAfterWrite затёр бы их в буфере = тихая потеря данных. Откатываем ход.
          if (useWorkspaceStore.getState().buffers[path]?.dirty) {
            setData(snapshot);
            addToast(t('board.dnd.statusError'), { kind: 'error' });
            setMoving(false);
            return;
          }
        }
        const res = await tauriApi.vault.setFrontmatterField(
          path,
          config.statusKey,
          plan.statusChange.status,
        );
        // SAFE-3 анти-эхо: синхронизируем открытый буфер новым контентом/хешем ДО watcher-события.
        useWorkspaceStore.getState().syncBufferAfterWrite(path, res.content, res.hash);
      }
    } catch {
      // §14.6(c): битый frontmatter / ошибка записи → карточка на ТОЧНЫЙ исходный индекс, файл цел.
      setData(snapshot);
      addToast(t('board.dnd.statusError'), { kind: 'error' });
      setMoving(false);
      return;
    }
    try {
      await tauriApi.board.save(nextConfig);
    } catch {
      // Статус (если был) УЖЕ на диске — карточку не возвращаем; не сохранён лишь ручной порядок.
      // Чистый реордер без статуса — откатываем (на диске ничего не менялось).
      if (!plan.statusChange) setData(snapshot);
      addToast(t('board.dnd.orderError'), { kind: 'error' });
    }
    setMoving(false);
  };

  return (
    <div className={styles.board}>
      <header className={styles.head}>
        <div className={styles.titleWrap}>
          <LayoutGrid size={20} aria-hidden />
          <h1 className={styles.title}>{t('board.title')}</h1>
          {data && (
            <span className={styles.total}>{t('board.taskCount', { count: total })}</span>
          )}
          {/* Битый JSON конфига (§14.6) — используется дефолт, файл НЕ перезаписан; видимый хинт. */}
          {data?.corrupt && (
            <span className={styles.errPill}>
              <AlertTriangle size={12} aria-hidden />
              {t('board.corruptConfig')}
            </span>
          )}
          {/* §14.6: ошибка ре-фетча при уже загруженной доске — последняя валидная доска цела, но
              провал виден (не молчит). Полноэкранная ошибка — только когда доски ещё нет. */}
          {error && data && (
            <span className={styles.errPill}>
              <AlertTriangle size={12} aria-hidden />
              {t('board.refreshError')}
            </span>
          )}
        </div>
        <button
          type="button"
          className={styles.refresh}
          onClick={() => void load()}
          title={t('board.refresh')}
          aria-label={t('board.refresh')}
          disabled={loading}
        >
          <RefreshCw size={15} className={loading ? styles.spin : ''} aria-hidden />
        </button>
      </header>

      {error && !data && (
        <div className={styles.state} role="alert">
          <AlertTriangle size={26} aria-hidden />
          <p>{t('board.loadError')}</p>
          <button type="button" className={styles.retry} onClick={() => void load()}>
            {t('board.retry')}
          </button>
        </div>
      )}

      {loading && !data && <div className={styles.state}>{t('board.loading')}</div>}

      {data && total === 0 && (
        <div className={styles.state}>
          <LayoutGrid size={30} aria-hidden />
          <p className={styles.emptyTitle}>{t('board.empty.title')}</p>
          <p className={styles.emptyBody}>{t('board.empty.body')}</p>
        </div>
      )}

      {data && total > 0 && (
        <div className={styles.bodyRow}>
        <div className={styles.columns}>
          {columns.map((col) => {
            const droppable = col.id !== OTHER_COLUMN_ID; // в «Прочее» ронять нельзя (нет статуса)
            const allowDrop = (e: React.DragEvent) => {
              if (!droppable || !e.dataTransfer.types.includes(CARD_MIME)) return false;
              e.preventDefault();
              e.dataTransfer.dropEffect = 'move';
              return true;
            };
            return (
              <section
                key={col.id}
                className={`${styles.column} ${dropCol === col.id ? styles.dropActive : ''}`}
                aria-label={columnLabel(col.id)}
                onDragOver={(e) => {
                  if (allowDrop(e) && dropCol !== col.id) setDropCol(col.id);
                }}
                onDragLeave={(e) => {
                  // Покидание секции (а не переход на дочерний элемент) — снять подсветку.
                  if (!e.currentTarget.contains(e.relatedTarget as Node)) {
                    setDropCol((c) => (c === col.id ? null : c));
                  }
                }}
                onDrop={(e) => {
                  if (!allowDrop(e)) return;
                  void performMove(col.id, col.cards.length); // на секцию = в конец колонки
                }}
              >
              <div className={styles.colHead}>
                <span className={styles.colTitle}>{columnLabel(col.id)}</span>
                <span className={styles.colCount}>{col.cards.length}</span>
              </div>
              <div className={styles.colCards}>
                {col.cards.map((card, cardIndex) => {
                  const overdue = isOverdue(card.due, today);
                  return (
                    <button
                      key={card.path}
                      type="button"
                      className={`${styles.card} ${peekPath === card.path ? styles.cardActive : ''}`}
                      draggable={!busy}
                      onClick={() => setPeekPath(card.path)}
                      onDragStart={(e) => {
                        const drag: DragData = { path: card.path, fromCol: col.id };
                        dragRef.current = drag;
                        e.dataTransfer.setData(CARD_MIME, card.path);
                        e.dataTransfer.effectAllowed = 'move';
                      }}
                      onDragEnd={clearDrag}
                      onDragOver={(e) => {
                        if (allowDrop(e)) {
                          e.stopPropagation();
                          if (dropCol !== col.id) setDropCol(col.id);
                        }
                      }}
                      onDrop={(e) => {
                        if (!allowDrop(e)) return;
                        e.stopPropagation(); // на карточку = вставка ПЕРЕД ней
                        void performMove(col.id, cardIndex);
                      }}
                    >
                      <span className={styles.cardTitle}>
                        {card.title || basename(card.path) || card.path}
                      </span>
                      {(card.priority || card.due || card.project) && (
                        <div className={styles.cardMeta}>
                          {card.priority && (
                            <span className={`${styles.badge} ${prioClass(card.priority)}`}>
                              {knownPriority(card.priority)
                                ? t(`board.priority.${knownPriority(card.priority)}`)
                                : card.priority}
                            </span>
                          )}
                          {card.due && (
                            <span className={`${styles.due} ${overdue ? styles.overdue : ''}`}>
                              <CalendarClock size={12} aria-hidden />
                              {card.due}
                              {overdue ? ` · ${t('board.overdue')}` : ''}
                            </span>
                          )}
                          {card.project && (
                            <span className={styles.project}>
                              <FolderClosed size={12} aria-hidden />
                              {card.project}
                            </span>
                          )}
                        </div>
                      )}
                      {card.tags.length > 0 && (
                        <div className={styles.tags}>
                          {card.tags.map((tag) => (
                            <span key={tag} className={styles.tag}>
                              #{tag}
                            </span>
                          ))}
                        </div>
                      )}
                    </button>
                  );
                })}
                {col.cards.length === 0 && (
                  <div className={styles.colEmpty}>{t('board.colEmpty')}</div>
                )}
              </div>
              </section>
            );
          })}
        </div>
          {peekCard && (
            <TaskPeek
              card={peekCard}
              onClose={() => setPeekPath(null)}
              onOpenFull={openNote}
              onOpenLink={openLink}
            />
          )}
        </div>
      )}
    </div>
  );
}
