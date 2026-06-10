// sidebar.jsx — rail switch + FileTree / Search / Tags panels.
(function () {
  const { useState, useMemo } = React;
  const Icon = window.Icon;

  function flatten(nodes, depth, openMap, path) {
    let rows = [];
    nodes.forEach((n, i) => {
      const key = path + "/" + (n.name || n.id) + i;
      if (n.type === "folder") {
        const open = openMap[key] ?? n.open;
        rows.push({ ...n, depth, key, open, isFolder: true });
        if (open) rows = rows.concat(flatten(n.children, depth + 1, openMap, key));
      } else {
        rows.push({ ...n, depth, key, isFolder: false });
      }
    });
    return rows;
  }

  function FileTree({ t, activeId, onOpen }) {
    const [openMap, setOpenMap] = useState({});
    const rows = useMemo(() => flatten(window.NEXUS_VAULT, 0, openMap, "root"), [openMap]);
    return React.createElement("div", { role: "tree", "aria-label": t.explorer },
      rows.map((r) =>
        React.createElement("div", {
          key: r.key, role: "treeitem",
          "aria-expanded": r.isFolder ? !!r.open : undefined,
          className: "tree-row" + (!r.isFolder && r.id === activeId ? " active" : ""),
          style: { "--depth": r.depth },
          onClick: () => r.isFolder
            ? setOpenMap((m) => ({ ...m, [r.key]: !(m[r.key] ?? r.open) }))
            : onOpen(r.id),
        },
          r.isFolder
            ? React.createElement("span", { className: "twist" + (r.open ? " open" : "") }, React.createElement(Icon, { name: "chevron", size: 13 }))
            : React.createElement("span", { className: "twist" }),
          React.createElement(Icon, {
            name: r.isFolder ? (r.open ? "folder-open" : "folder") : "file-text",
            size: 15, className: r.isFolder ? "ico-folder" : "ico-file",
          }),
          React.createElement("span", { className: "name" }, r.name.replace(/\.md$/, "")),
          r.tag === "★" ? React.createElement("span", { className: "star" }, "★") : null,
        )
      )
    );
  }

  function SearchPanel({ t, onOpen }) {
    const [q, setQ] = useState("");
    const notes = window.NEXUS_NOTES;
    const results = useMemo(() => {
      if (!q.trim()) return [];
      const lq = q.toLowerCase();
      return Object.entries(notes).filter(([id, n]) =>
        n.title.toLowerCase().includes(lq) || n.body.toLowerCase().includes(lq)
      ).slice(0, 20).map(([id, n]) => {
        const idx = n.body.toLowerCase().indexOf(lq);
        let ctx = idx >= 0 ? n.body.slice(Math.max(0, idx - 24), idx + 40).replace(/\n/g, " ") : "";
        return { id, title: n.title, ctx };
      });
    }, [q]);
    return React.createElement("div", { style: { padding: "var(--space-2)" } },
      React.createElement("div", { className: "global-search", style: { width: "100%", marginBottom: "var(--space-2)" } },
        React.createElement(Icon, { name: "search", size: 14 }),
        React.createElement("input", {
          autoFocus: true, value: q, onChange: (e) => setQ(e.target.value),
          placeholder: t.search_vault,
          style: { flex: 1, border: "none", background: "transparent", outline: "none", color: "var(--color-text)", fontSize: "var(--text-sm)", fontFamily: "var(--font-ui)" },
        }),
      ),
      q.trim() && results.length === 0
        ? React.createElement("div", { className: "cmd-empty" }, t.no_results)
        : results.map((r) =>
            React.createElement("div", { key: r.id, className: "bl-item", style: { marginBottom: 4 }, onClick: () => onOpen(r.id) },
              React.createElement("div", { className: "bl-title" }, r.title),
              r.ctx ? React.createElement("div", { className: "bl-ctx" }, "…" + r.ctx + "…") : null,
            )
          ),
      !q.trim() ? React.createElement("div", { className: "empty-state" },
        React.createElement(Icon, { name: "search", size: 26, style: { opacity: 0.4 } }),
        React.createElement("div", { className: "es-sub" }, "FTS мгновенно · семантика ≤300мс / FTS instant · semantic ≤300ms"),
      ) : null,
    );
  }

  function TagsPanel({ t, onTag }) {
    return React.createElement("div", null,
      React.createElement("div", { className: "side-head" }, t.tags),
      window.NEXUS_TAGS.map((tg) =>
        React.createElement("div", { key: tg.tag, className: "tag-row", onClick: () => onTag(tg.tag) },
          React.createElement(Icon, { name: "hash", size: 14, className: "ico" }),
          React.createElement("span", null, tg.tag),
          React.createElement("span", { className: "count" }, tg.count),
        )
      )
    );
  }

  function Sidebar({ t, activeId, onOpen, onTag, view, onHome, onNewNote }) {
    const [tab, setTab] = useState("files");
    return React.createElement("aside", { className: "sidebar" },
      React.createElement("div", { className: "side-rail" },
        [["files","file-text"],["search","search"],["tags","hash"],["starred","star"]].map(([id, ico]) =>
          React.createElement("button", {
            key: id, className: "rail-btn" + (tab === id ? " active" : ""),
            onClick: () => setTab(id), title: t[id] || id, "aria-label": t[id] || id,
          }, React.createElement(Icon, { name: ico, size: 17 }))
        )
      ),
      React.createElement("div", { className: "side-scroll" },
        React.createElement("div", { className: "side-nav" },
          React.createElement("div", { className: "nav-item" + (view === "home" ? " active" : ""), onClick: onHome },
            React.createElement(Icon, { name: "home", size: 15, className: "ico" }),
            React.createElement("span", null, "Home")),
          React.createElement("div", { className: "nav-item", onClick: onNewNote },
            React.createElement(Icon, { name: "plus", size: 15, className: "ico" }),
            React.createElement("span", null, t === window.NEXUS_I18N.ru ? "Новая заметка" : "New note"))),
        tab === "files" && React.createElement("div", null,
          React.createElement("div", { className: "side-head" }, t.explorer,
            React.createElement(Icon, { name: "plus", size: 14, style: { cursor: "pointer" } })),
          React.createElement(FileTree, { t, activeId, onOpen })
        ),
        tab === "search" && React.createElement(SearchPanel, { t, onOpen }),
        tab === "tags" && React.createElement(TagsPanel, { t, onTag }),
        tab === "starred" && React.createElement("div", null,
          React.createElement("div", { className: "side-head" }, t.starred),
          ["nexus","second-brain"].map((id) =>
            React.createElement("div", { key: id, className: "tree-row" + (id === activeId ? " active" : ""), onClick: () => onOpen(id) },
              React.createElement("span", { className: "twist" }),
              React.createElement(Icon, { name: "file-text", size: 15, className: "ico-file" }),
              React.createElement("span", { className: "name" }, window.NEXUS_NOTES[id].title),
              React.createElement("span", { className: "star" }, "★"),
            )
          )
        ),
      )
    );
  }
  window.Sidebar = Sidebar;
})();
