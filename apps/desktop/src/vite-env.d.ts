/// <reference types="vite/client" />

/** `?raw` 导入：把源文件内容当字符串拿过来（护栏测试用） */
declare module '*?raw' {
  const content: string
  export default content
}
