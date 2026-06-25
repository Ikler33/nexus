import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import {
  BookOpen,
  ChevronLeft,
  ChevronRight,
  Columns2,
  FileText,
  History,
  PanelRightClose,
  PenLine,
  Plus,
  X,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { EditorView } from '@codemirror/view';
import { toggleTaskAtLine } from '../../lib/editor/format';
import { getActiveEditorView } from '../../lib/editor/activeView';
import { formatCombo } from '../../lib/commands';
import { tauriApi } from '../../lib/tauri-api';
import { useInlineAIStore } from '../../stores/inlineAI';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { flush } from '../../stores/autosave';
import { Editor } from '../editor/Editor';
import { FileViewer } from '../editor/FileViewer';
import { isViewable } from '../../lib/file-kind';
import { InlineAIBar } from '../editor/InlineAIBar';
import { InspectorRail } from '../editor/InspectorRail';
import { MentionsBar } from '../editor/MentionsBar';
import { TagSuggest } from '../editor/TagSuggest';
import styles from './GroupPane.module.css';

// Preview грузится лениво (react-markdown+micromark ~160KB) — нужен только при включении режима «Просмотр».
const MarkdownPreview = lazy(() =>
  import('../editor/MarkdownPreview').then((m) => ({ default: m.MarkdownPreview })),
);

/** MIME-тип DnD вкладок между панами — контракт макета `editor.jsx` (DP-3). */
const TAB_MIME = 'text/nexus-tab';

/** Имя вкладки: basename без `.md` (DP-15, макет: табы носят title заметки, не имя файла). */
function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1).replace(/\.md$/, '');
}

/** Markdown-файл → доступен переключатель source/preview (#20). */
function isMarkdown(path: string): boolean {
  return /\.(md|markdown)$/i.test(path);
}

/**
 * Одна группа (сплит): floating-вкладки (DnD между панами, DP-3) + редактор/превью активной
 * вкладки (режим — в сторе, ⌘E / mode-float пилюля) + backlinks-бар. В режиме чтения хром
 * вкладок упрощается (App `.reading`).
 */
