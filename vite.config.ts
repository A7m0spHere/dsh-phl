import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'
import { fileURLToPath, URL } from 'node:url'

const host = process.env.TAURI_DEV_HOST

// The About panel must name the version that was actually packaged. Reading it
// from package.json at build time keeps it in step with the installer and the
// crate without a runtime call (and without the app-version permission); the
// gate checks that all four sources agree before anything ships.
const { version } = JSON.parse(
  readFileSync(fileURLToPath(new URL('./package.json', import.meta.url)), 'utf8'),
) as { version: string }

// Vite is driven by the Tauri CLI in desktop mode, so the dev server must be
// predictable: a fixed port the window points at, and no screen clearing that
// would wipe Rust's compiler output.
export default defineConfig({
  plugins: [react()],
  define: {
    __PHL_VERSION__: JSON.stringify(version),
  },
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
