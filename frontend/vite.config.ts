import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react-swc';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: 'prompt',
      manifest: {
        name: 'kino',
        short_name: 'kino',
        theme_color: '#111113',
        background_color: '#111113',
        display: 'standalone',
        icons: [
          { src: '/kino-app-icon-192.png', sizes: '192x192', type: 'image/png' },
          { src: '/kino-app-icon-512.png', sizes: '512x512', type: 'image/png' },
          { src: '/kino-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
      workbox: {
        runtimeCaching: [
          {
            urlPattern: /\/api\/v1\/images\/.*/,
            handler: 'CacheFirst',
            options: { cacheName: 'images', expiration: { maxEntries: 500 } },
          },
          {
            urlPattern: /\/api\/v1\/tmdb\/.*/,
            handler: 'StaleWhileRevalidate',
            options: { cacheName: 'tmdb', expiration: { maxAgeSeconds: 3600 } },
          },
        ],
      },
    }),
  ],
  build: {
    sourcemap: true,
  },
  server: {
    host: '0.0.0.0',
    port: 5173,
    watch: { usePolling: true },
    hmr: {
      clientPort: 5173,
    },
    proxy: {
      '/api': {
        target: 'http://localhost:8080',
        changeOrigin: true,
        ws: true,
      },
    },
    // Forward browser console logs to the dev-server terminal so we
    // can tail them via `just logs-frontend` for offline analysis.
    // Opted-in levels only — noisy `log` / `info` would drown out HMR
    // output; `debug` is what our structured traces (e.g. `[notif]`
    // decision lines in src/state/websocket.ts) use.
    forwardConsole: {
      logLevels: ['debug', 'warn', 'error'],
      unhandledErrors: true,
    },
  },
  resolve: {
    alias: { '@': '/src' },
  },
});
