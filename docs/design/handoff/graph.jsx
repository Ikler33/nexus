// graph.jsx — force-directed link graph over real [[wikilinks]].
// Local N-hop (default) + Global, tag filters, drag/hover/click-to-open.
(function () {
  const { useState, useRef, useEffect, useMemo, useCallback } = React;
  const Icon = window.Icon;

  // tag → token color for node accents
  const TAG_COLOR = {
    ai: "var(--color-ai)", rag: "var(--color-ai)", paper: "var(--color-link)",
    project: "var(--color-accent)", "local-first": "var(--color-accent)",
    daily: "var(--color-tag)", method: "var(--color-tag)", pkm: "var(--color-tag)",
    philosophy: "var(--color-link)", dev: "var(--color-warning)", rust: "var(--color-warning)",
  };
  function nodeColor(tags) { for (const tg of tags || []) if (TAG_COLOR[tg]) return TAG_COLOR[tg]; return "var(--color-text-muted)"; }

  // build undirected graph from wikilinks
  function buildGraph() {
    const NOTES = window.NEXUS_NOTES, T2I = window.NEXUS_TITLE2ID;
    const re = /\[\[([^\]]+)\]\]/g;
    const nodes = {}, edgeSet = new Set(), edges = [];
    Object.entries(NOTES).forEach(([id, n]) => { nodes[id] = { id, title: n.title, tags: n.tags || [], deg: 0 }; });
    Object.entries(NOTES).forEach(([id, n]) => {
      let m; re.lastIndex = 0;
      while ((m = re.exec(n.body))) {
        const tgt = T2I[m[1].toLowerCase()];
        if (tgt && tgt !== id) {
          const key = [id, tgt].sort().join("|");
          if (!edgeSet.has(key)) { edgeSet.add(key); edges.push({ a: id, b: tgt }); nodes[id].deg++; nodes[tgt].deg++; }
        }
      }
    });
    const adj = {}; Object.keys(nodes).forEach((k) => (adj[k] = []));
    edges.forEach((e) => { adj[e.a].push(e.b); adj[e.b].push(e.a); });
    return { nodes, edges, adj };
  }

  function bfs(adj, start, depth) {
    const seen = { [start]: 0 }, q = [start];
    while (q.length) { const cur = q.shift(); if (seen[cur] >= depth) continue;
      (adj[cur] || []).forEach((nb) => { if (!(nb in seen)) { seen[nb] = seen[cur] + 1; q.push(nb); } }); }
    return seen;
  }

  function GraphView({ t, activeId, onOpen, onClose }) {
    const G = useMemo(buildGraph, []);
    const [mode, setMode] = useState("local");
    const [depth, setDepth] = useState(2);
    const [tagFilter, setTagFilter] = useState(null);
    const [loading, setLoading] = useState(true);
    const [hover, setHover] = useState(null);
    const [dragId, setDragId] = useState(null);
    const [, force] = useState(0);
    const posRef = useRef({});
    const dragRef = useRef(null);
    const rafRef = useRef(null);
    const alphaRef = useRef(0);
    const visRef = useRef(null);
    const runningRef = useRef(false);
    const movedRef = useRef(false);
    const W = 900, H = 620;

    // visible subgraph
    const vis = useMemo(() => {
      let ids;
      if (mode === "local" && activeId && G.nodes[activeId]) {
        const seen = bfs(G.adj, activeId, depth); ids = new Set(Object.keys(seen));
      } else ids = new Set(Object.keys(G.nodes));
      if (tagFilter) {
        ids = new Set([...ids].filter((id) => (G.nodes[id].tags || []).includes(tagFilter) || id === activeId));
      }
      const nodes = [...ids].map((id) => G.nodes[id]);
      const edges = G.edges.filter((e) => ids.has(e.a) && ids.has(e.b));
      return { nodes, edges, idset: ids };
    }, [mode, depth, tagFilter, activeId, G]);

    const neighborSet = useMemo(() => {
      const focus = dragId || hover;
      if (!focus) return null; const s = new Set([focus]);
      vis.edges.forEach((e) => { if (e.a === focus) s.add(e.b); if (e.b === focus) s.add(e.a); }); return s;
    }, [hover, dragId, vis]);

    // kin of the currently-open note (active node) — highlighted on open
    const activeKin = useMemo(() => {
      const s = new Set();
      if (!activeId) return s;
      vis.edges.forEach((e) => { if (e.a === activeId) s.add(e.b); if (e.b === activeId) s.add(e.a); });
      return s;
    }, [activeId, vis]);

    // keep latest visible set reachable from the persistent sim loop
    visRef.current = vis;

    // ── persistent, re-heatable force simulation ──
    function step() {
      const V = visRef.current; if (!V) return;
      const P = posRef.current, cx = W / 2, cy = H / 2;
      // alpha floor while dragging so springs keep pulling neighbours
      const a = dragRef.current ? Math.max(alphaRef.current, 0.45) : alphaRef.current;
      const ids = V.nodes.map((n) => n.id);
      // repulsion
      for (let i = 0; i < ids.length; i++) for (let j = i + 1; j < ids.length; j++) {
        const A = P[ids[i]], B = P[ids[j]];
        let dx = A.x - B.x, dy = A.y - B.y, d2 = dx * dx + dy * dy || 0.01;
        const f = (6800 / d2) * a; const d = Math.sqrt(d2);
        A.vx += (dx / d) * f; A.vy += (dy / d) * f; B.vx -= (dx / d) * f; B.vy -= (dy / d) * f;
      }
      // springs (edges pull connected nodes toward a rest length)
      V.edges.forEach((e) => {
        const A = P[e.a], B = P[e.b]; if (!A || !B) return;
        let dx = B.x - A.x, dy = B.y - A.y; const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
        const f = (d - 96) * 0.05 * a;
        A.vx += (dx / d) * f; A.vy += (dy / d) * f; B.vx -= (dx / d) * f; B.vy -= (dy / d) * f;
      });
      // gravity + integrate (the dragged node is pinned to the cursor)
      ids.forEach((id) => {
        const N = P[id]; if (!N) return;
        if (dragRef.current === id) { N.vx = 0; N.vy = 0; return; }
        N.vx += (cx - N.x) * 0.012 * a; N.vy += (cy - N.y) * 0.012 * a;
        N.vx *= 0.85; N.vy *= 0.85; N.x += N.vx; N.y += N.vy;
        N.x = Math.max(40, Math.min(W - 40, N.x)); N.y = Math.max(36, Math.min(H - 36, N.y));
      });
      alphaRef.current *= 0.94;
      force((v) => v + 1);
    }
    function loop() {
      step();
      // keep running while warm OR while dragging
      if (alphaRef.current > 0.04 || dragRef.current) { rafRef.current = requestAnimationFrame(loop); }
      else { runningRef.current = false; }
    }
    function kick(a) {
      alphaRef.current = Math.max(alphaRef.current, a);
      if (!runningRef.current) { runningRef.current = true; rafRef.current = requestAnimationFrame(loop); }
    }
    // expose to drag handlers
    const kickRef = useRef(kick); kickRef.current = kick;

    // (re)seed positions + heat the sim whenever the visible set changes
    useEffect(() => {
      const P = posRef.current; const cx = W / 2, cy = H / 2;
      vis.nodes.forEach((n, i) => {
        if (!P[n.id]) {
          const ang = (i / vis.nodes.length) * Math.PI * 2;
          P[n.id] = { x: cx + Math.cos(ang) * 160 + (Math.random() - 0.5) * 30, y: cy + Math.sin(ang) * 160 + (Math.random() - 0.5) * 30, vx: 0, vy: 0 };
        }
      });
      setLoading(true);
      kick(1);
      const loadTimer = setTimeout(() => setLoading(false), 900);
      return () => { clearTimeout(loadTimer); };
    }, [vis]);

    // cleanup on unmount
    useEffect(() => () => { cancelAnimationFrame(rafRef.current); runningRef.current = false; }, []);

    // drag — pin node to cursor, neighbours follow via springs
    const svgRef = useRef(null);
    const toLocal = (e) => { const r = svgRef.current.getBoundingClientRect(); const cx = e.touches ? e.touches[0].clientX : e.clientX, cy = e.touches ? e.touches[0].clientY : e.clientY; return { x: (cx - r.left) / r.width * W, y: (cy - r.top) / r.height * H }; };
    const onDown = useCallback((id) => (e) => {
      e.preventDefault(); dragRef.current = id; setDragId(id); movedRef.current = false;
      const N0 = posRef.current[id];
      const start = toLocal(e); const off = N0 ? { x: N0.x - start.x, y: N0.y - start.y } : { x: 0, y: 0 };
      kickRef.current(0.7);
      const move = (ev) => {
        movedRef.current = true;
        const p = toLocal(ev); const N = posRef.current[id];
        if (N) { N.x = Math.max(40, Math.min(W - 40, p.x + off.x)); N.y = Math.max(36, Math.min(H - 36, p.y + off.y)); N.vx = N.vy = 0; }
        kickRef.current(0.5); // keep the sim warm so neighbours chase
      };
      const up = () => {
        dragRef.current = null; setDragId(null); kickRef.current(0.35);
        window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up);
        window.removeEventListener("touchmove", move); window.removeEventListener("touchend", up);
      };
      window.addEventListener("mousemove", move); window.addEventListener("mouseup", up);
      window.addEventListener("touchmove", move, { passive: false }); window.addEventListener("touchend", up);
    }, []);

    const allTags = window.NEXUS_TAGS.slice(0, 8);
    const P = posRef.current;

    return React.createElement("div", { className: "graph-view" },
      // toolbar
      React.createElement("div", { className: "graph-bar" },
        React.createElement("div", { className: "seg" },
          React.createElement("button", { className: "seg-btn" + (mode === "local" ? " on" : ""), onClick: () => setMode("local") }, t.graph_local),
          React.createElement("button", { className: "seg-btn" + (mode === "global" ? " on" : ""), onClick: () => setMode("global") }, t.graph_global)),
        mode === "local" ? React.createElement("label", { className: "graph-depth" },
          t.graph_depth,
          React.createElement("input", { type: "range", min: 1, max: 3, value: depth, onChange: (e) => setDepth(+e.target.value) }),
          React.createElement("span", { className: "mono" }, depth)) : null,
        React.createElement("div", { className: "graph-tags" },
          allTags.map((tg) => React.createElement("button", {
            key: tg.tag, className: "gt-chip" + (tagFilter === tg.tag ? " on" : ""),
            onClick: () => setTagFilter((f) => f === tg.tag ? null : tg.tag),
          }, "#" + tg.tag))),
        React.createElement("div", { className: "graph-spacer" }),
        React.createElement("span", { className: "graph-stat mono" }, t.graph_stat(vis.nodes.length, vis.edges.length)),
        React.createElement("button", { className: "tb-btn", onClick: onClose, title: t.close }, React.createElement(Icon, { name: "x", size: 16 }))),
      mode === "global" ? React.createElement("div", { className: "graph-warn" },
        React.createElement(Icon, { name: "alert", size: 14 }), t.graph_global_warn) : null,
      // canvas
      React.createElement("div", { className: "graph-stage" },
        loading ? React.createElement("div", { className: "graph-loading" },
          React.createElement(window.BrandThinking, { size: 26 }),
          React.createElement("span", { className: "mt-label" }, t.graph_loading)) : null,
        React.createElement("svg", { ref: svgRef, className: "graph-svg", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "xMidYMid meet" },
          (() => { let flowN = 0; return vis.edges.map((e, i) => {
            const A = P[e.a], B = P[e.b]; if (!A || !B) return null;
            const active = neighborSet ? (neighborSet.has(e.a) && neighborSet.has(e.b)) : true;
            const lit = dragId && (e.a === dragId || e.b === dragId);
            const flow = !dragId && activeId && (e.a === activeId || e.b === activeId);
            const fcls = flow ? " flow f" + ((flowN++ % 4) + 1) : "";
            return React.createElement("line", { key: i, x1: A.x, y1: A.y, x2: B.x, y2: B.y,
              className: "g-edge" + (active ? "" : " dim") + (lit ? " lit" : "") + fcls });
          }); })(),
          vis.nodes.map((n) => {
            const N = P[n.id]; if (!N) return null;
            const isActive = n.id === activeId;
            const r = Math.max(6, Math.min(15, 6 + n.deg * 1.7));
            const faded = neighborSet && !neighborSet.has(n.id);
            const isKin = !isActive && activeKin.has(n.id);
            return React.createElement("g", {
              key: n.id, className: "g-node" + (faded ? " faded" : "") + (isActive ? " active" : "") + (isKin ? " kin" : "") + (dragId === n.id ? " grabbing" : ""),
              transform: `translate(${N.x},${N.y})`,
              onMouseEnter: () => setHover(n.id), onMouseLeave: () => setHover(null),
              onMouseDown: onDown(n.id), onTouchStart: onDown(n.id),
              onClick: () => { if (movedRef.current) { movedRef.current = false; return; } onOpen(n.id); },
            },
              isActive ? React.createElement("circle", { r: r + 6, className: "g-pulse" }) : null,
              isActive ? React.createElement("circle", { r: r + 6, className: "g-ripple" }) : null,
              React.createElement("circle", { r, className: "g-dot", style: { fill: isActive ? "var(--color-accent)" : nodeColor(n.tags) } }),
              isActive ? React.createElement("circle", { r: r + 5, className: "g-ring" }) : null,
              isKin ? React.createElement("circle", { r: r + 3.5, className: "g-kinring" }) : null,
              React.createElement("text", { y: r + 14, className: "g-label", textAnchor: "middle" }, n.title));
          })),
      ),
    );
  }
  window.GraphView = GraphView;
})();
