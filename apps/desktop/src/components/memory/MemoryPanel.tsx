import { useEffect, useRef, useState } from 'react';
import { Brain, Check, Pencil, Pin, PinOff, Plus, Trash2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFocusTrap } from '../../hooks/useFocusTrap';
import { MEM_CAP, staleFactIds, useMemoryStore } from '../../stores/memory';
import { useUIStore } from '../../stores/ui';
import { BrandThinking } from '../chrome/BrandThinking';
import styles from './MemoryPanel.module.css';

/**
 * Панель «Память ИИ» (MEM-4, AC-MEM-7; спека `docs/specs/agent-memory.md`): полный контроль над
 * ЯВНЫМИ ФАКТАМИ памяти агента — список (пины сверху), пин/анпин, правка-на-месте, удаление, ручное
 * добавление. При переполнении мягкого капа (D6) старые не-пины подсвечены «давно не использовался»
 * для ручной чистки. Модалка «как Goals/Digest» (focus-trap, Esc/клик-вне закрывают).
 */
export function MemoryPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeMemory);
  const trapRef = useFocusTrap<HTMLDivElement>(close);
  const facts = useMemoryStore((s) => s.facts);
  const loading = useMemoryStore((s) => s.loading);
  const load = useMemoryStore((s) => s.load);
  const add = useMemoryStore((s) => s.add);
  const setPinned = useMemoryStore((s) => s.setPinned);
  const editFact = useMemoryStore((s) => s.edit);
  const remove = useMemoryStore((s) => s.remove);

  const [draft, setDraft] = useState('');
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editText, setEditText] = useState('');
  const editInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    void load();
  }, [load]);

  // Клавиатура правки-на-месте через НАТИВНЫЙ листенер на самом input: он всплывает РАНЬШЕ нативного
  // Esc-листенера focus-trap-контейнера, поэтому `stopPropagation` здесь реально гасит закрытие всей
  // панели (React-onKeyDown срабатывает уже ПОСЛЕ контейнера — слишком поздно). Enter — сохранить
  // (читаем актуальное значение из DOM), Escape — отменить; оба не доходят до контейнера.
  useEffect(() => {
    const el = editInputRef.current;
    if (editingId == null || !el) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Enter' && e.key !== 'Escape') return;
      e.stopPropagation();
      if (e.key === 'Enter') {
        const text = el.value.trim();
        if (text) void editFact(editingId, text);
      }
      setEditingId(null);
      setEditText('');
    };
    el.addEventListener('keydown', onKey);
    return () => el.removeEventListener('keydown', onKey);
  }, [editingId, editFact]);

  const stale = staleFactIds(facts);
  const nonPinCount = facts.filter((f) => !f.pinned).length;
  const overCap = nonPinCount > MEM_CAP;

  const submitAdd = () => {
    const text = draft.trim();
    if (!text) return;
    setDraft('');
    void add(text);
  };

  const startEdit = (id: number, text: string) => {
    setEditingId(id);
    setEditText(text);
  };
  const saveEdit = () => {
    if (editingId == null) return;
    const text = editText.trim();
    if (text) void editFact(editingId, text);
    setEditingId(null);
    setEditText('');
  };
  const cancelEdit = () => {
    setEditingId(null);
    setEditText('');
  };

  const del = (id: number, text: string) => {
    if (window.confirm(t('memory.deleteConfirm', { text }))) void remove(id);
  };

  return (
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label={t('memory.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.head}>
          <Brain size={16} aria-hidden />
          <span className={styles.title}>{t('memory.title')}</span>
          {facts.length > 0 && <span className={styles.count}>{facts.length}</span>}
          <span className={styles.spacer} />
          <button
            className={styles.iconBtn}
            onClick={close}
            title={t('memory.close')}
            aria-label={t('memory.close')}
          >
            <X size={15} aria-hidden />
          </button>
        </header>

        {/* Ручное добавление факта (AC-MEM-7). */}
        <div className={styles.addRow}>
          <input
            className={styles.addInput}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submitAdd();
            }}
            placeholder={t('memory.addPlaceholder')}
            aria-label={t('memory.addPlaceholder')}
          />
          <button
            type="button"
            className={styles.addBtn}
            onClick={submitAdd}
            disabled={!draft.trim()}
          >
            <Plus size={14} aria-hidden /> {t('memory.add')}
          </button>
        </div>

        {/* D6: подсветка переполнения капа — приглашение к ручной чистке. */}
        {overCap && (
          <p className={styles.capWarn}>{t('memory.overCap', { count: nonPinCount, cap: MEM_CAP })}</p>
        )}

        {loading && facts.length === 0 ? (
          <div className={styles.thinking}>
            <BrandThinking size={26} />
            <span className="mt-label">{t('memory.loading')}</span>
          </div>
        ) : facts.length === 0 ? (
          <div className={styles.emptyState}>
            <Brain size={22} className={styles.emptyIco} aria-hidden />
            <p className={styles.empty}>{t('memory.empty')}</p>
          </div>
        ) : (
          <ul className={styles.list}>
            {facts.map((f) => (
              <li
                key={f.id}
                className={`${styles.row} ${f.pinned ? styles.pinnedRow : ''} ${
                  stale.has(f.id) ? styles.staleRow : ''
                }`}
              >
                <button
                  type="button"
                  className={`${styles.pinBtn} ${f.pinned ? styles.pinOn : ''}`}
                  onClick={() => void setPinned(f.id, !f.pinned)}
                  title={t(f.pinned ? 'memory.unpin' : 'memory.pin')}
                  aria-label={t(f.pinned ? 'memory.unpin' : 'memory.pin')}
                  aria-pressed={f.pinned}
                >
                  {f.pinned ? <Pin size={14} aria-hidden /> : <PinOff size={14} aria-hidden />}
                </button>

                {editingId === f.id ? (
                  <input
                    ref={editInputRef}
                    className={styles.editInput}
                    value={editText}
                    autoFocus
                    onChange={(e) => setEditText(e.target.value)}
                    aria-label={t('memory.editLabel')}
                  />
                ) : (
                  <span className={styles.text} title={f.text}>
                    {f.text}
                    {stale.has(f.id) && <span className={styles.staleBadge}>{t('memory.stale')}</span>}
                    {f.source === 'auto' && <span className={styles.autoBadge}>{t('memory.auto')}</span>}
                  </span>
                )}

                <span className={styles.rowActions}>
                  {editingId === f.id ? (
                    <>
                      <button
                        type="button"
                        className={styles.actBtn}
                        onClick={saveEdit}
                        title={t('memory.save')}
                        aria-label={t('memory.save')}
                      >
                        <Check size={14} aria-hidden />
                      </button>
                      <button
                        type="button"
                        className={styles.actBtn}
                        onClick={cancelEdit}
                        title={t('memory.cancel')}
                        aria-label={t('memory.cancel')}
                      >
                        <X size={14} aria-hidden />
                      </button>
                    </>
                  ) : (
                    <>
                      <button
                        type="button"
                        className={styles.actBtn}
                        onClick={() => startEdit(f.id, f.text)}
                        title={t('memory.edit')}
                        aria-label={t('memory.edit')}
                      >
                        <Pencil size={14} aria-hidden />
                      </button>
                      <button
                        type="button"
                        className={`${styles.actBtn} ${styles.delBtn}`}
                        onClick={() => del(f.id, f.text)}
                        title={t('memory.delete')}
                        aria-label={t('memory.delete')}
                      >
                        <Trash2 size={14} aria-hidden />
                      </button>
                    </>
                  )}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
