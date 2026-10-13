import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'

// Tauri 期望固定端口，开发服务器端口不能被占用导致 HMR 失效
const host = process.env.TAURI_DEV_HOST

/**
 * 版本号从 `package.json` 读，用 Vite `define` 注入成编译期常量。
 * 这样「帮助 → 关于」里显示的版本永远与打包版本一致，
 * 不需要在两处手工同步（`tauri.conf.json` 的 version 与其保持一致即可）。
 */
const pkg = JSON.parse(
  readFileSync(new URL('./package.json', import.meta.url), 'utf-8'),
) as { version: string }

export default defineConfig({
  plugins: [react()],

  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },

  // 防止 Vite 清空终端影响 Rust 编译输出
  clearScreen: false,

  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: {
      // Rust 侧文件由 cargo 处理，Vite 不需要监听
      ignored: ['**/src-tauri/**'],
    },
  },

  // 生产构建用 ES2021（WebView2 与 WKWebView 均支持）
  build: {
    target: 'es2021',
    minify: 'esbuild',
    // 保留 sourcemap：线上报错时能反查真实源码位置，
    // 否则用户只能给 minified 的 index-XXXX.js:40:81181 这种无意义坐标。
    // 代价是产物多约 1.5 MB（不进安装包的可执行主体）。
    sourcemap: true,
    chunkSizeWarningLimit: 800,
  },
})
