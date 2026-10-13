//! 加密容器：`.pvault` 格式定义与密码学方案。
//!
//! **威胁模型（见开发文档 §0.0 确认三）：防离线翻阅。**
//! 攻击者拿到磁盘/文件副本后，无法读取内容、无法得知文件名。
//!
//! 明确不做：隐写、内存加密、反调试。
//! 密码学一律用成熟 crate，本文件只做组装，**禁止自研算法**。

use serde::{Deserialize, Serialize};

/// 容器头（4 KiB，明文）。仅含 KDF 参数与盐，不含任何可读信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultHeader {
    /// 魔数 "PVLT"
    pub magic: [u8; 4],
    pub version: u16,
    /// 预留扩展标志位，保证未来可演进（风险 R-05）
    pub flags: u16,
    pub kdf: KdfParams,
    /// 派生主密钥用
    pub salt: [u8; 32],
    /// 索引块专用 nonce
    pub index_nonce: [u8; 12],
    /// 头部完整性校验，防止篡改参数导致降级
    pub header_mac: [u8; 32],
    pub created_at: i64,
}

impl VaultHeader {
    pub const MAGIC: [u8; 4] = *b"PVLT";
    pub const SIZE: usize = 4096;
    pub const CURRENT_VERSION: u16 = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KdfAlgo {
    /// Argon2id —— 唯一推荐
    Argon2Id,
}

/// Argon2id 参数。
///
/// 默认 64 MiB / t=3：现代机器解锁 < 500 ms，
/// 同时对离线暴力破解有实质成本（见文档 §5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub algo: KdfAlgo,
    /// 内存（KiB）。最低 32768（32 MiB），低于此值直接拒绝创建
    pub memory_kib: u32,
    /// 迭代次数
    pub iterations: u32,
    /// 并行度
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            algo: KdfAlgo::Argon2Id,
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

impl KdfParams {
    /// 低于 32 MiB 视为不安全，拒绝（抗 GPU/ASIC 攻击下限）
    pub const MIN_MEMORY_KIB: u32 = 32 * 1024;

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.memory_kib < Self::MIN_MEMORY_KIB {
            return Err("Argon2 内存参数不得低于 32 MiB");
        }
        if self.iterations < 2 {
            return Err("迭代次数不得低于 2");
        }
        if self.parallelism == 0 || self.parallelism > 16 {
            return Err("并行度必须在 1-16 之间");
        }
        Ok(())
    }

    /// 估算解锁耗时（用于 UI 提示"正在解锁…"）
    pub fn estimated_unlock_ms(&self) -> u64 {
        // 经验值：64 MiB / t=3 在现代 CPU 约 200-400 ms
        let base = 300u64;
        let mem_ratio = self.memory_kib as f64 / (64 * 1024) as f64;
        let iter_ratio = self.iterations as f64 / 3.0;
        (base as f64 * mem_ratio * iter_ratio.max(0.5)) as u64
    }
}

/// 索引条目。**整个索引块加密**（FR-5.8：文件名与大小必须加密）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultEntryMeta {
    /// 随机 UUID，不泄露原名
    pub entry_uuid: [u8; 16],
    /// **加密后的**原文件名（长度信息也隐含在密文中）
    pub name_cipher: Vec<u8>,
    /// 原始大小（需与密文一同存储才能定位，但密文长度已泄露近似值）
    /// —— 接受：文件名是关键，精确大小不是
    pub size: u64,
    pub mtime: i64,
    /// 图像数据在容器内的偏移
    pub data_offset: u64,
    /// 密文长度（含 GCM tag）
    pub data_len: u32,
    /// 加密缩略图块（可选，减小容器体积）
    pub thumb_offset: u64,
    pub thumb_len: u32,
    /// 内嵌格式标识，用于图标与快速分类（不泄露内容）
    pub format_hint: pv_core::format::ImageFormat,
}

/// 自动锁定策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockTrigger {
    /// 空闲超时
    Idle,
    /// 系统锁屏
    SystemLock,
    /// 用户手动
    Manual,
    /// 应用退出
    Exit,
}

/// 解锁会话状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Locked,
    Unlocked,
}

/// 错误密码的指数退避（FR-5.5）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffState {
    /// 尚未出错
    Clean,
    /// 已出错 n 次，需等待 2^n 秒
    Waiting { attempts: u32 },
}

impl BackoffState {
    pub fn next_wait_secs(&self) -> Option<u64> {
        match self {
            Self::Clean => None,
            Self::Waiting { attempts } => {
                // 2s → 4s → 8s → 16s ... 上限 5 分钟
                Some((1u64 << (*attempts).min(17)).min(300))
            }
        }
    }

    pub fn record_failure(&mut self) {
        let a = match self {
            Self::Clean => 1,
            Self::Waiting { attempts } => *attempts + 1,
        };
        *self = Self::Waiting { attempts: a };
    }

    pub fn reset(&mut self) {
        *self = Self::Clean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 魔数与版本常量正确() {
        assert_eq!(&VaultHeader::MAGIC, b"PVLT");
        assert_eq!(VaultHeader::CURRENT_VERSION, 1);
    }

    #[test]
    fn 默认KDF参数通过校验() {
        assert!(KdfParams::default().validate().is_ok());
    }

    #[test]
    fn 拒绝过低的内存参数() {
        let p = KdfParams {
            memory_kib: 1024, // 1 MiB，远低于下限
            ..Default::default()
        };
        assert!(p.validate().is_err(), "低于 32 MiB 必须拒绝");
    }

    #[test]
    fn 拒绝过低的迭代次数() {
        let p = KdfParams {
            iterations: 1,
            ..Default::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn 拒绝非法并行度() {
        assert!(KdfParams { parallelism: 0, ..Default::default() }.validate().is_err());
        assert!(KdfParams { parallelism: 99, ..Default::default() }.validate().is_err());
    }

    #[test]
    fn 退避时间指数增长并封顶() {
        let mut b = BackoffState::Clean;
        assert_eq!(b.next_wait_secs(), None);

        b.record_failure();
        assert_eq!(b.next_wait_secs(), Some(2));

        b.record_failure();
        assert_eq!(b.next_wait_secs(), Some(4));

        b.record_failure();
        assert_eq!(b.next_wait_secs(), Some(8));

        // 连续多次错误后封顶 300 秒，不会无限增长
        for _ in 0..20 {
            b.record_failure();
        }
        assert_eq!(b.next_wait_secs(), Some(300), "退避应封顶 5 分钟");
    }

    #[test]
    fn 成功后重置退避() {
        let mut b = BackoffState::Clean;
        b.record_failure();
        b.record_failure();
        b.reset();
        assert_eq!(b.next_wait_secs(), None);
    }

    #[test]
    fn 默认KDF解锁耗时在交互可接受范围() {
        let ms = KdfParams::default().estimated_unlock_ms();
        assert!(
            (100..=2000).contains(&ms),
            "默认参数解锁应 < 2 秒，实际估算 {ms} ms"
        );
    }

    #[test]
    fn 内存越大估算耗时越长() {
        let small = KdfParams {
            memory_kib: 32 * 1024,
            ..Default::default()
        };
        let large = KdfParams {
            memory_kib: 128 * 1024,
            ..Default::default()
        };
        assert!(large.estimated_unlock_ms() > small.estimated_unlock_ms());
    }
}
