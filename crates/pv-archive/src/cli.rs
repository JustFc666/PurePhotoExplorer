//! 7-Zip 命令行后端（RAR / 7z / 自解压 zip 等）。
//!
//! ## 为什么复用系统的 7z.exe，而不是编一个 RAR 解码器
//! RAR **没有可用的纯 Rust 解码器**：`unrar` 是 C++（要 MSVC 编译 + 非自由许可），
//! `unar` 是 GPL。7z.exe 本身又要随应用分发（用户机器上未必装了 7-Zip），
//! 复用它比自己维护一份 RAR 解码器划算得多。
//!
//! ## 关键：`e -so` 直接写 stdout，**不落临时目录**
//! 这是 ComicViewer 那边验证过的做法。7-Zip 不带 `-so` 时会解到
//! `%TEMP%\7zO<随机>` 然后**自己删掉**，外面根本拿不到；带上 `-so` 就是
//! 「把解出来的字节直接给我」，既没有临时目录残留，也不用反推源包。
//!
//! ## 编码
//! 中文包里全是中文文件名。7-Zip 21.07+ 支持 `-sccUTF-8` 把**控制台输出**
//! 强制成 UTF-8（不加的话在中文 Windows 上是 GBK，Rust 侧解析出来是问号）。
//! 读条目字节走 stdout 二进制，不受这个影响。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Entry, Pack};

/// 找到 7z.exe。
///
/// 顺序：环境变量 `PV_7Z` → exe 同目录 → 常见安装路径 → `PATH`。
/// 前两个是「随应用分发」的场景（便携版把 7z.exe 放 exe 旁边就能用）。
pub fn seven_zip_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PV_7Z") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("7z.exe");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    let mut candidates = vec![
        PathBuf::from(r"C:\Program Files\7-Zip\7z.exe"),
        PathBuf::from(r"C:\Program Files (x86)\7-Zip\7z.exe"),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(local);
        candidates.push(base.join(r"Programs\7-Zip\7z.exe"));
        candidates.push(base.join(r"7-Zip\7z.exe"));
    }
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    // PATH 里找（最后兜底）
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join("7z.exe"))
        .find(|p| p.is_file())
}

pub struct CliPack {
    exe: PathBuf,
    pack: PathBuf,
}

impl CliPack {
    pub fn open(path: &Path) -> Result<Self, String> {
        let exe = seven_zip_exe().ok_or_else(|| {
            "打开 RAR / 7z 需要 7-Zip。请安装 7-Zip，或把 7z.exe 放到程序目录下".to_string()
        })?;
        Ok(Self {
            exe,
            pack: path.to_path_buf(),
        })
    }
}

impl Pack for CliPack {
    fn entries(&mut self) -> Result<Vec<Entry>, String> {
        // `-ba` 只输出条目（去掉页眉页脚），`-slt` 每行一个 `键 = 值`
        let out = Command::new(&self.exe)
            .args(["l", "-slt", "-ba", "-sccUTF-8"])
            .arg(&self.pack)
            .output()
            .map_err(|e| format!("调用 7z.exe 失败：{e}"))?;

        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr);
            let msg = msg.trim();
            return Err(format!(
                "7z 读不了这个包：{}",
                if msg.is_empty() { "（无输出）" } else { msg }
            ));
        }

        let text = String::from_utf8_lossy(&out.stdout);
        let mut items = Vec::new();
        let mut name: Option<String> = None;
        let mut size: u64 = 0;

        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Path = ") {
                // 上一段收尾
                if let Some(n) = name.take() {
                    if !n.ends_with('/') {
                        items.push(Entry { name: n, size });
                    }
                }
                name = Some(rest.trim().replace('\\', "/"));
                size = 0;
            } else if let Some(rest) = line.strip_prefix("Size = ") {
                size = rest.trim().parse().unwrap_or(0);
            }
        }
        if let Some(n) = name.take() {
            if !n.ends_with('/') {
                items.push(Entry { name: n, size });
            }
        }
        Ok(items)
    }

    fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
        // `-spd` 关掉通配符匹配：包内名字里有 `[` `]` 时会被当成通配符，
        // 匹配到别的文件甚至多个文件（多个时 7z 会在 stdout 里拼起来，静默出错）。
        let out = Command::new(&self.exe)
            .args(["e", "-so", "-y", "-spd"])
            .arg(&self.pack)
            .arg(name)
            .output()
            .map_err(|e| format!("调用 7z.exe 失败：{e}"))?;

        if !out.status.success() || out.stdout.is_empty() {
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("解压 {name} 失败：{}", msg.trim()));
        }
        Ok(out.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 能定位到7zip() {
        // 认不出就跳过：CI / 别的机器上可能没装
        let Some(exe) = seven_zip_exe() else {
            eprintln!("没有 7z.exe，跳过");
            return;
        };
        assert!(exe.is_file());
    }
}
