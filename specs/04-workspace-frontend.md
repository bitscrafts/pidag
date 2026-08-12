# Spec: Workspace Frontend

## Overview

**Project**: /projects/pidag
**Phase**: 4a - Workspace UI

Add workspace landing page and per-project navigation to the pidag UI frontend.

---

## Requirements

1. `#/` shows project cards grid from `GET /api/workspace`
2. `#/project/{name}` shows specs + runs for that project
3. Click card navigates to `#/project/{name}`

---

## Exit Criteria

- [ ] `#/` route shows project cards
- [ ] `#/project/{name}` shows specs
- [ ] Navigation between views works

---

## Guardrails

- Do NOT modify backend
- Do NOT break existing routes
