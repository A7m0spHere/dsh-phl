import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { fileURLToPath, URL } from 'node:url'

const host = process.env.TAURI_DEV_HOST

// Vite is driven by the Tauri CLI in desktop mode, so the dev server must be
// predictable: a fixed port the window points at, and no screen clearing that
// would wipe Rust's compiler output.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    port: 5180,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 5181 } : undefined,
    watch: {
      // Rust artefacts churn constantly; watching them would thrash HMR.
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // WebView2 on Windows 10/11 and WKWebView both handle this comfortably.
    target: 'chrome110',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'esbuild',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
})
