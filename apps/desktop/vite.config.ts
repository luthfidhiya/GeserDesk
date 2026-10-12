import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, 'src') } },
  // Tauri needs a fixed port and must see the compiler's own messages.
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
})
