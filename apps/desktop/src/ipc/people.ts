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

/** 一张没归入任何人物的脸（人像墙「未归类」区）。 */
export interface OrphanFace extends FaceBox {
  /** 原图绝对路径；卷没挂上时为 `null` */
  path: string | null
}

/**
 * 未归类的人脸，**一张照片只留最大那张脸**。
 *
 * 用户 2026-11-02 要求「图库里每张有脸的照片都要能在人像里找到」：
 * 人物只覆盖聚成簇的（约 70%），剩下这些按照片补在墙上。
 */
export async function managerOrphanFaces(o?: {
  limit?: number
  libraryId?: number
}): Promise<OrphanFace[]> {
  return invoke<OrphanFace[]>('manager_orphan_faces', {
    limit: o?.limit ?? null,
    libraryId: o?.libraryId ?? null,
  })
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

/**
 * 把这个人的头像换成「`imageId` 这张图里该人的脸」（同图多张脸取面积最大的）。
 * 返回新封面；`null` = 这张图里没有这个人的脸。
 */
export async function managerSetPersonCover(
  personId: number,
  imageId: number,
): Promise<FaceBox | null> {
  return invoke<FaceBox | null>('manager_set_person_cover', { personId, imageId })
}

/** 把人脸从若干人物合并到 `into`，返回被并过去的脸数。 */export async function managerMergePersons(from: number[], into: number): Promise<number> {
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

/**
 * 清除人脸数据；返回删掉的人脸行数。
 *
 * @param library 传图库名 = 只清这个图库；不传 = 清全部（连人物一起）
 *
 * ⚠️ 按图库清时**只清理变空的人物**，其他图库的人物原封不动。
 */
export async function managerFacesClear(library?: string): Promise<number> {
  return invoke<number>('manager_faces_clear', { library: library ?? null })
}
