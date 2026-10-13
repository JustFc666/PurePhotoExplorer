/**
 * 全局编译期常量声明。
 *
 * `__APP_VERSION__` 由 Vite 的 `define` 从 `package.json` 注入（见 `vite.config.ts`），
 * 打包时会被替换成字面量字符串。这样「帮助 → 关于」里显示的版本
 * 永远与打包版本一致，不需要在多处手工同步。
 */
declare const __APP_VERSION__: string
