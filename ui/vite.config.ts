import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
const target = process.env.ZEROSHOT_UI_TARGET ?? 'http://127.0.0.1:4173';
export default defineConfig(({ command }) => ({
  base: command === 'serve' ? '/ui/' : './',
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    proxy: {
      '/ui/api': {
        target,
        changeOrigin: true,
        configure(proxy) {
          proxy.on('proxyReq', (request, incoming) => {
            // Only the known local Vite origin is adapted. Foreign origins remain rejected.
            if (incoming.headers.origin === 'http://127.0.0.1:5173') {
              request.setHeader('origin', new URL(target).origin);
            }
          });
        },
      },
    },
  },
}));
