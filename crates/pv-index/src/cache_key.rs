//! 缓存键计算。
//!
//! **安全关键**：缓存键必须区分普通图片与加密容器内容。
//! 容器内容走 `(container_id, entry_uuid)`，绝不能用路径哈希——
//! 否则容器内图片会复用/污染普通缓存目录，导致明文泄露（见文档 §3.5）。

use serde::{Deserialize, Serialize};

/// 缩略图档位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThumbTier {
    /// 网格小图，约 256px
    Small,
    /// 网格大图 / 单图预览，约 512px
    Medium,
    /// 高清预览，约 1024px
    Large,
    /// 编辑器 / 全屏首屏，约 2048px
    Xlarge,
}

impl ThumbTier {
    pub fn long_edge(self) -> u32 {
        match self {
            Self::Small => 256,
            Self::Medium => 512,
            Self::Large => 1024,
            Self::Xlarge => 2048,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "sm",
            Self::Medium => "md",
            Self::Large => "lg",
            Self::Xlarge => "xl",
        }
    }

    /// 单档缩略图字节数估算。
    ///
    /// WebP q82 实测量级约 `n * n * 0.11` 字节（n 为长边像素）。
    /// 四档合计约 600 KB/张，1 万张约 **5.7 GB**（见文档 §3.1 容量重算）。
    pub fn approx_bytes(self) -> u64 {
        let n = self.long_edge() as u64;
        n * n * 11 / 100
    }
}

/// 缓存来源
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheSource {
    /// 普通文件：blake3(规范化路径 || size || mtime_ns)
    Plain {
        path: std::path::PathBuf,
        size: u64,
        mtime_ns: i128,
    },
    /// 加密容器内容：绝不使用路径
    Vault {
        container_id: uuid_lite::Uuid,
        entry_uuid: uuid_lite::Uuid,
    },
}

/// 极简 UUID（避免为 pv-index 引入完整 uuid crate 依赖）
pub mod uuid_lite {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Uuid(#[serde(with = "serde_bytes16")] pub [u8; 16]);

    /// serde 默认把定长数组当 16 个元素序列化，这里统一为紧凑字节数组
    mod serde_bytes16 {
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        pub fn serialize<S: Serializer>(v: &[u8; 16], s: S) -> Result<S::Ok, S::Error> {
            v.serialize(s)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 16], D::Error> {
            let v = <[u8; 16]>::deserialize(d)?;
            Ok(v)
        }
    }

    impl Uuid {
        pub fn nil() -> Self {
            Self([0u8; 16])
        }

        pub fn is_nil(&self) -> bool {
            self.0 == [0u8; 16]
        }
    }
}

/// 缓存键：20 字节（16 字节摘要 + 4 字节档位）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey(pub [u8; 20]);

impl CacheKey {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 缩略图文件名后缀（不含路径）
    pub fn filename(&self) -> String {
        format!("{}-{}", &self.hex()[..24], self.0[19])
    }
}

/// 计算缓存键
pub fn compute_key(source: &CacheSource, tier: ThumbTier) -> CacheKey {
    let mut hasher = blake3::Hasher::new();

    match source {
        CacheSource::Plain { path, size, mtime_ns } => {
            hasher.update(b"plain");
            // Windows 路径大小写不敏感，先规范化避免同一文件产生两个键
            let norm = normalize_windows_path(path);
            hasher.update(norm.as_os_str().as_encoded_bytes());
            hasher.update(&size.to_le_bytes());
            hasher.update(&mtime_ns.to_le_bytes());
        }
        CacheSource::Vault { container_id, entry_uuid } => {
            hasher.update(b"vault");
            hasher.update(&container_id.0);
            hasher.update(&entry_uuid.0);
        }
    }

    hasher.update(tier.as_str().as_bytes());

    let digest = hasher.finalize();
    let mut key = [0u8; 20];
    key.copy_from_slice(&digest.as_bytes()[..20]);
    CacheKey(key)
}

/// Windows 路径规范化：转小写 + 统一分隔符。
/// 避免 `C:\A.JPG` 与 `c:\a.jpg` 产生两个缓存条目。
fn normalize_windows_path(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '/' => out.push('\\'),
            c => out.push(c.to_ascii_lowercase()),
        }
    }
    std::path::PathBuf::from(out)
}

/// 容器缩略图存储的子目录名（不泄露容器名与位置）
pub fn vault_thumb_subdir(container_id: &uuid_lite::Uuid) -> String {
    format!("vault-{}", hex8(&container_id.0))
}

/// 容器内条目缓存文件名（不泄露原文件名）
pub fn vault_thumb_filename(entry_uuid: &uuid_lite::Uuid) -> String {
    format!("{}.enc", hex8(&entry_uuid.0))
}

