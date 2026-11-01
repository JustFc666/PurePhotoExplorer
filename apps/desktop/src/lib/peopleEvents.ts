/**
 * 「人物数据变了」的全局广播。
 *
 * ## 为什么要有它
 * 人像条（`PersonStrip`）和人像墙（`PersonWall`）各自持有一份 `persons` 列表，
 * 都不跟主区查询走。任何会改变 `persons` / `faces` 表的操作都必须广播一次，
 * 否则界面停在旧数字上。
 *
 * 2026-10-27 踩过：`removeAddedFolder` 删掉 rednote 后，后端已经把
 * `faces 541 → 200`、`persons 31 → 8` 改对了，但人像条还写着「更多图像 共31人」——
 * 因为删目录这条路径**没广播**，而另外两处（改名、换封面）各自写了一遍
 * `dispatchEvent('pv:person-renamed')`，谁也想不到还有第三个入口。
 *
 * ## ⛔ 新增操作时记得调
 * 只要它会动 `persons` 或 `faces`，就必须调一次 [`notifyPeopleChanged`]：
 * - 删照片（批量 / 单张）——脸会跟着图一起没
 * - 从图库移除目录
 * - 改名 / 换封面 / 合并 / 拆分 / 删除人物
 * - 重跑人脸聚类
 *
 * 事件名统一为 `pv:people-changed`：原来叫 `pv:person-renamed`，但换封面、
 * 删目录也叫「renamed」实在太误导，所以在统一入口时一并改了。
 */

export const PEOPLE_CHANGED_EVENT = 'pv:people-changed'

/** 广播「人物数据已变化」。不带 payload —— 监听方自己重拉，避免维护两套口径。 */
export function notifyPeopleChanged(): void {
  window.dispatchEvent(new Event(PEOPLE_CHANGED_EVENT))
}

/** 订阅。返回的函数可直接作为 `useEffect` 的清理逻辑。 */
export function onPeopleChanged(fn: () => void): () => void {
  window.addEventListener(PEOPLE_CHANGED_EVENT, fn)
  return () => window.removeEventListener(PEOPLE_CHANGED_EVENT, fn)
}

/*
 * ── 「库元数据变了」 ─────────────────────────────────────────────
 *
 * 和「人物变了」分开：人物只影响人像条 / 人像墙，而**标签、统计、筛选维度**
 * 影响的是主区一整套下拉和标签条。
 *
 * 加它的直接原因（2026-11-01）：在设置面板里跑完打标 / 人脸，主区既不刷新
 * 标签条也不刷新人像条 —— 照片列表没变，用户以为「识别没跑」。
 * 主区自己没有「任务结束」这个信号，只能靠广播。
 */

export const META_CHANGED_EVENT = 'pv:meta-changed'

/** 广播「库的元数据（标签 / 统计 / 人物）已变化」，主区据此重拉一次。 */
export function notifyMetaChanged(): void {
  window.dispatchEvent(new Event(META_CHANGED_EVENT))
}

/** 订阅元数据变更。 */
export function onMetaChanged(fn: () => void): () => void {
  window.addEventListener(META_CHANGED_EVENT, fn)
  return () => window.removeEventListener(META_CHANGED_EVENT, fn)
}
