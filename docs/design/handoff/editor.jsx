// editor.jsx — tab strip, rendered markdown doc, [[wikilink]] autocomplete, backlinks bar.
(function () {
  const { useState, useRef, useMemo, useEffect } = React;
  const Icon = window.Icon;

  function TabStrip({ t, tabs, activeTab, onActivate, onClose, onAdd, dirtyMap, onSplit, secondary, onClosePane, pane, mode, onToggleMode }) {
    return React.createElement("div", { className: "tab-strip", role: "tablist" },
      tabs.map((id) => {
        const n = window.NEXUS_NOTES[id];
        const dirty = dirtyMap[id];
        return React.createElement("div", {
          key: id, role: "tab", "aria-selected": id === activeTab,
          className: "tab" + (id === activeTab ? " active" : ""),
          draggable: true,
          onDragStart: (e) => { e.dataTransfer.setData("text/nexus-tab", JSON.stringify({ id, pane })); e.dataTransfer.effectAllowed = "move"; e.currentTarget.classList.add("dragging"); },
          onDragEnd: (e) => { e.currentTarget.classList.remove("dragging"); },
          onClick: () => onActivate(id), onAuxClick: (e) => { if (e.button === 1) onClose(id); },
        },
          React.createElement(Icon, { name: "file-text", size: 13, style: { color: "var(--color-text-faint)" } }),
          React.createElement("span", { className: "tab-name" }, n.title),
          dirty
            ? React.createElement("span", { className: "dirty", title: "Не сохранено" })
            : React.createElement("span", {
                className: "tab-close", role: "button", "aria-label": t.close,
                onClick: (e) => { e.stopPropagation(); onClose(id); },
              }, React.createElement(Icon, { name: "x", size: 13 })),
        );
      }),
      React.createElement("div", { className: "tab-add", onClick: onAdd, title: t.new_tab },
        React.createElement(Icon, { name: "plus", size: 15 })),
      React.createElement("div", { className: "tab-tools" },
        secondary
          ? React.createElement("button", { className: "tb-btn", title: t.close_pane || "Закрыть панель", onClick: onClosePane },
              React.createElement(Icon, { name: "x", size: 15 }))
          : React.createElement("button", { className: "tb-btn", title: t.split, onClick: onSplit },
              React.createElement(Icon, { name: "panel-right", size: 15 })),
      ),
    );
  }

  // single-line append input with [[wikilink]] autocomplete
  function AppendLine({ t, onAppend }) {
    const [val, setVal] = useState("");
    const [pop, setPop] = useState(null); // {query, start}
    const [sel, setSel] = useState(0);
    const ref = useRef(null);
    const titles = useMemo(() => Object.entries(window.NEXUS_NOTES).map(([id, n]) => ({ id, title: n.title })), []);
    const matches = pop ? titles.filter((x) => x.title.toLowerCase().includes(pop.query.toLowerCase())).slice(0, 6) : [];

    function detect(text, caret) {
      const before = text.slice(0, caret);
      const m = before.match(/\[\[([^\]]*)$/);
      if (m) { setPop({ query: m[1], start: caret - m[1].length }); setSel(0); }
      else setPop(null);
    }
    function pick(item) {
      const text = val, caret = ref.current.selectionStart;
      const head = text.slice(0, pop.start - 2); // drop the [[
      const tail = text.slice(caret);
      const next = head + "[[" + item.title + "]]" + tail;
      setVal(next); setPop(null);
      requestAnimationFrame(() => { ref.current.focus(); const p = (head + "[[" + item.title + "]]").length; ref.current.setSelectionRange(p, p); });
    }
    function onKey(e) {
      if (pop && matches.length) {
        if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => (s + 1) % matches.length); return; }
        if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => (s - 1 + matches.length) % matches.length); return; }
        if (e.key === "Enter" || e.key === "Tab") { e.preventDefault(); pick(matches[sel]); return; }
        if (e.key === "Escape") { setPop(null); return; }
      }
      if (e.key === "Enter" && val.trim()) { onAppend(val.trim()); setVal(""); setPop(null); }
    }
    return React.createElement("div", { style: { position: "relative", marginTop: "var(--space-5)" } },
      React.createElement("div", { className: "global-search", style: { width: "100%", fontFamily: "var(--font-mono)" } },
        React.createElement(Icon, { name: "plus", size: 14 }),
        React.createElement("input", {
          ref, value: val,
          onChange: (e) => { setVal(e.target.value); detect(e.target.value, e.target.selectionStart); },
          onKeyDown: onKey, onClick: (e) => detect(val, e.target.selectionStart),
          placeholder: "Новая строка…  [[ — ссылка / link",
          style: { flex: 1, border: "none", background: "transparent", outline: "none", color: "var(--color-text)", fontSize: "var(--text-sm)", fontFamily: "var(--font-mono)" },
        }),
      ),
      pop && matches.length ? React.createElement("div", { className: "wl-pop", style: { top: 36, left: 22 } },
        matches.map((m, i) =>
          React.createElement("div", { key: m.id, className: "wl-item" + (i === sel ? " sel" : ""), onMouseEnter: () => setSel(i), onClick: () => pick(m) },
            React.createElement(Icon, { name: "link", size: 13, className: "ico" }),
            React.createElement("span", null, m.title),
          )
        )
      ) : null,
    );
  }

  function BacklinksBar({ t, noteId, onOpen }) {
    const [open, setOpen] = useState(true);
    const links = useMemo(() => window.computeBacklinks(noteId), [noteId]);
    return React.createElement("div", { className: "backlinks-bar" },
      React.createElement("div", { className: "bl-head", onClick: () => setOpen((o) => !o) },
        React.createElement("span", { className: "twist" + (open ? " open" : "") }, React.createElement(Icon, { name: "chevron", size: 13 })),
        React.createElement(Icon, { name: "link", size: 14, className: "ico" }),
        React.createElement("span", null, links.length ? t.backlinks_count(links.length) : t.backlinks),
      ),
      open ? React.createElement("div", { className: "bl-list" },
        links.length === 0
          ? React.createElement("div", { className: "es-sub", style: { padding: "var(--space-3)", color: "var(--color-text-faint)" } }, t.no_backlinks)
          : links.map((l, i) =>
              React.createElement("div", { key: i, className: "bl-item", onClick: () => onOpen(l.id) },
                React.createElement("div", { className: "bl-title" }, l.title),
                React.createElement("div", { className: "bl-ctx" }, l.context),
              )
            )
      ) : null,
    );
  }

  // auto-growing raw markdown source editor — outer container scrolls (like preview)
  function SourceEditor({ value, onChange, noteKey }) {
    const ref = useRef(null);
    const resize = () => { const el = ref.current; if (el) { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; } };
    useEffect(() => { resize(); }, [value, noteKey]);
    return React.createElement("textarea", {
      ref, className: "editor-source", value, spellCheck: false,
      onChange: (e) => { onChange(e.target.value); resize(); },
      placeholder: "# Заголовок\n\nТекст в markdown…  [[ссылка]]  #тег",
    });
  }

  function EditorArea(props) {
    const { t, tabs, activeTab, dirtyMap, onActivate, onClose, onAdd, onOpen, onTag, extraBody, editedBody, onAppend, onSplit, secondary, onClosePane, pane, mode, onToggleMode, onEditBody } = props;
    const scrollRef = useRef(null);
    useEffect(() => { if (scrollRef.current) scrollRef.current.scrollTop = 0; }, [activeTab, mode]);
    const note = activeTab ? window.NEXUS_NOTES[activeTab] : null;
    const raw = note ? (editedBody != null ? editedBody : (note.body + (extraBody || ""))) : "";
    const handlers = { onLink: onOpen, onTag };
    return React.createElement("main", { className: "editor-area" },
      React.createElement(TabStrip, { t, tabs, activeTab, onActivate, onClose, onAdd, dirtyMap, onSplit, secondary, onClosePane, pane }),
      note ? React.createElement("button", {
        className: "mode-float", onClick: onToggleMode,
        title: (mode === "edit" ? (t.preview_mode || "Просмотр") : (t.edit_mode || "Редактирование")) + "  ⌘E",
        "aria-label": mode === "edit" ? (t.preview_mode || "Просмотр") : (t.edit_mode || "Редактирование"),
      },
        React.createElement(Icon, { name: mode === "edit" ? "book-open" : "pencil", size: 15, key: mode, className: "mode-ico" })) : null,
      !note
        ? React.createElement("div", { className: "empty-state", style: { margin: "auto" } },
            React.createElement(Icon, { name: secondary ? "panel-right" : "file-text", size: 32, style: { opacity: 0.35 } }),
            React.createElement("div", { className: "es-title" }, secondary ? (t.drop_here_title || "Перетащите заметку сюда") : "Нет открытых заметок / No note open"),
            React.createElement("div", { className: "es-sub" }, secondary ? (t.drop_here_sub || "Перетащите вкладку из соседней панели или нажмите +") : "Откройте файл слева или нажмите ⌘K"))
        : (mode === "edit"
          ? React.createElement("div", { className: "editor-scroll", ref: scrollRef },
              React.createElement(SourceEditor, { value: raw, onChange: (text) => onEditBody(activeTab, text), noteKey: activeTab }))
          : React.createElement("div", { className: "editor-scroll", ref: scrollRef },
            React.createElement("article", { className: "editor-doc", key: activeTab },
              React.createElement("div", { className: "doc-meta" },
                React.createElement("span", { className: "chip" }, React.createElement(Icon, { name: "clock", size: 13 }), note.mtime),
                React.createElement("span", { className: "chip" }, note.words + " " + t.words),
                React.createElement("span", { className: "chip" }, t.reading_time(Math.max(1, Math.round(note.words / 200)))),
              ),
              window.renderMarkdown(raw, handlers),
              React.createElement(AppendLine, { t, onAppend }),
            )
          )),
      note ? React.createElement(BacklinksBar, { t, noteId: activeTab, onOpen, key: activeTab }) : null,
    );
  }
  window.EditorArea = EditorArea;
})();
