import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";
import { execSync } from "child_process";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

function resolveBuildId(command: 'build' | 'serve'): string {
  if (command === 'serve') {
    return 'dev';
  }

  const envBuildId =
    process.env.REMOTE_CODE_BUILD_ID?.trim() ||
    process.env.GITHUB_SHA?.trim() ||
    process.env.VERCEL_GIT_COMMIT_SHA?.trim() ||
    process.env.CF_PAGES_COMMIT_SHA?.trim();

  if (envBuildId) {
    return envBuildId.slice(0, 12);
  }

  const runGit = (commandText: string): string | null => {
    try {
      return execSync(commandText, {
        cwd: __dirname,
        stdio: ['ignore', 'pipe', 'ignore'],
      })
        .toString()
        .trim();
    } catch {
      return null;
    }
  };

  const gitBuildId = runGit('git rev-parse --short=12 HEAD');
  if (gitBuildId) {
    const dirty = runGit('git status --porcelain -- .');
    if (dirty) {
      const timestamp = new Date().toISOString().replace(/[^0-9]/g, '').slice(0, 14);
      return `${gitBuildId}-${timestamp}`;
    }
    return gitBuildId;
  }

  try {
    return new Date().toISOString().replace(/[^0-9]/g, '').slice(0, 14);
  } catch {
    return 'local';
  }
}

// https://vite.dev/config/
export default defineConfig(async ({ command }) => ({
  plugins: [react()],
  define: {
    __APP_BUILD_ID__: JSON.stringify(resolveBuildId(command)),
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  optimizeDeps: {
    exclude: ['@tauri-apps/plugin-network'],
  },
  build: {
    chunkSizeWarningLimit: 700,
    rollupOptions: {
      output: {
        manualChunks: {
          react: ['react', 'react-dom', 'zustand'],
          ui: ['lucide-react', 'clsx', 'tailwind-merge'],
          'markdown-rendering': [
            'react-markdown',
            'remark-gfm',
            'remark-math',
            'rehype-katex',
            'rehype-highlight',
          ],
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    css: true,
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
