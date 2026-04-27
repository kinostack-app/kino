---
paths:
  - "**"
---
# Command Reference

## Backend (/workspace/backend)

```bash
# Quality & Linting
just fix-ci          # Auto-fix then verify (preferred for agentic workflow)
just ci              # Quality gates (fmt + clippy) — verify only
just fix             # Auto-fix only
just lint            # Clippy lint (includes type checking)
just typecheck       # Fast type check without clippy

# Testing
just test            # Full test suite
just test-filter 'EXPR'  # Filter by nextest expression

# Container management (docker CLI available in dev shell)
just logs            # Backend logs (follow)
just logs-tail 50    # Last N lines
just logs-frontend   # Frontend logs
just restart         # Restart backend container
just restart-frontend # Restart frontend (picks up vite.config changes)
just restart-all     # Restart all service containers
just status          # Container status + health check

# Reset
just reset           # Delete DB + librqbit session, restart backend fresh

# Database
just migrate         # Run pending migrations

# Build
just build           # Debug build
just build-release   # Release build

# OpenAPI + frontend SDK codegen — the canonical one-shot.
# ALWAYS use this after backend schema changes. Never run the
# frontend's `npm run codegen` alone — it only reads
# `backend/openapi.json`, it can't regenerate it, so you'll get
# stale types without noticing.
just codegen         # Export openapi.json → regen TS SDK → typecheck frontend
```

## Frontend (/workspace/frontend)

```bash
# Quality
npm run lint         # Biome check
npm run lint:fix     # Biome auto-fix
npm run typecheck    # TypeScript type check
npm run test         # Vitest tests

# Dev server
npm run dev          # Vite dev server (port 5173)

# Build
npm run build        # Production build

# API codegen — do NOT call this in isolation. Use
# `cd ../backend && just codegen` instead; that regenerates
# `backend/openapi.json` first, which this step consumes.
npm run codegen      # (Inner step of `just codegen`; reads openapi.json)
```

## Diagnostics

- `just fix-ci` (cargo clippy) is the **authoritative** compiler check
- If clippy passes, the code compiles — do not second-guess it
- After backend changes that affect the API, run `npm run codegen` in frontend
- `just status` shows container health + API status in one command
- `just logs` / `just logs-frontend` for debugging runtime issues
