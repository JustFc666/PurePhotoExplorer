/**
 * 人物（人脸归纳）IPC 封装。
 *
 * ⚠️ 字段名必须与 Rust 侧 `commands/people.rs` 的 DTO 一致（`camelCase`）。
 *
 * ## 数据来源
 * 人脸与人物**不是前端算的**：`tools/ai/face_probe.py` 写 `faces`（检测+128 维 int8 向量），
 * `tools/ai/face_cluster.py` 跑两阶段聚类写 `persons`。这里只负责读出来与纠正
 * （改名 / 合并 / 拆分 / 删除）。以后 Rust 侧接手推理时，命令不用改。
 */

import { invoke } from '@tauri-apps/api/core'

/** 一张人脸的几何信息（归一化坐标，相对检测时那一档缩略图）。 */
export interface FaceBox {
  id: number
  imageId: number
  x: number
  y: number
  w: number
  h: number
  /** 脸宽像素（质量指标，太小说明这张脸不值得当头像） */
  quality: number | null
}

/** 侧栏「人物」条目。 */
export interface PersonItem {
  id: number
  /** `null` = 未命名，界面显示「未命名 ①」 */
  name: string | null
  faceCount: number
  cover: FaceBox | null
}

/** 人脸库概览（设置面板）。 */
export interface FacesStats {
  faces: number
  assigned: number
  persons: number
  namedPersons: number
  /** 出现过的模型版本；多于一条说明换过模型、需要重建 */
  models: string[]
}

/** 人物列表（默认只含 ≥3 张脸的）。 */
export async function managerPeople(minFaces = 3, libraryId?: number): Promise<PersonItem[]> {
  /* `libraryId` 有值 = 只算"至少有一张脸在这个图库里"的人（2026-10-25） */
  return invoke<PersonItem[]>('manager_people', { minFaces, libraryId: libraryId ?? null })
}

/** 某个人物下的所有脸。 */
export async function managerPersonFaces(personId: number): Promise<FaceBox[]> {
  return invoke<FaceBox[]>('manager_person_faces', { personId })
}

/** 改名；传 `null` 或空串 = 取消命名。 */
export async function managerRenamePerson(personId: number, name: string | null): Promise<void> {
  return invoke<void>('manager_rename_person', { personId, name })
}

/** 把人脸从若干人物合并到 `into`，返回被并过去的脸数。 */
export async function managerMergePersons(from: number[], into: number): Promise<number> {
  return invoke<number>('manager_merge_persons', { from, into })
}

/** 把选中的脸从人物里拆出来，建成新的未命名人物；返回新人物 id。 */
export async function managerSplitFaces(
  personId: number,
  faceIds: number[],
): Promise<number | null> {
  return invoke<number | null>('manager_split_faces', { personId, faceIds })
}

/** 删除人物（只解绑人脸，不删人脸）。 */
export async function managerDeletePerson(personId: number): Promise<void> {
  return invoke<void>('manager_delete_person', { personId })
}

/** 人脸库概览。 */
export async function managerFacesStats(): Promise<FacesStats> {
  return invoke<FacesStats>('manager_faces_stats')
}

/** 清除全部人脸数据（隐私开关）；返回删掉的人脸行数。 */
export async function managerFacesClear(): Promise<number> {
  return invoke<number>('manager_faces_clear')
}
