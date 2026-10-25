//! 连接层：打开数据库、应用 pragma。不含业务逻辑。

use crate::error::Result;
use rusqlite::Connection;
use std::path::Path;

/// 打开磁盘数据库（WAL 模式）。
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn, true)?;
    Ok(conn)
}

/// 打开内存数据库（测试用；内存库不支持 WAL）。
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    apply_pragmas(&conn, false)?;
    Ok(conn)
}

/// 统一 pragma：外键约束、忙等待、同步级别。
fn apply_pragmas(conn: &Connection, wal: bool) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;",
    )?;
    if wal {
        // 内存库执行此句会返回 "memory"，execute_batch 忽略结果，安全。
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    }
    Ok(())
}

/// 在事务中执行批量写入（`pv-scan` 首扫/增量落库用）。
///
/// 闭包返回 `Err` 时**自动回滚**；成功则提交。闭包拿到的是 `&Connection`
/// （`Transaction` 自动解引用），因此可直接复用各 `repo::*` 函数。
pub fn with_tx<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let tx = conn.transaction()?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}