export function GroupPane({ groupId }: { groupId: string }) {
  const { t } = useTranslation();
  const group = useWorkspaceStore((s) => s.groups.find((g) => g.id === groupId));
  const buffers = useWorkspaceStore((s) => s.buffers);
  const isActive = useWorkspaceStore((s) => s.activeGroupId === groupId);
  const mode = useWorkspaceStore((s) => s.modes[groupId] ?? 'source');
  const setActiveTab = useWorkspaceStore((s) => s.setActiveTab);
  const setActiveGroup = useWorkspaceStore((s) => s.setActiveGroup);
  const closeTab = useWorkspaceStore((s) => s.closeTab);
  const moveTab = useWorkspaceStore((s) => s.moveTab);
  const toggleMode = useWorkspaceStore((s) => s.toggleMode);
  const splitRight = useWorkspaceStore((s) => s.splitRight);
  const closeGroup = useWorkspaceStore((s) => s.closeGroup);
  // W-1: крестик закрытия пейна показываем только при сплите (>1 группы) — последний не закрыть.
  const groupCount = useWorkspaceStore((s) => s.groups.length);
  const updateBufferDoc = useWorkspaceStore((s) => s.updateBufferDoc);
  const saveBuffer = useWorkspaceStore((s) => s.saveBuffer);
  const reloadFromDisk = useWorkspaceStore((s) => s.reloadFromDisk);
  const keepMine = useWorkspaceStore((s) => s.keepMine);
  const openLink = useWorkspaceStore((s) => s.openLink);
  const openFile = useWorkspaceStore((s) => s.openFile);
  // NAV-3: история навигации back/forward — ЛОГИКА уже в сторе (navBack/navForward, ⌘[ / ⌘]);
  // здесь только кнопки таб-стрипа + disabled-state по границам истории (макет editor.jsx tab-nav).
  const navBack = useWorkspaceStore((s) => s.navBack);
  const navForward = useWorkspaceStore((s) => s.navForward);
  const canBack = useWorkspaceStore((s) => s.navIndex > 0);
  const canForward = useWorkspaceStore((s) => s.navIndex < s.navHistory.length - 1);
  const createNote = useVaultStore((s) => s.createNote);
  const reading = useUIStore((s) => s.reading);
  const openVersions = useUIStore((s) => s.openVersions);
  const openTagFilter = useUIStore((s) => s.openTagFilter); // TAGCLICK-1: клик по #tag в превью → фильтр сайдбара
  // InlineAI prompt-box (⌘/): открыт ли он в ЭТОЙ группе (стор держит одну активную группу).
  const aiOpenHere = useInlineAIStore((s) => s.openGroupId === groupId);
  const [dropTarget, setDropTarget] = useState(false);
  // EDIT-7: ссылка на скролл-контейнер пейна — в режиме чтения/превью оглавление скроллит к заголовку
  // по `data-outline-line` (CM6 в source-режиме скроллит сам). Реф своего пейна → корректно при сплитах.
  const scrollRef = useRef<HTMLDivElement>(null);

  // DP-15 (макет editor.jsx): clock-чип doc-meta — mtime активного файла; перечитываем при смене
  // вкладки и переключении в превью (после правок mtime обновился сохранением).
  const activePath = group?.activeTab ?? null;
  const [mtime, setMtime] = useState<number | null>(null);
  useEffect(() => {
    if (!activePath) {
      setMtime(null);
      return;
    }
    let cancelled = false;
    tauriApi.vault
      .fileMtime(activePath)
      .then((v) => {
        if (!cancelled) setMtime(v);
      })
      .catch(() => {
        if (!cancelled) setMtime(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activePath, mode]);

  // Закрываем InlineAI prompt-box при смене активной вкладки ИЛИ режима (source↔preview) группы (макет:
  // aiOpen=false на смене вкладки; в превью нет живого CM6 — бар не нужен). getState — чтобы закрыть
  // ТОЛЬКО бар этой группы (close() глобален).
  useEffect(() => {
    const s = useInlineAIStore.getState();
    if (s.openGroupId === groupId) s.close();
  }, [activePath, groupId, mode]);

  if (!group) return null;
  const active = group.activeTab ? buffers[group.activeTab] : null;
  const mdActive = active != null && !isViewable(active.path) && isMarkdown(active.path);

  // InlineAI (⌘/): вставка сгенерированного текста БЛОКОМ в позицию курсора ТОГО редактора, из которого
  // открыли бар (view захвачен в сторе на триггере — при сплитах не промахнёмся в чужой пейн, ревью-MAJOR).
  // dispatch → updateListener → updateBufferDoc (без двойной записи). Нет живого view (закрыт/превью) →
  // дописываем БЛОКОМ в конец буфера — это и есть поведение дизайна editor.jsx (вставка-в-курсор — наша
  // адаптация под живой CM6). Закрываем prompt-box после вставки.
  const insertAI = (text: string) => {
    const trimmed = text.trim();
    if (trimmed && active) {
      const target = useInlineAIStore.getState().view;
      const live = target && target.dom?.isConnected ? target : null;
      if (live) {
        const pos = live.state.selection.main.head;
        const before = live.state.sliceDoc(Math.max(0, pos - 2), pos);
        const lead = pos === 0 || before.endsWith('\n\n') ? '' : before.endsWith('\n') ? '\n' : '\n\n';
        const insert = `${lead}${trimmed}\n`;
        live.dispatch({ changes: { from: pos, insert }, selection: { anchor: pos + insert.length } });
        live.focus();
      } else {
        const sep = active.doc && !active.doc.endsWith('\n') ? '\n' : '';
        updateBufferDoc(active.path, `${active.doc}${sep}\n${trimmed}\n`);
      }
    }
    useInlineAIStore.getState().close();
  };

  // EDIT-7: переход к заголовку из оглавления. Превью/чтение — скролл к элементу `data-outline-line`
  // в СВОЁМ скролл-контейнере (надёжно при сплитах). Source — через активный CM6-редактор: курсор на
  // начало строки + scrollIntoView + фокус (паттерн P6-AR/NAV-4). ОГРАНИЧЕНИЕ (репо-широкое, backlog):
  // getActiveEditorView = последний в фокусе → при ДВУХ source-сплитах клик в оглавлении пейна B уведёт
  // в редактор A; одно-пейновый случай (типичный) корректен, потери данных нет (как в commands-core/TasksPanel).
  const jumpToHeading = (line: number) => {
    if (mode === 'preview' || reading) {
      scrollRef.current
        ?.querySelector<HTMLElement>(`[data-outline-line="${line}"]`)
        ?.scrollIntoView({ block: 'start', behavior: 'smooth' });
      return;
    }
    const view = getActiveEditorView();
    if (!view) return;
    const lineNo = Math.min(Math.max(1, line), view.state.doc.lines);
    const pos = view.state.doc.line(lineNo).from;
    view.dispatch({ selection: { anchor: pos }, effects: EditorView.scrollIntoView(pos, { y: 'start' }) });
    view.focus();
  };

  return (
    <section
      className={`${styles.pane} ${dropTarget ? styles.dropTarget : ''}`}
      data-active={isActive || undefined}
      onMouseDownCapture={() => {
        if (!isActive) setActiveGroup(groupId);
      }}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(TAB_MIME)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        setDropTarget(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropTarget(false);
      }}
      onDrop={(e) => {
        setDropTarget(false);
        const raw = e.dataTransfer.getData(TAB_MIME);
        if (!raw) return;
        e.preventDefault();
        try {
          const { path, group: from } = JSON.parse(raw) as { path: string; group: string };
          moveTab(from, groupId, path);
        } catch {
          /* чужой/битый payload — игнор */
        }
      }}
      aria-label={`Группа редактора ${groupId}`}
    >
      <div className={styles.tabbar}>
        {/* NAV-3: back/forward — слева от вкладок (макет editor.jsx tab-nav); привязка к
            существующим navBack/navForward стора, disabled на границах истории. */}
        <div className={styles.tabNav}>
          <button
            className={styles.navBtn}
            disabled={!canBack}
            onClick={() => void navBack()}
            title={`${t('editor.back')}  ${formatCombo('mod+[')}`}
            aria-label={t('editor.back')}
          >
            <ChevronLeft size={14} aria-hidden />
          </button>
          <button
            className={styles.navBtn}
            disabled={!canForward}
            onClick={() => void navForward()}
            title={`${t('editor.forward')}  ${formatCombo('mod+]')}`}
            aria-label={t('editor.forward')}
          >
            <ChevronRight size={14} aria-hidden />
          </button>
        </div>
        <div className={styles.tabs} role="tablist">
          {group.tabs.map((path) => {
            const dirty = Boolean(buffers[path]?.dirty);
            return (
              <div
                key={path}
                role="tab"
                aria-selected={path === group.activeTab}
                data-active={path === group.activeTab || undefined}
                className={styles.tab}
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData(TAB_MIME, JSON.stringify({ path, group: groupId }));
                  e.dataTransfer.effectAllowed = 'move';
                  e.currentTarget.classList.add(styles.dragging);
                }}
                onDragEnd={(e) => e.currentTarget.classList.remove(styles.dragging)}
                onClick={() => setActiveTab(groupId, path)}
                title={path}
              >
                <FileText size={13} className={styles.tabIco} aria-hidden />
                <span className={styles.tabName}>{basename(path)}</span>
                {dirty ? (
                  <span className={styles.dot} aria-label={t('editor.unsaved')} />
                ) : (
                  <button
                    className={styles.close}
                    onClick={(e) => {
                      e.stopPropagation();
                      void closeTab(groupId, path);
                    }}
                    aria-label={t('editor.close', { name: basename(path) })}
                  >
                    <X size={12} aria-hidden />
                  </button>
                )}
              </div>
            );
          })}
          <button
            className={styles.tabAdd}
            onClick={() => void createNote().then((path) => openFile(path, groupId))}
            title={t('editor.newTab')}
            aria-label={t('editor.newTab')}
          >
            <Plus size={15} aria-hidden />
          </button>
        </div>
        <div className={styles.tabTools}>
          {mdActive && (
            <button
              className={styles.split}
              onClick={() => openVersions()}
              title={t('versions.open')}
              aria-label={t('versions.open')}
            >
              <History size={14} aria-hidden />
            </button>
          )}
          <button
            className={styles.split}
            onClick={() => splitRight()}
            title={t('editor.splitRight')}
            aria-label={t('editor.splitRight')}
          >
            <Columns2 size={14} aria-hidden />
          </button>
          {/* W-1: закрыть пейн (сплит) — только когда групп > 1 (последний пейн не закрываем). */}
          {groupCount > 1 && (
            <button
              className={styles.split}
              onClick={() => void closeGroup(groupId)}
              title={t('editor.closePane')}
              aria-label={t('editor.closePane')}
            >
              <PanelRightClose size={14} aria-hidden />
            </button>
          )}
        </div>
      </div>

      {active ? (
        <>
          {/* SAFE-3 guard: файл изменился на диске, пока в буфере были несохранённые правки. */}
          {active.externalChange && (
            <div className={styles.externalBanner} role="alert">
              <span className={styles.externalMsg}>{t('editor.external.title')}</span>
              <div className={styles.externalActions}>
                <button
                  className={styles.externalBtn}
                  onClick={() => void keepMine(active.path)}
                >
                  {t('editor.external.keepMine')}
                </button>
                <button
                  className={`${styles.externalBtn} ${styles.externalPrimary}`}
                  onClick={() => void reloadFromDisk(active.path)}
                >
                  {t('editor.external.loadDisk')}
                </button>
                <button className={styles.externalBtn} onClick={() => openVersions()}>
                  {t('editor.external.compare')}
                </button>
              </div>
            </div>
          )}
          {/* Editor-row (макет editor.jsx): контент слева + Inspector-rail справа. */}
          <div className={styles.editorRow}>
          <div className={styles.editorCol}>
          {/* InlineAI prompt-box (⌘/ или /ai, макет editor.jsx): плавающая карточка над колонкой.
              Заземление — текущая заметка (active.doc). Только source-режим (есть живой CM6 для вставки). */}
          {mdActive && mode === 'source' && !reading && aiOpenHere && (
            <InlineAIBar
              note={active.doc}
              onInsert={insertAI}
              onClose={() => useInlineAIStore.getState().close()}
            />
          )}
          <div className={styles.scroll} ref={scrollRef}>
            {/* Mode-float (DP-3): плавающая пилюля Edit/Preview — иконка показывает ДЕЙСТВИЕ. */}
            {mdActive && !reading && (
              <button
                className={styles.modeFloat}
                onClick={() => toggleMode(groupId)}
                title={mode === 'source' ? t('editor.preview') : t('editor.source')}
                aria-label={mode === 'source' ? t('editor.preview') : t('editor.source')}
                aria-pressed={mode === 'preview'}
              >
                <span key={mode} className={styles.modeIco}>
                  {mode === 'source' ? (
                    <BookOpen size={16} aria-hidden />
                  ) : (
                    <PenLine size={16} aria-hidden />
                  )}
                </span>
              </button>
            )}
            {isViewable(active.path) ? (
              <FileViewer path={active.path} />
            ) : mdActive && (mode === 'preview' || reading) ? (
              <Suspense fallback={null}>
                {/* MASTHEAD-1: editorial-шапка (kicker/title/byline) + буквица ведущего абзаца —
                    внутри MarkdownPreview (шапка и первый абзац должны быть соседями для буквицы).
                    `mtime` (живое состояние GroupPane) и `reading` (⌘R) прокидываем; остальное —
                    title/теги/слова — MarkdownPreview считает из источника сам. */}
                <MarkdownPreview
                  source={active.doc}
                  notePath={active.path}
                  masthead={{ mtime, reading }}
                  onOpenLink={(target) => void openLink(target)}
                  onOpenTag={openTagFilter}
                  onToggleTask={(line) => {
                    // EDIT-5: клик по чекбоксу в превью → флип исходной строки + dirty/автосейв.
                    // toggleTaskAtLine вернёт null, если строка уже не таск (дрейф) — тогда no-op.
                    const next = toggleTaskAtLine(active.doc, line);
                    if (next != null) updateBufferDoc(active.path, next);
                  }}
                  // AppendLine (макет): дописать строку в конец через буфер — НЕ новый бэкенд, обычная
                  // правка (updateBufferDoc → dirty → автосейв). В режиме чтения не показываем (это правка).
                  onAppendLine={
                    reading
                      ? undefined
                      : (line) => {
                          const sep = active.doc && !active.doc.endsWith('\n') ? '\n' : '';
                          updateBufferDoc(active.path, `${active.doc}${sep}${line}\n`);
                        }
                  }
                  fetchNotes={(q) => tauriApi.vault.listNotes(q, 50)}
                />
              </Suspense>
            ) : (
              <Editor
                key={groupId}
                path={active.path}
                groupId={groupId}
                initialDoc={active.doc}
                onChange={(doc) => updateBufferDoc(active.path, doc)}
                onSave={(doc) => {
                  updateBufferDoc(active.path, doc);
                  void saveBuffer(active.path, true); // Ctrl-S — ручная точка истории (SAFE-5)
                }}
                onBlur={() => void flush(active.path)}
                onOpenLink={(t) => void openLink(t)}
                fetchNotes={(q) => tauriApi.vault.listNotes(q, 50)}
                fetchTags={() => tauriApi.vault.listTags().then((ts) => ts.map((t) => t.name))}
              />
            )}
          </div>
          {/* UNLINK-1: незалинкованные упоминания заголовка — скрыты, если их нет. */}
          {!isViewable(active.path) && !reading && <MentionsBar path={active.path} />}
          {/* AI-2c: авто-тег (closed-vocab) — по клику; пишет инлайн-теги в тело. `key`=путь обязателен:
              иначе при смене вкладки в полёте suggest() «Применить» записал бы теги ЗАМЕТКИ-А в заметку-Б
              (стейт переживает смену path, ревью AI-2c MAJOR). key форсит ремоунт → сброс состояния. */}
          {!isViewable(active.path) && !reading && (
            <TagSuggest key={active.path} path={active.path} doc={active.doc} />
          )}
          </div>
          {/* Inspector-rail (макет editor.jsx): outline/backlinks — существующие OutlineBar/BacklinksBar;
              related/summary — структура + заглушка (контент в отдельном AI-срезе). Скрыт в режиме чтения
              и для бинарей (картинка/PDF — нет outline/backlinks). */}
          {mdActive && !reading && (
            <InspectorRail doc={active.doc} path={active.path} onJump={jumpToHeading} />
          )}
          </div>
        </>
      ) : (
        <p className={styles.empty}>{t('editor.emptyGroup')}</p>
      )}
    </section>
  );
}
