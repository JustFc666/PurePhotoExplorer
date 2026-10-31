/**
 * 「默认打开方式」的文件关联状态与动作（设置浮层用）。
 *
 * 为什么不放在 `App.tsx` 里：这段是**完全自洽的一小块** —— 三个 state、
 * 一个「打开设置时才查」的 effect、两个写注册表的动作，读的只有
 * `ASSOCIATION_EXTS` / `ASSOCIATION_FORMATS` 两个常量。留在组合根里，
 * 它夹在放映、打印、调整面板中间，而组合根本来该只负责装配。
 *
 * ## 两条语义
 * 1. ⛔ **只在打开设置时才查**（不是启动时）：注册表读取虽快，但这是低频设置，
 *    没必要占用启动路径；而且用户可能在外部改过关联，每次打开都重读更准。
 * 2. ⛔ **改完立刻重读，不乐观更新**：复选框反映注册表的**真实**状态 ——
 *    万一写入失败（权限、被杀软拦），乐观翻转会显示成"已关联"。
 */

import { useCallback, useEffect, useState } from 'react'
import {
  decodeError,
  getAssociationState,
  setFileAssociation,
  setFileAssociationBulk,
  type AssociationState,
} from '../ipc'
import { ASSOCIATION_EXTS, ASSOCIATION_FORMATS } from '../constants/formats'

export type FileAssociationApi = {
  /** 注册表里的真实状态；`null` = 读不出来（或还没读过） */
  associations: AssociationState | null
  busy: boolean
  error: string | null
  /** 重新读一遍注册表（设置浮层每次打开都会自动调） */
  refresh: () => Promise<void>
  /** 勾选 / 取消单个格式 */
  toggleOne: (ext: string, label: string, enable: boolean) => Promise<void>
  /** 全选 / 全不选（只写需要变更的那些） */
  toggleAll: (enable: boolean) => Promise<void>
}

/**
 * @param isOpen 设置浮层是否打开 —— 打开时自动重读一次注册表
 */
export function useFileAssociations(isOpen: boolean): FileAssociationApi {
  const [associations, setAssociations] = useState<AssociationState | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      setAssociations(await getAssociationState(ASSOCIATION_EXTS))
      setError(null)
    } catch (err) {
      setAssociations(null)
      setError(decodeError(err, '读取文件关联失败').message)
    }
  }, [])

  useEffect(() => {
    if (isOpen) void refresh()
  }, [isOpen, refresh])

  /**
   * 「全选 / 全不选」所有可关联格式。
   *
   * **只处理需要变更的那些**（拿 `registered` 求差集）—— 幂等重复写注册表
   * 没有收益，还平添失败面。全部已一致时直接返回，不惊动后端。
   *
   * 一次性提交给后端的批量命令：它走静默写入、末尾只广播**一次**
   * `SHCNE_ASSOCCHANGED`（逐个调用单扩展名命令会广播 N 次）。
   */
  const toggleAll = useCallback(
    async (enable: boolean) => {
      const registered = new Set(associations?.registered ?? [])
      const pending = ASSOCIATION_FORMATS.filter(([ext]) => registered.has(ext) !== enable)
      if (pending.length === 0) return

      setBusy(true)
      setError(null)
      try {
        const failed = await setFileAssociationBulk(
          pending.map(([ext, label]) => ({ ext, displayName: `${label} 图片` })),
          enable,
        )
        // 同样以注册表的真实状态为准回显
        await refresh()
        if (failed.length > 0) {
          setError(`以下格式设置失败：${failed.map((e) => `.${e}`).join('、')}`)
        }
      } catch (err) {
        setError(decodeError(err, '设置文件关联失败').message)
      } finally {
        setBusy(false)
      }
    },
    [associations, refresh],
  )

  /**
   * 勾选 / 取消「打开方式」关联。
   *
   * 写的是 `HKCU`（不需要管理员），且**只是加入「打开方式」列表**，
   * 不是设为默认 —— Windows 不允许程序静默改默认程序，详见 Rust 侧 `win_assoc`。
   */
  const toggleOne = useCallback(
    async (ext: string, label: string, enable: boolean) => {
      setBusy(true)
      setError(null)
      try {
        await setFileAssociation(ext, `${label} 图片`, enable)
        await refresh()
      } catch (err) {
        setError(decodeError(err, '设置文件关联失败').message)
      } finally {
        setBusy(false)
      }
    },
    [refresh],
  )

  return { associations, busy, error, refresh, toggleOne, toggleAll }
}
