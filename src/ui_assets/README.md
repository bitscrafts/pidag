# ui_assets/

Embedded frontend assets for the pidag trace UI.

## Overview

This directory contains the single-page application (SPA) that powers the
`pidag ui` web interface. Assets are embedded into the binary at compile time
via `include_str!()`, eliminating external file dependencies.

## Files

| File | Description |
|------|-------------|
| `index.html` | Complete SPA (~58KB) — HTML + CSS + JavaScript |

---

## Architecture

```
pidag ui --port 4600
    │
    └── axum web server
            │
            GET / ──► include_str!("ui_assets/index.html")
            │
            └── Vanilla JS SPA (no build toolchain)
```

### Design Decisions

1. **Single file**: All HTML, CSS, and JavaScript in one `index.html`
2. **No build step**: Vanilla JS, no Node/Bun/Webpack required
3. **Embedded binary**: `include_str!()` bakes assets into the executable
4. **Zero external deps**: No CDN links, no external CSS frameworks

### Embedding Pattern

```rust
// In ui/mod.rs
const INDEX_HTML: &str = include_str!("../ui_assets/index.html");

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}
```

---

## SPA Features

### Views (Hash Router)

| Route | View |
|-------|------|
| `#/` | Runs list (single-project) or Project cards (workspace) |
| `#/run/:id` | Run detail with timeline |
| `#/project` | Project overview |
| `#/project/spec/:name` | Spec detail |

### Workspace Mode Routes

| Route | View |
|-------|------|
| `#/` | Project cards grid |
| `#/project/:name` | Project overview |
| `#/project/:name/spec/:spec` | Spec detail |
| `#/project/:name/run/:id` | Run detail |

### Components

- **Run list**: Table of runs with status, timestamps
- **Run detail**: Node states, timeline visualization
- **Timeline**: vis-timeline Gantt chart of node execution
- **Project overview**: Spec cards, run history
- **Spec detail**: Parsed spec with exit criteria

---

## Development

To modify the UI:

1. Edit `index.html` directly
2. Rebuild pidag: `cargo build -p pidag`
3. Test: `pidag ui --port 4600`

No hot-reload — the binary must be rebuilt for changes to take effect.

### Adding External Libraries

If a library is needed (e.g., vis-timeline):
- Include it inline in the HTML
- Or use a CDN link (breaks offline capability)

Current inline dependencies:
- vis-timeline (for Gantt charts)

---

## See Also

- [ui/README.md](../ui/README.md) — Backend handlers and routes
- [ui/render.rs](../ui/render.rs) — Server-side rendering (mermaid, status)
