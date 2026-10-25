//! 迁移：用 `PRAGMA user_version` 驱动，**可重入**。

use crate::error::{IndexError, Result};
use crate::schema;
use rusqlite::Connection;

/// 本程序支持的最新 schema 版本。新增迁移时 +1 并在 [`migrate`] 追加分支。
pub const SCHEMA_VERSION: i64 = 4;

/// 把数据库升级到 [`SCHEMA_VERSION`]。
///
/// - 重复调用安全（已是最新则空转）；
/// - 库版本高于程序 → 报 [`IndexError::SchemaTooNew`]，避免旧程序写坏新库。
pub fn migrate(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if current > SCHEMA_VERSION {
        return Err(IndexError::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }

    if current < 1 {
        conn.execute_batch(schema::DDL_V1)?;
    }
    if current < 2 {
        // `images.meta_checked`：区分「没读过 EXIF」与「读过但文件没有」，
        // 否则没有拍摄时间的行每次启动都会被重新读一遍（大库上是几十秒浪费）。
        conn.execute_batch(schema::DDL_V2)?;
    }
    if current < 3 {
        // `image_places`：每张图解析出的离线城市名（侧栏「地点」分区 + 按地点筛）。
        conn.execute_batch(schema::DDL_V3)?;
    }
    if current < 4 {
        // `faces` / `persons` 重建：人脸归纳的数据模型（归一化坐标 + 模型版本 + 质量分）。
        // v1 建的是空壳，从没写过数据，所以 DROP 重建是安全的。
        conn.execute_batch(schema::DDL_V4)?;
    }

    if current < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

/// 读取当前库的 schema 版本。
pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}
