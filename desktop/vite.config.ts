import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
    proxy: { '/api': `http://127.0.0.1:${process.env.ARISTIDE_API_PORT ?? '9669'}` },
  },
  build: { target: ['es2022', 'safari15'] },
});
