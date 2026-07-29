import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Relative base so the built SPA works when served from any path by the Go
// binary (single-binary embedding). In dev, /api is proxied to the running
// cross233 server for hot-reload + live data.
export default defineConfig({
  plugins: [react()],
  base: './',
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:7711',
      '/login': 'http://localhost:7711',
      '/logout': 'http://localhost:7711',
    },
  },
  build: {
    outDir: 'dist',
    // We clear dist ourselves in the Taskfile (shell rm) before building so the
    // embedded-asset copy stays clean; disabling Vite's own empty avoids a
    // bulk-delete guard in some sandboxed Node environments.
    emptyOutDir: false,
    target: 'es2022',
  },
})
