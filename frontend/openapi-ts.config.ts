import { defineConfig } from '@hey-api/openapi-ts';

export default defineConfig({
  client: '@hey-api/client-fetch',
  input: '../backend/openapi.json',
  output: {
    path: 'src/api/generated',
    lint: false,
    format: false,
  },
  plugins: [
    '@tanstack/react-query',
    'zod',
  ],
});
