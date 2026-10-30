//! 解压缓存：把包内单个条目落成真实文件，供上层按普通路径处理。
//!
//! ## 目录结构
//! ```text
//! <cache_root>/
//!   <包哈希>/
//!     manifest.json      ← 包内名字 → 缓存文件名
//!     <内容哈希>_001.jpg ← 解出来的图
//! ```
//!
//! 包哈希 = `blake3(绝对路径 + 字节数 + mtime)` 前 16 位：
//! 包被替换/重新下载后哈希变了，旧目录自然成孤儿，**不会把新包的内容
//! 认成旧包的**（那会显示成完全不相干的图，比报错难查得多）。
//!
//! ## 为什么缓存文件名带内容哈希而不是原文件名
//! 1. 包内可能有 `a/001.jpg` 和 `b/001.jpg` —— 光用末段名会互相覆盖；
//! 2. 原文件名可能含 `<>:"|?*` 等 Windows 非法字符；
//! 3. 哈希名不随「列目录顺序变化」而变，缓存能一直命中。
//!
//! ## 并发
//! 同一张图可能被「缩略图队列 4 路并发 + 大图预览」同时请求。
//! 按**包**加锁（而不是全局锁）：同一个包串行解压（manifest 不会被写坏），
//! 不同包之间依然并行。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::{open, split_virtual};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// 主键：包哈希（和目录名一致，用于自检）
    key: String,
    /// 包绝对路径（人看的，不参与判定）
    pack: String,
    /// 包内名字 → 缓存文件名
    files: HashMap<String, String>,
}

/// 按包哈希分锁：同包串行、异包并行。
fn pack_lock(key: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    g.entry(key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// 包指纹：路径 + 字节数 + mtime。
fn pack_key(pack: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(pack)
        .map_err(|e| format!("读 {} 的信息失败：{e}", pack.display()))?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let mut h = blake3::Hasher::new();
    h.update(pack.to_string_lossy().to_lowercase().as_bytes());
    h.update(&size.to_le_bytes());
    h.update(&mtime.to_le_bytes());
    Ok(h.finalize().to_hex()[..16].to_string())
}

/// 缓存文件名：内容哈希前 8 位 + 清洗过的原名（只为肉眼可读，不参与唯一性）。
fn cache_name(inner: &str) -> String {
    let h = blake3::Hash::from_bytes(blake3::hash(inner.as_bytes()).into()).to_hex();
    let stem = inner.rsplit('/').next().unwrap_or(inner);
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(60)
        .collect();
    format!("{}_{safe}", &h[..8])
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

fn load_manifest(dir: &Path, key: &str, pack: &Path) -> Manifest {
    let p = manifest_path(dir);
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(m) = serde_json::from_str::<Manifest>(&s) {
            // key 对不上说明目录被复用了，重建
            if m.key == key {
                return m;
            }
        }
    }
    Manifest {
        key: key.to_string(),
        pack: pack.to_string_lossy().into_owned(),
        files: HashMap::new(),
    }
}

fn save_manifest(dir: &Path, m: &Manifest) {
    let p = manifest_path(dir);
    let tmp = p.with_extension("json.part");
    if let Ok(s) = serde_json::to_string(m) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// 把包内 `inner` 解到缓存目录，返回真实路径。
///
/// 已解过就直接返回（一次解压，后续零成本）。
pub fn extract_one(pack: &str, inner: &str, cache_root: &Path) -> Result<PathBuf, String> {
    let pack_path = Path::new(pack);
    let key = pack_key(pack_path)?;
    let dir = cache_root.join(&key);

    let lock = pack_lock(&key);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());

    let mut m = load_manifest(&dir, &key, pack_path);

    if let Some(name) = m.files.get(inner) {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
        // 文件被人删了（或被清理工具清掉），下面重新解一次
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("建缓存目录失败：{e}"))?;

    // ⚠️ 读之前再确认一次虚拟路径没被搞错：`split_virtual` 已经校验过包存在，
    //    但这里 `pack` 是独立的字符串参数，还是检查一下更稳。
    if split_virtual(&format!("{pack}!{inner}")).is_none() {
        return Err(format!("无效的压缩包内路径：{pack} → {inner}"));
    }

    let mut arc = open(pack)?;
    let data = arc.read(inner)?;

    let name = cache_name(inner);
    let target = dir.join(&name);
    let tmp = dir.join(format!("{name}.part"));
    std::fs::write(&tmp, &data).map_err(|e| format!("写缓存失败：{e}"))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("落盘缓存失败：{e}"))?;

    m.files.insert(inner.to_string(), name);
    save_manifest(&dir, &m);

    Ok(target)
}

/// 缓存根目录（`%LOCALAPPDATA%\PurePhotoExplorer\arcache`）。
///
/// 与 `manager` 的默认数据目录同源，保证「卸载/清理数据目录」时一起清掉。
pub fn default_cache_root() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("PurePhotoExplorer").join("arcache")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::SimpleFileOptions = Default::default();
            for (n, d) in files {
                w.start_file(*n, opts).unwrap();
                w.write_all(d).unwrap();
            }
            w.finish().unwrap();
        }
        p
    }

    /// 核心契约：同一个条目解两次只有第一次真解，第二次直接命中。
    #[test]
    fn 解一次后续命中() {
        let root = std::env::temp_dir().join("pv_arc_cache_test1");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let pack = make_zip(&root, "a.cbz", &[("001.jpg", b"AAA"), ("002.jpg", b"BBB")]);

        let out1 = extract_one(pack.to_str().unwrap(), "001.jpg", &root).unwrap();
        assert_eq!(std::fs::read(&out1).unwrap(), b"AAA");

        // 删掉缓存文件再取，应当重新解出同样的内容
        std::fs::remove_file(&out1).unwrap();
        let out2 = extract_one(pack.to_str().unwrap(), "001.jpg", &root).unwrap();
        assert_eq!(std::fs::read(&out2).unwrap(), b"AAA");

        // 同名不同目录的两个条目不能互相覆盖
        let pack2 = make_zip(
            &root,
            "b.cbz",
            &[("x/001.jpg", b"XXX"), ("y/001.jpg", b"YYY")],
        );
        let a = extract_one(pack2.to_str().unwrap(), "x/001.jpg", &root).unwrap();
        let b = extract_one(pack2.to_str().unwrap(), "y/001.jpg", &root).unwrap();
        assert_ne!(a, b, "同名不同目录必须落到不同缓存文件");
        assert_eq!(std::fs::read(&a).unwrap(), b"XXX");
        assert_eq!(std::fs::read(&b).unwrap(), b"YYY");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 包被替换（大小/mtime 变）后必须重新解 —— 否则会显示旧包里的图。
    #[test]
    fn 包变了缓存失效() {
        let root = std::env::temp_dir().join("pv_arc_cache_test2");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let pack = make_zip(&root, "c.cbz", &[("001.jpg", b"OLD")]);
        let p = pack.to_str().unwrap();
        let a = extract_one(p, "001.jpg", &root).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"OLD");

        // 换内容 + 确保 mtime 变化
        std::thread::sleep(std::time::Duration::from_millis(20));
        make_zip(&root, "c.cbz", &[("001.jpg", b"NEW!")]);
        let b = extract_one(p, "001.jpg", &root).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"NEW!", "包换了就该重新解");

        let _ = std::fs::remove_dir_all(&root);
    }
}
