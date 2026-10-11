import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { VitePWA } from 'vite-plugin-pwa';

// The viewer is a static app (docs/SYNC.md §8): carnet-server serves the
// build at `/`, and its socket at /v1/sync. In development, Vite proxies
// the socket to a server running beside it (CARNET_SERVER, or :8787).
const server = process.env.CARNET_SERVER ?? 'http://127.0.0.1:8787';

export default defineConfig({
  plugins: [
    svelte(),
    VitePWA({
      registerType: 'autoUpdate',
      injectRegister: 'auto',
      includeAssets: ['icon.svg', 'icon-256.png', 'fonts/*.woff2'],
      manifest: {
        name: 'Carnet',
        short_name: 'Carnet',
        description: 'Your Carnet notes, in a browser.',
        start_url: '/',
        display: 'standalone',
        background_color: '#fcfaf7',
        theme_color: '#fcfaf7',
        icons: [
          { src: 'icon-256.png', sizes: '256x256', type: 'image/png' },
          { src: 'icon.svg', sizes: 'any', type: 'image/svg+xml' },
        ],
      },
      workbox: {
        globPatterns: ['**/*.{js,css,html,wasm,woff2,svg,png}'],
        maximumFileSizeToCacheInBytes: 8 * 1024 * 1024,
        // The app is one page at `/` (its screens are in the hash), so
        // that's the only address the worker answers from its cache. Any
        // other is the server's to answer: a hosted service's `/start`
        // and `/status`, the API. Answering them all with the app's page
        // made a signup link open the app again.
        navigateFallbackAllowlist: [/^\/(index\.html)?(\?|$)/],
        navigateFallbackDenylist: [/^\/v1\//],
      },
    }),
  ],
  server: {
    proxy: { '/v1': { target: server, ws: true } },
  },
  build: { target: 'es2022' },
});
