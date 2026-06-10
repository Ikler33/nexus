// app.jsx — App composition, titlebar, status bar, state, tweaks wiring.
(function () {
  const { useState, useEffect, useRef, useCallback } = React;
  const Icon = window.Icon;

  const ACCENTS = [
    { id: "amber", swatch: "oklch(0.62 0.135 47)" },
    { id: "teal",  swatch: "oklch(0.58 0.075 205)" },
    { id: "sage",  swatch: "oklch(0.58 0.07 158)" },
    { id: "clay",  swatch: "oklch(0.57 0.11 28)" },
  ];
  const EDITOR_FONTS = {
    sans: '"Onest", system-ui, sans-serif',
    serif: '"Source Serif 4", Georgia, serif',
    mono: '"JetBrains Mono", ui-monospace, monospace',
  };

  const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
    "density": "comfortable",
    "chrome": "standard",
    "accent": "amber",
    "aiLayout": "side",
    "paletteStyle": "top",
    "editorFont": "sans",
    "ragSources": "cards",
    "platform": "mac",
    "offline": false,
    "cloud": false
  }/*EDITMODE-END*/;

  function AIMenu({ lang, onDigest, onGoals, onContra }) {
    const [open, setOpen] = useState(false);
    const ref = useRef(null);
    useEffect(() => {
      if (!open) return;
      const h = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
      window.addEventListener("mousedown", h); return () => window.removeEventListener("mousedown", h);
    }, [open]);
    const items = [
      { ic: "newspaper", label: lang === "ru" ? "Дайджест изменений" : "Change digest", run: onDigest },
      { ic: "target", label: lang === "ru" ? "Цели" : "Goals", run: onGoals },
      { ic: "scale", label: lang === "ru" ? "Поиск противоречий" : "Contradictions", run: onContra },
    ];
    return React.createElement("div", { className: "ai-menu-wrap", ref },
      React.createElement("button", { className: "tb-btn ai-menu-trigger" + (open ? " active" : ""), onClick: () => setOpen((v) => !v), title: lang === "ru" ? "AI-инсайты" : "AI insights" },
        React.createElement(Icon, { name: "sparkles", size: 16 }),
        React.createElement(Icon, { name: "chevron-down", size: 11, style: { opacity: 0.55 } })),
      open ? React.createElement("div", { className: "ai-menu" },
        React.createElement("div", { className: "ai-menu-head" }, lang === "ru" ? "AI-инсайты по vault" : "AI insights"),
        items.map((it) => React.createElement("button", { key: it.ic, className: "ai-menu-item", onClick: () => { setOpen(false); it.run(); } },
          React.createElement(Icon, { name: it.ic, size: 15, className: "ico" }), it.label))) : null);
  }

  function Titlebar({ t, theme, setTheme, lang, setLang, platform, onOpenPalette, aiOpen, setAiOpen, onHome, onDigest, onGoals, onContra, reading, setReading }) {
    const traffic = platform === "win"
      ? React.createElement("div", { className: "traffic win" },
          React.createElement("button", { className: "light flat", tabIndex: -1 }, React.createElement(Icon, { name: "minus", size: 14 })),
          React.createElement("button", { className: "light flat", tabIndex: -1 }, React.createElement(Icon, { name: "square", size: 12 })),
          React.createElement("button", { className: "light flat close", tabIndex: -1 }, React.createElement(Icon, { name: "x", size: 14 })))
      : React.createElement("div", { className: "traffic" },
          React.createElement("span", { className: "light r" }), React.createElement("span", { className: "light y" }), React.createElement("span", { className: "light g" }));

    return React.createElement("header", { className: "titlebar" },
      platform === "win" ? null : traffic,
      React.createElement("button", { className: "brand", onClick: onHome, title: lang === "ru" ? "На главную" : "Home", style: { background: "none", border: "none", cursor: "pointer", padding: 0 } },
        React.createElement(window.BrandMark, { size: 26 }),
        React.createElement("span", { className: "app-name" }, "Nexus")),
      React.createElement("div", { className: "tb-spacer" }),
      React.createElement("button", { className: "global-search", onClick: onOpenPalette },
        React.createElement(Icon, { name: "search", size: 14 }),
        React.createElement("span", { style: { flex: 1, textAlign: "left" } }, t.search_files_cmds),
        React.createElement("span", { className: "kbd" }, "⌘K")),
      React.createElement("div", { className: "tb-spacer" }),
      React.createElement("div", { className: "tb-group" },
        React.createElement(AIMenu, { lang, onDigest, onGoals, onContra }),
        React.createElement("div", { className: "tb-divider" }),
        React.createElement("button", { className: "tb-btn" + (reading ? " active" : ""), onClick: () => setReading((v) => !v), title: t.reading_mode + "  ⌘R" },
          React.createElement(Icon, { name: "book-open", size: 16 })),
        React.createElement("button", { className: "tb-btn tb-lang", onClick: () => setLang(lang === "ru" ? "en" : "ru"), title: t.toggle_lang },
          React.createElement("span", { className: lang === "ru" ? "on" : "" }, "RU"),
          React.createElement("span", { className: "sep" }, "/"),
          React.createElement("span", { className: lang === "en" ? "on" : "" }, "EN")),
        React.createElement("button", { className: "tb-btn", onClick: () => setTheme(theme === "light" ? "dark" : theme === "dark" ? "midnight" : theme === "midnight" ? "platinum" : "light"), title: t.toggle_theme },
          React.createElement(Icon, { name: theme === "light" ? "sun" : theme === "dark" ? "moon" : theme === "midnight" ? "sparkles" : "drive", size: 16 })),
        React.createElement("button", { className: "tb-btn" + (aiOpen ? " active" : ""), onClick: () => setAiOpen((v) => !v), title: t.ai_assistant },
          React.createElement(Icon, { name: "panel-right", size: 16 }))),
      platform === "win" ? React.createElement("div", { className: "tb-divider" }) : null,
      platform === "win" ? traffic : null,
    );
  }

  // Left vertical activity bar — app navigation + tools (Obsidian/VS Code style)
  function ActivityBar({ t, lang, view, onHome, onNews, sidebarOpen, setSidebarOpen, graphOpen, setGraphOpen, reading, setReading, onSync, syncConflict, onSettings }) {
    const Btn = (p) => React.createElement("button", {
      className: "act-btn" + (p.active ? " active" : ""), onClick: p.onClick, title: p.title, "aria-label": p.title,
    }, React.createElement(Icon, { name: p.icon, size: 19 }), p.badge ? React.createElement("span", { className: "act-badge" }) : null);
    return React.createElement("nav", { className: "activity-bar", "aria-label": lang === "ru" ? "Навигация" : "Navigation" },
      React.createElement("div", { className: "act-group" },
        React.createElement(Btn, { icon: "home", title: "Home", active: view === "home", onClick: onHome }),
        React.createElement(Btn, { icon: "newspaper", title: lang === "ru" ? "Новости" : "News", active: view === "news", onClick: onNews }),
        React.createElement(Btn, { icon: "file-text", title: t.explorer, active: view === "workspace" && sidebarOpen, onClick: () => setSidebarOpen((v) => !v) }),
        React.createElement(Btn, { icon: "graph", title: t.graph_view + "  ⌘⇧G", active: graphOpen, onClick: () => setGraphOpen((v) => !v) })),
      React.createElement("div", { className: "act-spacer" }),
      React.createElement("div", { className: "act-group" },
        React.createElement(Btn, { icon: "git-branch", title: (lang === "ru" ? "Синхронизация" : "Sync") + "  (git)", badge: syncConflict, onClick: onSync }),
        React.createElement(Btn, { icon: "settings", title: (lang === "ru" ? "Настройки" : "Settings") + "  ⌘,", onClick: onSettings })));
  }

  function StatusBar({ t, indexing, conflict, onConflict, lang }) {
    return React.createElement("footer", { className: "status-bar" },
      React.createElement("span", { className: "sb-item" }, React.createElement("span", { className: "sb-dot ok" }), t.synced),
      indexing.active
        ? React.createElement("span", { className: "sb-item" },
            React.createElement("span", { className: "sb-progress" }, React.createElement("i", { style: { width: (indexing.done / indexing.total * 100) + "%" } })),
            t.indexing(indexing.done, indexing.total))
        : React.createElement("span", { className: "sb-item" }, React.createElement(Icon, { name: "check", size: 12, className: "ico" }), t.indexed + " · 50k"),
      React.createElement("div", { className: "sb-right" },
        conflict ? React.createElement("button", { className: "sb-item sb-conflict", onClick: onConflict },
          React.createElement(Icon, { name: "git-merge", size: 13 }),
          lang === "ru" ? "1 конфликт" : "1 conflict") : null,
        React.createElement("span", { className: "sb-item" }, React.createElement(Icon, { name: "drive", size: 13, className: "ico" }), t.local),
        React.createElement("span", { className: "sb-item" }, "UTF-8"),
        React.createElement("span", { className: "sb-item" }, "Markdown")),
    );
  }

  function App() {
    const [tw, setTweak] = window.useTweaks(TWEAK_DEFAULTS);
    const [theme, setTheme] = useState(() => localStorage.getItem("nexus-theme") || "light");
    const [lang, setLang] = useState(() => localStorage.getItem("nexus-lang") || "ru");
    const [tabsA, setTabsA] = useState(["nexus", "rag-pipeline", "second-brain"]);
    const [tabsB, setTabsB] = useState([]);
    const [activeTab, setActiveTab] = useState("nexus");
    const [dirtyMap, setDirtyMap] = useState({});
    const [extraBodies, setExtraBodies] = useState({});
    const [editedBodies, setEditedBodies] = useState({});
    const [modeA, setModeA] = useState("preview"); // 'edit' | 'preview'
    const [modeB, setModeB] = useState("preview");
    const [dropPane, setDropPane] = useState(null); // 'a' | 'b' while dragging a tab over a pane
    const [sidebarOpen, setSidebarOpen] = useState(true);
    const [aiOpen, setAiOpen] = useState(true);
    const [reading, setReading] = useState(false);
    const [paletteOpen, setPaletteOpen] = useState(false);
    const [secondPane, setSecondPane] = useState(null); // null | 'graph' | 'editor'
    const [splitTab, setSplitTab] = useState(null);
    const [splitW, setSplitW] = useState(480);
    const [activePane, setActivePane] = useState("a");
    const [view, setView] = useState("home"); // 'home' | 'workspace'
    const [conflictOpen, setConflictOpen] = useState(false);
    const [conflictResolved, setConflictResolved] = useState(false);
    const [pluginsOpen, setPluginsOpen] = useState(false);
    const [syncOpen, setSyncOpen] = useState(false);
    const [modal, setModal] = useState(null); // null | 'digest' | 'goals' | 'contra'
    const [settingsOpen, setSettingsOpen] = useState(false);
    const [toasts, setToasts] = useState([]);
    const [indexing, setIndexing] = useState({ active: true, done: 320, total: 1200 });
    const [sidebarW, setSidebarW] = useState(260);
    const [aiW, setAiW] = useState(360);
    const [resizing, setResizing] = useState(false);

    const t = window.NEXUS_I18N[lang];
    const activePaneRef = useRef("a"); activePaneRef.current = activePane;
    const secondPaneRef = useRef(null); secondPaneRef.current = secondPane;
    const activeTabRef = useRef(null); activeTabRef.current = activeTab;
    const tabsBRef = useRef([]); tabsBRef.current = tabsB;
    const splitTabRef = useRef(null); splitTabRef.current = splitTab;

    // apply theme + token overrides
    useEffect(() => {
      const el = document.documentElement;
      el.classList.add("theme-anim");
      el.setAttribute("data-theme", theme);
      localStorage.setItem("nexus-theme", theme);
      const tm = setTimeout(() => el.classList.remove("theme-anim"), 380);
      return () => clearTimeout(tm);
    }, [theme]);
    useEffect(() => { localStorage.setItem("nexus-lang", lang); }, [lang]);
    useEffect(() => {
      const r = document.documentElement.style;
      r.setProperty("--chrome", tw.chrome === "minimal" ? "0" : "1");
      document.documentElement.setAttribute("data-accent", tw.accent);
      r.setProperty("--editor-font", EDITOR_FONTS[tw.editorFont]);
    }, [tw.chrome, tw.accent, tw.editorFont]);
    // adaptive density (compact | comfortable | auto-by-width)
    useEffect(() => {
      const r = document.documentElement.style;
      const apply = () => {
        let mode = tw.density;
        if (mode === "auto") mode = window.innerWidth < 1180 ? "compact" : "comfortable";
        r.setProperty("--density", mode === "compact" ? "0.82" : "1");
        r.setProperty("--row-h", mode === "compact" ? "24px" : "28px");
      };
      apply();
      if (tw.density === "auto") { window.addEventListener("resize", apply); return () => window.removeEventListener("resize", apply); }
    }, [tw.density]);
    useEffect(() => { document.documentElement.style.setProperty("--sidebar-w", sidebarW + "px"); }, [sidebarW]);
    useEffect(() => { document.documentElement.style.setProperty("--ai-w", aiW + "px"); }, [aiW]);

    // index ticker
    useEffect(() => {
      if (!indexing.active) return;
      const iv = setInterval(() => setIndexing((s) => {
        if (!s.active) return s;
        const done = Math.min(s.total, s.done + 40);
        return done >= s.total ? { active: false, done: s.total, total: s.total } : { ...s, done };
      }), 600);
      return () => clearInterval(iv);
    }, [indexing.active]);

    const toast = useCallback((text, kind) => {
      const id = Math.random(); setToasts((ts) => [...ts, { id, text, kind: kind || "ok" }]);
      setTimeout(() => setToasts((ts) => ts.filter((x) => x.id !== id)), 2600);
    }, []);

    const openNote = useCallback((id) => {
      setView("workspace");
      // new/opened notes land in the ACTIVE pane only
      if (activePaneRef.current === "b" && secondPaneRef.current === "editor") {
        setTabsB((tb) => tb.includes(id) ? tb : [...tb, id]);
        setSplitTab(id);
      } else {
        setTabsA((ta) => ta.includes(id) ? ta : [...ta, id]);
        setActiveTab(id);
      }
    }, []);
    const toggleGraph = useCallback(() => {
      setView("workspace");
      setSecondPane((p) => { if (p === "graph") return null; setActivePane("a"); return "graph"; });
    }, []);
    const openSplit = useCallback(() => {
      // open a second editor pane seeded with ONLY the current note (no tab duplication)
      const cur = activeTabRef.current;
      setTabsB((tb) => tb.length ? tb : (cur ? [cur] : []));
      setSplitTab((s) => s || cur);
      setSecondPane("editor"); setActivePane("b");
    }, []);
    const closeSecond = useCallback(() => { setSecondPane(null); setActivePane("a"); }, []);
    const closeTabIn = useCallback((pane, id) => {
      if (pane === "b") {
        setTabsB((tb) => { const i = tb.indexOf(id); const next = tb.filter((x) => x !== id);
          setSplitTab((cur) => cur === id ? (next[Math.max(0, i - 1)] || null) : cur); return next; });
      } else {
        setTabsA((ta) => { const i = ta.indexOf(id); const next = ta.filter((x) => x !== id);
          setActiveTab((cur) => cur === id ? (next[Math.max(0, i - 1)] || null) : cur); return next; });
      }
    }, []);
    // drag a tab from one pane into the other
    const moveTab = useCallback((id, fromPane, toPane) => {
      if (!id || fromPane === toPane) return;
      if (fromPane === "a") setTabsA((ta) => { const i = ta.indexOf(id); const next = ta.filter((x) => x !== id); setActiveTab((cur) => cur === id ? (next[Math.max(0, i - 1)] || null) : cur); return next; });
      else setTabsB((tb) => { const i = tb.indexOf(id); const next = tb.filter((x) => x !== id); setSplitTab((cur) => cur === id ? (next[Math.max(0, i - 1)] || null) : cur); return next; });
      if (toPane === "a") { setTabsA((ta) => ta.includes(id) ? ta : [...ta, id]); setActiveTab(id); setActivePane("a"); }
      else { setTabsB((tb) => tb.includes(id) ? tb : [...tb, id]); setSplitTab(id); setActivePane("b"); }
    }, []);
    const appendLineToTab = useCallback((id, line) => {
      if (!id) return;
      setExtraBodies((e) => ({ ...e, [id]: (e[id] || "") + "\n" + line }));
      setDirtyMap((d) => ({ ...d, [id]: true }));
    }, []);
    const onTag = useCallback((tag) => toast((lang === "ru" ? "Фильтр по тегу #" : "Filter by tag #") + tag, "ok"), [lang]);
    // edit raw markdown source
    const editBody = useCallback((id, text) => {
      if (!id) return;
      setEditedBodies((e) => ({ ...e, [id]: text }));
      setDirtyMap((d) => ({ ...d, [id]: true }));
    }, []);
    // create a fresh note in the given (or active) pane only
    const newNote = useCallback((pane) => {
      setView("workspace");
      const p = pane || activePaneRef.current;
      const id = "untitled-" + Date.now();
      const title = lang === "ru" ? "Без названия" : "Untitled";
      window.NEXUS_NOTES[id] = { title, mtime: lang === "ru" ? "только что / now" : "now", words: 0, tags: [], body: "# " + title + "\n\n" };
      if (p === "b" && secondPaneRef.current === "editor") { setTabsB((tb) => [...tb, id]); setSplitTab(id); setActivePane("b"); }
      else { setTabsA((ta) => [...ta, id]); setActiveTab(id); setActivePane("a"); }
      setDirtyMap((d) => ({ ...d, [id]: true }));
    }, [lang]);
    // drop a dragged tab onto a pane
    const paneDropProps = (pane) => ({
      onDragOver: (e) => { if ((e.dataTransfer.types || []).includes("text/nexus-tab")) { e.preventDefault(); e.dataTransfer.dropEffect = "move"; if (dropPane !== pane) setDropPane(pane); } },
      onDragLeave: (e) => { if (!e.currentTarget.contains(e.relatedTarget)) setDropPane((d) => d === pane ? null : d); },
      onDrop: (e) => { const raw = e.dataTransfer.getData("text/nexus-tab"); setDropPane(null); if (raw) { e.preventDefault(); try { const o = JSON.parse(raw); moveTab(o.id, o.pane, pane); } catch (_) {} } },
    });

    // keyboard: Cmd/Ctrl+K
    useEffect(() => {
      const h = (e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === ",") { e.preventDefault(); setSettingsOpen((v) => !v); }
        if ((e.metaKey || e.ctrlKey) && e.key === "/") { e.preventDefault(); window.dispatchEvent(new CustomEvent("nexus-inline-ai")); }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setPaletteOpen((v) => !v); }
        if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "g") { e.preventDefault(); toggleGraph(); }
        if ((e.metaKey || e.ctrlKey) && e.key === "\\") { e.preventDefault(); secondPaneRef.current === "editor" ? closeSecond() : openSplit(); }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "r") { e.preventDefault(); setReading((v) => !v); }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "e") { e.preventDefault();
          if (activePaneRef.current === "b" && secondPaneRef.current === "editor") setModeB((m) => m === "edit" ? "preview" : "edit");
          else setModeA((m) => m === "edit" ? "preview" : "edit");
        }
        if (e.key === "Escape") { setReading((v) => v ? false : v); }
        if ((e.metaKey || e.ctrlKey) && e.key === "s") { e.preventDefault(); setDirtyMap((d) => ({ ...d, [activeTab]: false })); toast(lang === "ru" ? "Сохранено" : "Saved"); }
      };
      window.addEventListener("keydown", h); return () => window.removeEventListener("keydown", h);
    }, [activeTab, lang, toast]);

    const commands = [
      { label: lang === "ru" ? "На главную (Home)" : "Go to Home", icon: "home", run: () => setView("home") },
      { label: lang === "ru" ? "Новости" : "News", icon: "newspaper", run: () => setView("news") },
      { label: lang === "ru" ? "Переключить тему" : "Toggle theme", icon: "sun", hint: "⌘⇧T", run: () => setTheme((x) => x === "dark" ? "light" : "dark") },
      { label: lang === "ru" ? "Переключить язык (RU/EN)" : "Toggle language", icon: "languages", run: () => setLang((x) => x === "ru" ? "en" : "ru") },
      { label: lang === "ru" ? "AI-ассистент" : "Toggle AI panel", icon: "sparkles", run: () => setAiOpen((v) => !v) },
      { label: t.graph_view, icon: "graph", hint: "⌘⇧G", run: () => toggleGraph() },
      { label: t.split_right, icon: "panel-right", hint: "⌘\\", run: () => openSplit() },
      { label: t.reading_mode, icon: "book-open", hint: "⌘R", run: () => setReading((v) => !v) },
      { label: lang === "ru" ? "Редактирование / Просмотр" : "Edit / Preview", icon: "pencil", hint: "⌘E", run: () => {
        if (activePaneRef.current === "b" && secondPaneRef.current === "editor") setModeB((m) => m === "edit" ? "preview" : "edit");
        else setModeA((m) => m === "edit" ? "preview" : "edit");
      } },
      { label: lang === "ru" ? "Свернуть боковую панель" : "Toggle sidebar", icon: "panel-left", run: () => setSidebarOpen((v) => !v) },
      { label: t.open_settings, icon: "settings", run: () => setSettingsOpen(true) },
      { label: lang === "ru" ? "Менеджер плагинов" : "Plugin manager", icon: "puzzle", run: () => setPluginsOpen(true) },
      { label: lang === "ru" ? "Синхронизация (git)" : "Sync (git)", icon: "git-branch", run: () => setSyncOpen(true) },
      { label: lang === "ru" ? "Дайджест изменений" : "Change digest", icon: "newspaper", run: () => setModal("digest") },
      { label: lang === "ru" ? "Цели" : "Goals", icon: "target", run: () => setModal("goals") },
      { label: lang === "ru" ? "Поиск противоречий" : "Find contradictions", icon: "scale", run: () => setModal("contra") },
      { label: lang === "ru" ? "Запустить онбординг заново" : "Re-run onboarding", icon: "sparkles", run: () => { window.location.href = "Onboarding.html"; } },
      ...(conflictResolved ? [] : [{ label: lang === "ru" ? "Разрешить конфликт синхронизации" : "Resolve sync conflict", icon: "git-merge", run: () => setConflictOpen(true) }]),
    ];

    // resizers
    const drag = (which) => (e) => {
      e.preventDefault(); setResizing(true);
      const move = (ev) => {
        if (which === "left") setSidebarW(Math.max(180, Math.min(420, ev.clientX)));
        else setAiW(Math.max(280, Math.min(560, window.innerWidth - ev.clientX)));
      };
      const up = () => { setResizing(false); window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); document.body.style.cursor = ""; };
      window.addEventListener("mousemove", move); window.addEventListener("mouseup", up); document.body.style.cursor = "col-resize";
    };
    // split pane resizer — shrink/grow the second pane (graph or editor)
    const splitDrag = (e) => {
      e.preventDefault(); setResizing(true);
      const rightEdge = window.innerWidth - (aiSide ? aiW : 0);
      const move = (ev) => setSplitW(Math.max(300, Math.min(rightEdge - 360, rightEdge - ev.clientX)));
      const up = () => { setResizing(false); window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); document.body.style.cursor = ""; };
      window.addEventListener("mousemove", move); window.addEventListener("mouseup", up); document.body.style.cursor = "col-resize";
    };

    const aiSide = aiOpen && tw.aiLayout === "side" && view === "workspace";
    const aiBottom = aiOpen && tw.aiLayout === "bottom" && view === "workspace";
    const aiOverlay = aiOpen && tw.aiLayout === "overlay" && view === "workspace";

    const aiProps = { t, lang, srcStyle: tw.ragSources, offline: tw.offline, cloudMode: tw.cloud, onOpen: openNote, activeNoteId: activeTab };

    const bodyCls = "app-body"
      + (aiSide ? " with-ai-side" : "")
      + (aiBottom ? " with-ai-bottom" : "")
      + (resizing ? " resizing" : "")
      + (!sidebarOpen ? " sidebar-collapsed" : "");

    return React.createElement("div", { className: "app" + (reading ? " reading" : "") },
      React.createElement(Titlebar, { t, theme, setTheme, lang, setLang, platform: tw.platform, onOpenPalette: () => setPaletteOpen(true), aiOpen, setAiOpen, onHome: () => setView("home"), onDigest: () => setModal("digest"), onGoals: () => setModal("goals"), onContra: () => setModal("contra"), reading, setReading }),
      React.createElement("div", { className: "app-shell" },
        React.createElement(ActivityBar, { t, lang, view, onHome: () => setView("home"), onNews: () => setView("news"), sidebarOpen, setSidebarOpen, graphOpen: secondPane === "graph", setGraphOpen: toggleGraph, reading, setReading, onSync: () => setSyncOpen(true), syncConflict: !conflictResolved, onSettings: () => setSettingsOpen(true) }),
      React.createElement("div", { className: bodyCls },
        React.createElement(window.Sidebar, { t, activeId: activeTab, onOpen: openNote, onTag, view, onHome: () => setView("home"), onNewNote: () => newNote("a") }),
        view === "home"
          ? React.createElement(window.Home, { t, lang, onOpenNote: openNote, onNewNote: () => newNote("a"), onGraph: toggleGraph, onSearch: () => setPaletteOpen(true), toast })
          : view === "news"
          ? React.createElement(window.News, { t, lang, toast, offline: tw.offline })
          : React.createElement("div", { className: "editor-split" + (secondPane && !reading ? " split" : "") },
          React.createElement("div", {
            className: "pane" + (secondPane && !reading && activePane === "a" ? " focused" : "") + (dropPane === "a" ? " drop-target" : ""),
            onMouseDownCapture: () => setActivePane("a"),
            ...paneDropProps("a"),
          },
            React.createElement(window.EditorArea, {
              t, lang, tabs: tabsA, activeTab, dirtyMap, onActivate: setActiveTab, onClose: (id) => closeTabIn("a", id),
              onAdd: () => newNote("a"), onOpen: openNote, onTag,
              extraBody: extraBodies[activeTab], editedBody: editedBodies[activeTab], onAppend: (l) => appendLineToTab(activeTab, l),
              onSplit: openSplit, pane: "a", mode: modeA, onToggleMode: () => setModeA((m) => m === "edit" ? "preview" : "edit"), onEditBody: editBody,
            })),
          (secondPane && !reading) ? React.createElement("div", { className: "split-resizer", onMouseDown: splitDrag }) : null,
          (secondPane === "graph" && !reading) ? React.createElement("div", { className: "pane graph-pane", style: { width: splitW, flex: "0 0 " + splitW + "px" } },
            React.createElement(window.GraphView, { t, lang, activeId: activeTab, onOpen: (id) => { setActivePane("a"); openNote(id); }, onClose: closeSecond })) : null,
          (secondPane === "editor" && !reading) ? React.createElement("div", {
            className: "pane" + (activePane === "b" ? " focused" : "") + (dropPane === "b" ? " drop-target" : ""), style: { width: splitW, flex: "0 0 " + splitW + "px" },
            onMouseDownCapture: () => setActivePane("b"),
            ...paneDropProps("b"),
          },
            React.createElement(window.EditorArea, {
              t, lang, tabs: tabsB, activeTab: splitTab, dirtyMap, onActivate: setSplitTab, onClose: (id) => closeTabIn("b", id),
              onAdd: () => newNote("b"), onOpen: openNote, onTag,
              extraBody: extraBodies[splitTab], editedBody: editedBodies[splitTab], onAppend: (l) => appendLineToTab(splitTab, l),
              secondary: true, onClosePane: closeSecond, pane: "b", mode: modeB, onToggleMode: () => setModeB((m) => m === "edit" ? "preview" : "edit"), onEditBody: editBody,
            })) : null),
        aiSide ? React.createElement(window.AIPanel, aiProps) : null,
        aiBottom ? React.createElement(window.AIPanel, aiProps) : null,
        sidebarOpen ? React.createElement("div", { className: "resizer left", onMouseDown: drag("left") }) : null,
        aiSide ? React.createElement("div", { className: "resizer right", onMouseDown: drag("right") }) : null,
        aiOverlay ? React.createElement("div", { className: "ai-overlay-scrim", onMouseDown: (e) => { if (e.target === e.currentTarget) setAiOpen(false); } },
          React.createElement(window.AIPanel, { ...aiProps, overlay: true, onClose: () => setAiOpen(false) })) : null,
      )),
      React.createElement(StatusBar, { t, indexing, conflict: !conflictResolved, onConflict: () => setConflictOpen(true), lang }),
      paletteOpen ? React.createElement(window.CommandPalette, { t, style: tw.paletteStyle, onClose: () => setPaletteOpen(false), onOpenNote: openNote, commands }) : null,
      conflictOpen ? React.createElement(window.ConflictResolver, {
        lang, onClose: () => setConflictOpen(false),
        onResolved: () => { setConflictOpen(false); setConflictResolved(true); toast(lang === "ru" ? "Конфликт разрешён · сохранено" : "Conflict resolved · saved"); },
      }) : null,
      pluginsOpen ? React.createElement(window.PluginManager, { lang, onClose: () => setPluginsOpen(false), toast }) : null,
      syncOpen ? React.createElement(window.SyncPanel, { lang, onClose: () => setSyncOpen(false), hasConflict: !conflictResolved, onResolveConflict: () => setConflictOpen(true), toast }) : null,
      modal === "digest" ? React.createElement(window.DigestPanel, { lang, onClose: () => setModal(null) }) : null,
      modal === "goals" ? React.createElement(window.GoalsPanel, { lang, onClose: () => setModal(null), onOpenNote: (id) => { setModal(null); openNote(id); } }) : null,
      settingsOpen ? React.createElement(window.Settings, {
        lang, setLang, theme, setTheme,
        accent: tw.accent, setAccent: (v) => setTweak("accent", v),
        density: tw.density, setDensity: (v) => setTweak("density", v),
        editorFont: tw.editorFont, setEditorFont: (v) => setTweak("editorFont", v),
        toast, onClose: () => setSettingsOpen(false),
      }) : null,
      React.createElement("div", { className: "toast-wrap" },
        toasts.map((ts) => React.createElement("div", { key: ts.id, className: "toast" },
          React.createElement(Icon, { name: ts.kind === "bad" ? "alert" : "check", size: 15, className: "ico " + ts.kind }), ts.text))),
      React.createElement(TweaksUI, { tw, setTweak }),
    );
  }

  function TweaksUI({ tw, setTweak }) {
    const { TweaksPanel, TweakSection, TweakRadio, TweakSelect, TweakToggle, TweakRow } = window;
    return React.createElement(TweaksPanel, { title: "Tweaks" },
      React.createElement(TweakSection, { label: "Общая подача / Look" }),
      React.createElement(TweakSelect, { label: "Плотность / Density", value: tw.density, options: ["compact", "comfortable", "auto"], onChange: (v) => setTweak("density", v) }),
      React.createElement(TweakRadio, { label: "Хром / Chrome", value: tw.chrome, options: ["minimal", "standard"], onChange: (v) => setTweak("chrome", v) }),
      React.createElement(TweakRow, { label: "Акцент / Accent" },
        React.createElement("div", { style: { display: "flex", gap: 6 } },
          ACCENTS.map((a) => React.createElement("button", {
            key: a.id, title: a.id, onClick: () => setTweak("accent", a.id),
            style: {
              width: 22, height: 22, borderRadius: 6, cursor: "pointer",
              background: a.swatch,
              border: tw.accent === a.id ? "2px solid var(--color-text)" : "2px solid transparent",
              boxShadow: tw.accent === a.id ? "0 0 0 1px var(--color-text)" : "none",
            },
          })))),
      React.createElement(TweakSection, { label: "AI-панель / AI panel" }),
      React.createElement(TweakRadio, { label: "Layout", value: tw.aiLayout, options: ["side", "bottom", "overlay"], onChange: (v) => setTweak("aiLayout", v) }),
      React.createElement(TweakSelect, { label: "Источники RAG / Sources", value: tw.ragSources, options: ["cards", "chips", "footnotes"], onChange: (v) => setTweak("ragSources", v) }),
      React.createElement(TweakSection, { label: "Редактор / Editor" }),
      React.createElement(TweakRadio, { label: "Шрифт / Font", value: tw.editorFont, options: ["sans", "serif", "mono"], onChange: (v) => setTweak("editorFont", v) }),
      React.createElement(TweakSection, { label: "Палитра команд / Palette" }),
      React.createElement(TweakRadio, { label: "Стиль / Style", value: tw.paletteStyle, options: ["top", "center", "spotlight"], onChange: (v) => setTweak("paletteStyle", v) }),
      React.createElement(TweakSection, { label: "Демо-состояния / Demo states" }),
      React.createElement(TweakRadio, { label: "Платформа / Platform", value: tw.platform, options: ["mac", "win"], onChange: (v) => setTweak("platform", v) }),
      React.createElement(TweakToggle, { label: "LLM offline", value: tw.offline, onChange: (v) => setTweak("offline", v) }),
      React.createElement(TweakToggle, { label: "Ответ из облака / Cloud", value: tw.cloud, onChange: (v) => setTweak("cloud", v) }),
    );
  }

  window.NexusApp = App;
})();
