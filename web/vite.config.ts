import { fileURLToPath, URL } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

/**
 * Bundler entry for the Ourealis SPA.
 *
 * `/api` is proxied to the local service so the dev server addresses the same
 * relative URLs as the build embedded in the service binary. `echarts` and
 * `@babylonjs/core` are both large and neither is needed by the app shell, so
 * each gets its own chunk; `zrender` rides with `echarts` because only the
 * chart bundle can pull it in.
 */
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  define: {
    // The app uses the composition API only and no intlify devtools, so the
    // legacy build and the full install are compiled out of vue-i18n.
    __VUE_I18N_FULL_INSTALL__: 'false',
    __VUE_I18N_LEGACY_API__: 'false',
    __INTLIFY_PROD_DEVTOOLS__: 'false',
  },
  server: {
    proxy: {
      '/api': {
        // WebSocket upgrades must be proxied too: the job socket shares the API
        // prefix, and without this the handshake hangs instead of failing.
        ws: true,
        target: 'http://127.0.0.1:8080',
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
    rollupOptions: {
      output: {
        manualChunks: (id) => {
          if (id.includes('node_modules/echarts') || id.includes('node_modules/zrender')) {
            return 'echarts'
          }
          if (id.includes('node_modules/@babylonjs')) {
            return 'babylon'
          }
          return undefined
        },
      },
    },
  },
})
