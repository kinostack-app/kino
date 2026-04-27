import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/__tests__/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    exclude: ['src/api/generated/**'],
    coverage: {
      provider: 'v8',
      exclude: ['src/api/generated/**', 'src/components/ui/**', 'src/main.tsx'],
    },
  },
  resolve: {
    alias: { '@': '/src' },
  },
});
