//! `volumes` 表仓储。

use crate::error::Result;
use crate::models::{Volume, VolumeState};
use crate::repo::parse_enum;
use rusqlite::{params, Connection, OptionalExtension};

/// 查询列（顺序与 [`map_row`] 一致）。
const COLS: &str =
    "id, volume_guid, serial, label, fs, removable, last_mount_path, last_seen_ts, state";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Volume> {
    let raw: String = row.get(8)?;
    Ok(Volume {
        id: row.get(0)?,
        volume_guid: row.get(1)?,
        serial: row.get(2)?,
        label: row.get(3)?,
        fs: row.get(4)?,
        removable: row.get::<_, i64>(5)? != 0,
        last_mount_path: row.get(6)?,
        last_seen_ts: row.get(7)?,
        state: parse_enum(8, raw.clone(), VolumeState::parse(&raw))?,
    })
}

/// 插入卷，返回自增 id。
pub fn insert(conn: &Connection, v: &Volume) -> Result<i64> {
    conn.execute(
        "INSERT INTO volumes
           (volume_guid, serial, label, fs, removable, last_mount_path, last_seen_ts, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            v.volume_guid,
            v.serial,
            v.label,
            v.fs,
            v.removable as i64,
            v.last_mount_path,
            v.last_seen_ts,
            v.state.as_str()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按卷 GUID 查（可移动盘插回时用）。
pub fn get_by_guid(conn: &Connection, guid: &str) -> Result<Option<Volume>> {
    let sql = format!("SELECT {COLS} FROM volumes WHERE volume_guid = ?1");
    Ok(conn.query_row(&sql, params![guid], map_row).optional()?)
}

/// 列出全部卷。
pub fn list(conn: &Connection) -> Result<Vec<Volume>> {
    let sql = format!("SELECT {COLS} FROM volumes ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 规范化卷 GUID：去掉首尾空白与**尾部的路径分隔符**，再转小写。
/// **只用于比较**，不写回数据库（写入仍是调用方给的原样字符串）。
///
/// ⚠️ 为什么需要它：`volume_guid` 是字符串标识，来源有两处 ——
/// ① 系统 API 直读，形如 `\\?\Volume{...}\`（尾部**一个**反斜杠）；
/// ② 人工按文档手填，极容易多写或少写一个反斜杠。
/// 精确比较时差一个字符就被当成**新卷**再插一条，随后同一批文件被索引两遍。
/// 2026-10-27 踩过：J 盘出现两条卷（guid 差一个 `\`），766 张图重复入库，
/// 「从图库移除」删掉一份另一份还在 —— 用户看到的就是「移除没生效」。
fn norm_guid(s: &str) -> String {
    s.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase()
}

/// 按 GUID **取或建**卷：已存在则更新可变字段（序列号/卷标/文件系统/最近盘符），返回 id。
///
/// 移动盘换盘符后重新挂载时，靠它把 `last_mount_path` 刷新成新盘符。
///
/// ⭐ 匹配分两级：先精确，再按 [`norm_guid`] 规范化匹配。
/// 命中规范化那一级时会把库里的 `volume_guid` **改写成调用方给的规范形式**，
/// 这样下次就精确命中，不会每次都走兜底。
pub fn upsert(conn: &Connection, v: &Volume) -> Result<i64> {
    let mut found = get_by_guid(conn, &v.volume_guid)?;

    if found.is_none() {
        // 退一步：容忍尾部反斜杠 / 大小写差异，宁可复用也别建出第二条卷
        let target = norm_guid(&v.volume_guid);
        found = list(conn)?
            .into_iter()
            .find(|r| norm_guid(&r.volume_guid) == target);
    }

    if let Some(existing) = found {
        conn.execute(
            "UPDATE volumes SET volume_guid=?2, serial=?3, label=?4, fs=?5, removable=?6,
                    last_mount_path=?7
             WHERE id=?1",
            params![
                existing.id,
                v.volume_guid,
                v.serial,
                v.label,
                v.fs,
                v.removable as i64,
                v.last_mount_path
            ],
        )?;
        return Ok(existing.id);
    }
    insert(conn, v)
}

/// 按 id 查卷。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Volume>> {
    let sql = format!("SELECT {COLS} FROM volumes WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], map_row).optional()?)
}

/// 设置卷状态（拔出 → offline / 插回 → online）。
pub fn set_state(conn: &Connection, id: i64, state: VolumeState) -> Result<()> {
    conn.execute(
        "UPDATE volumes SET state = ?1 WHERE id = ?2",
        params![state.as_str(), id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_test_db;

    fn mem() -> Connection {
        open_test_db().unwrap()
    }

    /// 同一个卷，两次调用只因 guid 尾部反斜杠个数不同 —— **不许建出第二条卷**。
    ///
    /// 这是 2026-10-27 那次「从图库移除没生效」的根因：手工修的卷记录多写了一个
    /// 反斜杠，程序扫描时精确匹配落空 → 新建一条卷 → 整批文件被索引两遍，
    /// 移除删掉一份、另一份还在，用户看起来就是「点了没反应」。
    #[test]
    fn 卷guid尾部反斜杠不同不会建出第二条卷() {
        let c = mem();

        let mut a = Volume::new(r"\?\Volume{6e03c1c7-0000-0000-0000-100000000000}\");
        a.label = Some("AI".into());
        a.serial = Some(3204099802);
        let id1 = upsert(&c, &a).unwrap();

        // 只多一个尾部反斜杠
        let mut b = Volume::new(r"\?\Volume{6e03c1c7-0000-0000-0000-100000000000}\\");
        b.label = Some("AIX".into());
        b.serial = Some(3204109802);
        let id2 = upsert(&c, &b).unwrap();

        assert_eq!(id1, id2, "应复用同一条卷记录");
        assert_eq!(list(&c).unwrap().len(), 1, "volumes 只能有一条");

        // 兜底命中后 guid 应被改写成规范形式，下次直接精确命中
        let row = get(&c, id1).unwrap().unwrap();
        assert_eq!(row.volume_guid, b.volume_guid);
        assert_eq!(row.label.as_deref(), Some("AIX"));
    }

    /// 大小写不同的同一 guid 也视为同一个卷。
    #[test]
    fn 卷guid大小写不同视为同一卷() {
        let c = mem();
        let id1 = upsert(&c, &Volume::new(r"\?\Volume{ABCDEF}\")).unwrap();
        let id2 = upsert(&c, &Volume::new(r"\?\volume{abcdef}\")).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(list(&c).unwrap().len(), 1);
    }

    /// 真正不同的卷仍然各自一条（别把兜底写成"合并所有卷"）。
    #[test]
    fn 不同的卷仍然分别建行() {
        let c = mem();
        let id1 = upsert(&c, &Volume::new(r"\?\Volume{aaa}\")).unwrap();
        let id2 = upsert(&c, &Volume::new(r"\?\Volume{bbb}\")).unwrap();
        assert_ne!(id1, id2);
        assert_eq!(list(&c).unwrap().len(), 2);
    }
}