fn hex8(bytes: &[u8; 16]) -> String {
    bytes[..4].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plain(path: &str, size: u64, mtime: i128) -> CacheSource {
        CacheSource::Plain {
            path: PathBuf::from(path),
            size,
            mtime_ns: mtime,
        }
    }

    #[test]
    fn 相同输入产生相同键() {
        let a = compute_key(&plain("C:/a.jpg", 100, 12345), ThumbTier::Medium);
        let b = compute_key(&plain("C:/a.jpg", 100, 12345), ThumbTier::Medium);
        assert_eq!(a, b);
    }

    #[test]
    fn 文件大小变化则键变化() {
        let a = compute_key(&plain("C:/a.jpg", 100, 1), ThumbTier::Medium);
        let b = compute_key(&plain("C:/a.jpg", 101, 1), ThumbTier::Medium);
        assert_ne!(a, b, "文件被修改后缓存必须失效");
    }

    #[test]
    fn 修改时间变化则键变化() {
        let a = compute_key(&plain("C:/a.jpg", 100, 1), ThumbTier::Medium);
        let b = compute_key(&plain("C:/a.jpg", 100, 2), ThumbTier::Medium);
        assert_ne!(a, b, "mtime 变化必须触发缓存失效");
    }

    #[test]
    fn 档位不同则键不同() {
        let src = plain("C:/a.jpg", 100, 1);
        let a = compute_key(&src, ThumbTier::Small);
        let b = compute_key(&src, ThumbTier::Large);
        assert_ne!(a, b);
    }

    #[test]
    fn 路径大小写与分隔符归一化() {
        let a = compute_key(&plain("C:/Photos/A.JPG", 100, 1), ThumbTier::Medium);
        let b = compute_key(&plain("c:\\photos\\a.jpg", 100, 1), ThumbTier::Medium);
        assert_eq!(a, b, "Windows 下路径应大小写与分隔符不敏感");
    }

    #[test]
    fn 容器内容与普通文件键不冲突() {
        let v = compute_key(
            &CacheSource::Vault {
                container_id: uuid_lite::Uuid([1u8; 16]),
                entry_uuid: uuid_lite::Uuid([2u8; 16]),
            },
            ThumbTier::Medium,
        );
        let p = compute_key(&plain("C:/photos/x.jpg", 100, 1), ThumbTier::Medium);
        assert_ne!(v, p, "容器缓存与普通缓存必须完全隔离");
    }

    #[test]
    fn 容器内容不使用路径信息() {
        // 仅用 container_id + entry_uuid，两个不同位置同一容器键相同
        let k1 = compute_key(
            &CacheSource::Vault {
                container_id: uuid_lite::Uuid([7u8; 16]),
                entry_uuid: uuid_lite::Uuid([9u8; 16]),
            },
            ThumbTier::Medium,
        );
        let k2 = compute_key(
            &CacheSource::Vault {
                container_id: uuid_lite::Uuid([7u8; 16]),
                entry_uuid: uuid_lite::Uuid([9u8; 16]),
            },
            ThumbTier::Medium,
        );
        assert_eq!(k1, k2);
    }

    #[test]
    fn 容器缓存文件名不含原始信息() {
        let id = uuid_lite::Uuid([0xAB; 16]);
        let name = vault_thumb_filename(&id);
        assert!(name.ends_with(".enc"));
        assert!(!name.contains('\\') && !name.contains('/'));
        assert!(!name.contains("photos"), "文件名不得泄露路径");
        assert!(name.len() < 20, "文件名应短且为随机形式");
    }

    #[test]
    fn 容器子目录名不含容器原名() {
        let id = uuid_lite::Uuid([0xCD; 16]);
        let sub = vault_thumb_subdir(&id);
        assert!(sub.starts_with("vault-"));
        assert_eq!(sub.len(), "vault-".len() + 8);
    }

    #[test]
    fn 缩略图容量估算符合万级设计() {
        // 四档合计约 600 KB/张 → 1 万张约 5.7 GB
        let per_image: u64 = [
            ThumbTier::Small,
            ThumbTier::Medium,
            ThumbTier::Large,
            ThumbTier::Xlarge,
        ]
        .iter()
        .map(|t| t.approx_bytes())
        .sum();
        let total = per_image * 10_000;
        assert!(
            (4_000_000_000..8_000_000_000).contains(&total),
            "估算 {total} 字节应在 4-8 GB 区间（实测约 5.7 GB）"
        );
    }

    #[test]
    fn 默认磁盘缓存上限足以容纳万级库() {
        let need: u64 = [
            ThumbTier::Small,
            ThumbTier::Medium,
            ThumbTier::Large,
            ThumbTier::Xlarge,
        ]
        .iter()
        .map(|t| t.approx_bytes())
        .sum::<u64>()
            * 10_000;
        // 文档默认上限 5 GB —— 略小于四档全量，但只需前两档常驻即可
        let default_cap: u64 = 5 * 1024 * 1024 * 1024;
        let first_two: u64 =
            ThumbTier::Small.approx_bytes() + ThumbTier::Medium.approx_bytes();
        assert!(
            first_two * 10_000 < default_cap,
            "前两档（网格常用）应能全量放下"
        );
        assert!(
            need > default_cap,
            "四档全量超出默认上限是预期的，靠 LRU 淘汰冷档位"
        );
    }
}
