import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { resolve } from 'path';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
      '@lib': resolve(__dirname, 'src/lib'),
    },
    extensions: ['.tsx', '.ts', '.mjs', '.js', '.jsx', '.json'],
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: true,
    target: 'es2022',
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
});
