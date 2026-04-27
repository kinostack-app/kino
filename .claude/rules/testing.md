---
paths:
  - "backend/**/tests/**"
  - "backend/**/*_test.rs"
  - "frontend/**/*.test.*"
  - "frontend/**/__tests__/**"
---
# Testing Rules

## Backend

- Use `cargo nextest` via `just test` or `just test-filter`
- Integration tests with real SQLite database (no mocks)
- Use `#[tokio::test]` for async tests
- sqlx test fixtures for database setup

## Frontend

- Vitest for unit/component tests
- `@testing-library/react` for component rendering
- Mock API calls at the fetch level, not the hook level
- Test files co-located with source: `MyComponent.test.tsx`
