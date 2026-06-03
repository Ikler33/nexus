// ai-panel.jsx — Chat (empty/streaming/sources/offline/cloud), Suggestions, Summary.
(function () {
  const { useState, useRef, useEffect } = React;
  const Icon = window.Icon;

  function Provider({ t, cloud }) {
    return React.createElement("span", { className: "provider " + (cloud ? "cloud" : "local") },
      React.createElement(Icon, { name: cloud ? "cloud" : "drive", size: 12, className: "ico" }),
      cloud ? "☁ " + t.cloud : t.local,
    );
  }

  function Sources({ t, ids, style, onOpen }) {
    const notes = window.NEXUS_NOTES;
    if (!ids || !ids.length) return null;
    if (style === "chips") {
      return React.createElement("div", { className: "src-chips" },
        ids.map((id, i) => React.createElement("button", { key: id, className: "src-chip", onClick: () => onOpen(id) },
          React.createElement("span", { className: "num" }, i + 1),
          React.createElement(Icon, { name: "file-text", size: 12, className: "ico" }),
          notes[id].title)));
    }
    if (style === "footnotes") {
      return React.createElement("div", { className: "src-foot" },
        React.createElement("div", { style: { fontSize: "var(--text-xs)", color: "var(--color-text-faint)", marginBottom: 2 } }, t.sources),
        ids.map((id, i) => React.createElement("div", { key: id, className: "sf-row", onClick: () => onOpen(id) },
          React.createElement("span", { className: "sf-num" }, "[" + (i + 1) + "]"),
          React.createElement("span", null, notes[id].title))));
    }
    // cards (default)
    return React.createElement("div", { className: "src-cards" },
      ids.map((id, i) => React.createElement("div", { key: id, className: "src-card", onClick: () => onOpen(id) },
        React.createElement("span", { className: "sc-num" }, i + 1),
        React.createElement("div", { style: { minWidth: 0 } },
          React.createElement("div", { className: "sc-title" }, notes[id].title),
          React.createElement("div", { className: "sc-ctx" }, notes[id].body.split("\n").filter(Boolean)[1] || notes[id].body.slice(0, 60))))));
  }

  function ChatTab({ t, lang, srcStyle, offline, onOpen, cloudMode }) {
    const [msgs, setMsgs] = useState([]);
    const [input, setInput] = useState("");
    const [streaming, setStreaming] = useState(false);
    const timer = useRef(null);
    const thinkRef = useRef(null);
    const bodyRef = useRef(null);
    const stickRef = useRef(true);

    useEffect(() => () => { if (timer.current) cancelAnimationFrame(timer.current); clearTimeout(thinkRef.current); }, []);
    useEffect(() => {
      const el = bodyRef.current; if (el && stickRef.current) el.scrollTop = el.scrollHeight;
    });

    function onScroll() {
      const el = bodyRef.current; if (!el) return;
      stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    }

    function send(text) {
      const q = (text ?? input).trim(); if (!q || streaming) return;
      setInput(""); stickRef.current = true;
      const userMsg = { role: "user", text: q };
      if (offline) {
        setMsgs((m) => [...m, userMsg, { role: "ai", offline: true }]);
        return;
      }
      const ans = window.mockAnswer(q, lang);
      const cloud = cloudMode;
      const full = ans.text;
      // 1) thinking phase — animated brand mark while the model "reasons"
      setMsgs((m) => [...m, userMsg, { role: "ai", thinking: true, sources: ans.sources, cloud }]);
      setStreaming(true);
      thinkRef.current = setTimeout(() => {
        setMsgs((m) => { const c = [...m]; c[c.length - 1] = { ...c[c.length - 1], thinking: false, text: "", streaming: true }; return c; });
        // 2) stream character-by-character, time-based for a smooth, even reveal
        const CPS = 62;            // characters per second
        let shown = 0, last = null;
        const tick = (ts) => {
          if (last == null) last = ts;
          const dt = (ts - last) / 1000; last = ts;
          // ease the speed slightly so punctuation/newlines feel natural
          shown = Math.min(full.length, shown + dt * CPS);
          const n = Math.floor(shown);
          const partial = full.slice(0, n);
          setMsgs((m) => { const c = [...m]; const lm = c[c.length - 1]; c[c.length - 1] = { ...lm, text: partial }; return c; });
          if (n >= full.length) {
            timer.current = null;
            setMsgs((m) => { const c = [...m]; c[c.length - 1] = { ...c[c.length - 1], streaming: false }; return c; });
            setStreaming(false);
            return;
          }
          timer.current = requestAnimationFrame(tick);
        };
        timer.current = requestAnimationFrame(tick);
      }, 1300);
    }
    function stop() {
      if (timer.current) cancelAnimationFrame(timer.current);
      clearTimeout(thinkRef.current); setStreaming(false);
      setMsgs((m) => { const c = [...m]; const last = c[c.length - 1]; if (last && (last.streaming || last.thinking)) c[c.length - 1] = { ...last, streaming: false, thinking: false, stopped: true }; return c; });
    }

    const suggestions = lang === "ru"
      ? ["Как устроен RAG в Nexus?", "Что такое эмбеддинги?", "Объясни принцип local-first"]
      : ["How does RAG work in Nexus?", "What are embeddings?", "Explain local-first"];

    return React.createElement("div", { style: { display: "flex", flexDirection: "column", minHeight: 0, flex: 1 } },
      React.createElement("div", { className: "ai-body", ref: bodyRef, onScroll, "aria-live": "polite" },
        msgs.length === 0
          ? React.createElement("div", { className: "chat-empty" },
              React.createElement("div", { className: "glyph" }, React.createElement(Icon, { name: "sparkles", size: 24 })),
              React.createElement("div", { className: "ce-title" }, t.ai_empty_title),
              React.createElement("div", { className: "ce-sub" }, t.ai_empty_sub),
              React.createElement("div", { className: "chat-suggest" },
                suggestions.map((s) => React.createElement("button", { key: s, className: "suggest-pill", onClick: () => send(s) }, s))),
            )
          : msgs.map((m, i) =>
              m.role === "user"
                ? React.createElement("div", { key: i, className: "msg user" },
                    React.createElement("div", { className: "bubble" }, m.text))
                : React.createElement("div", { key: i, className: "msg ai" },
                    m.offline
                      ? React.createElement("div", { className: "ai-banner danger" },
                          React.createElement(Icon, { name: "alert", size: 16, className: "ico" }),
                          React.createElement("div", null,
                            React.createElement("div", { className: "b-title" }, t.llm_offline),
                            React.createElement("div", { className: "b-sub" }, t.llm_offline_sub)))
                      : m.thinking
                        ? React.createElement("div", { className: "msg-thinking" },
                            React.createElement(window.BrandThinking, { size: 30 }),
                            React.createElement("span", { className: "mt-label" }, t.thinking))
                      : React.createElement(React.Fragment, null,
                          m.cloud && !m.streaming ? React.createElement("div", { className: "ai-banner warn", style: { marginBottom: 8 } },
                            React.createElement(Icon, { name: "cloud", size: 15, className: "ico" }),
                            React.createElement("div", null, React.createElement("span", { className: "b-title" }, "☁ " + t.cloud_answer))) : null,
                          React.createElement("div", { className: "bubble" },
                            ...window.renderInline(m.text, "ai" + i, { onLink: onOpen, onTag: () => {} }),
                            m.streaming ? React.createElement("span", { className: "cursor-blink" }) : null),
                          !m.streaming ? React.createElement(React.Fragment, null,
                            React.createElement(Sources, { t, ids: m.sources, style: srcStyle, onOpen }),
                            React.createElement("div", { className: "msg-meta" },
                              React.createElement(Provider, { t, cloud: m.cloud }),
                              m.stopped ? React.createElement("span", null, "остановлено / stopped") : null)) : null,
                        ))
            ),
      ),
      React.createElement("div", { className: "ai-composer" },
        React.createElement("div", { className: "composer-box" },
          React.createElement("textarea", {
            rows: 1, value: input, placeholder: t.ask_anything,
            onChange: (e) => setInput(e.target.value),
            onKeyDown: (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); } },
          }),
          streaming
            ? React.createElement("button", { className: "send-btn stop", onClick: stop, title: t.stop }, React.createElement(Icon, { name: "stop", size: 15 }))
            : React.createElement("button", { className: "send-btn", onClick: () => send(), disabled: !input.trim(), title: "Enter" }, React.createElement(Icon, { name: "arrow-up", size: 16 })),
        ),
        React.createElement("div", { className: "composer-foot" },
          streaming
            ? React.createElement("span", { className: "cf-status" },
                React.createElement("span", { className: "cf-pulse" }), t.thinking)
            : React.createElement("span", { className: "cf-hint" },
                React.createElement("span", { className: "kbd cf-kbd" }, "↵"), lang === "ru" ? "отправить" : "to send")),
      ),
    );
  }

  function SuggestionsTab({ t, activeNoteId, onOpen }) {
    const [list, setList] = useState(() => window.mockSuggestions(activeNoteId));
    const [busy, setBusy] = useState(false);
    const [leaving, setLeaving] = useState({});
    useEffect(() => { setList(window.mockSuggestions(activeNoteId)); setLeaving({}); }, [activeNoteId]);
    function remove(id, cb) {
      setLeaving((l) => ({ ...l, [id]: true }));
      setTimeout(() => { setList((l) => l.filter((x) => x.id !== id)); cb && cb(); }, 300);
    }
    function recompute() { setBusy(true); setTimeout(() => { setList(window.mockSuggestions(activeNoteId)); setLeaving({}); setBusy(false); }, 900); }
    return React.createElement("div", { className: "ai-body" },
      React.createElement("div", { style: { fontSize: "var(--text-sm)", color: "var(--color-text-muted)", display: "flex", alignItems: "center", gap: 8 } },
        React.createElement("span", { style: { flex: 1 } }, t.ai_suggest_intro),
        React.createElement("button", { className: "sg-btn", style: { flex: "none", padding: "0 10px", height: 26 }, onClick: recompute },
          React.createElement(Icon, { name: "refresh", size: 13 }), t.recompute)),
      busy ? React.createElement("div", { className: "msg-thinking", style: { padding: "var(--space-4) 4px" } },
        React.createElement(window.BrandThinking, { size: 24 }), React.createElement("span", { className: "mt-label" }, t.compute_links)) : null,
      !busy && list.length === 0 ? React.createElement("div", { className: "cmd-empty" }, t.no_suggestions) : null,
      !busy && list.map((s) => {
        const n = window.NEXUS_NOTES[s.id];
        return React.createElement("div", { key: s.id, className: "sg-card" + (leaving[s.id] ? " m-leave" : "") },
          React.createElement("div", { className: "sg-top" },
            React.createElement(Icon, { name: "link", size: 14, style: { color: "var(--color-link)" } }),
            React.createElement("span", { className: "sg-title" }, n.title),
            React.createElement("span", { className: "sg-score" }, s.score.toFixed(2))),
          React.createElement("div", { className: "sg-reason" }, t.suggestion_reason),
          React.createElement("div", { className: "sg-actions" },
            React.createElement("button", { className: "sg-btn primary", onClick: () => remove(s.id, () => onOpen(s.id)) },
              React.createElement(Icon, { name: "check", size: 13 }), t.accept),
            React.createElement("button", { className: "sg-btn", onClick: () => remove(s.id) },
              React.createElement(Icon, { name: "x", size: 13 }), t.dismiss)),
        );
      }),
    );
  }

  function SummaryTab({ t, activeNoteId }) {
    const n = window.NEXUS_NOTES[activeNoteId];
    const [gen, setGen] = useState(true);
    useEffect(() => { setGen(true); const tm = setTimeout(() => setGen(false), 1400); return () => clearTimeout(tm); }, [activeNoteId]);
    const pts = n.body.split("\n").filter((l) => /^[-*]\s|^\d+\.\s/.test(l)).slice(0, 4).map((l) => l.replace(/^[-*\d.]+\s/, ""));
    if (gen) return React.createElement("div", { className: "ai-body", style: { alignItems: "center", justifyContent: "center", flex: 1 } },
      React.createElement("div", { className: "msg-thinking", style: { flexDirection: "column", gap: 14, padding: "var(--space-7) 0" } },
        React.createElement(window.BrandThinking, { size: 46 }),
        React.createElement("span", { className: "mt-label" }, t.thinking)));
    return React.createElement("div", { className: "ai-body" },
      React.createElement("div", { className: "sg-card" },
        React.createElement("div", { className: "sg-top" }, React.createElement(Icon, { name: "sparkles", size: 14, style: { color: "var(--color-ai)" } }),
          React.createElement("span", { className: "sg-title", style: { color: "var(--color-text)" } }, n.title)),
        React.createElement("div", { style: { fontSize: "var(--text-md)", lineHeight: "var(--leading-normal)", color: "var(--color-text-muted)" } },
          (pts.length ? pts : ["Заметка о " + n.title]).map((p, i) => React.createElement("div", { key: i, style: { display: "flex", gap: 8, margin: "5px 0" } },
            React.createElement(Icon, { name: "dot", size: 12, style: { color: "var(--color-ai)", marginTop: 4 } }), p)))),
      React.createElement("div", { style: { fontSize: "var(--text-xs)", color: "var(--color-text-faint)", textAlign: "center" } }, "Сгенерировано локально / Generated locally"));
  }

  function AIPanel(props) {
    const { t, onClose, overlay } = props;
    const [tab, setTab] = useState("chat");
    return React.createElement("div", { className: overlay ? "ai-overlay" : "ai-panel" },
      React.createElement("div", { className: "ai-head" },
        React.createElement("div", { className: "ai-title" }, React.createElement(Icon, { name: "sparkles", size: 16, className: "ico" }), t.ai_assistant),
        React.createElement("div", { className: "spacer" }),
        React.createElement(Provider, { t, cloud: props.cloudMode }),
        onClose ? React.createElement("button", { className: "tb-btn", onClick: onClose, "aria-label": t.close }, React.createElement(Icon, { name: "x", size: 15 })) : null,
      ),
      React.createElement("div", { className: "ai-tabs" },
        [["chat", t.chat], ["suggestions", t.suggestions], ["summary", t.summary]].map(([id, label]) =>
          React.createElement("div", { key: id, className: "ai-tab" + (tab === id ? " active" : ""), onClick: () => setTab(id) }, label))),
      tab === "chat" && React.createElement(ChatTab, props),
      tab === "suggestions" && React.createElement(SuggestionsTab, props),
      tab === "summary" && React.createElement(SummaryTab, props),
    );
  }
  window.AIPanel = AIPanel;
})();
