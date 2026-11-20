import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vitest/config'

// The service the development server sends API calls to (`dalsegno serve`).
const API_TARGET = process.env.DALSEGNO_API ?? 'http://127.0.0.1:8737'

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: { '/api': API_TARGET },
  },
  test: {
    include: ['src/**/*.test.ts'],
  },
})
