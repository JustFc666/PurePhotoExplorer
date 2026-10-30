//! ZIP / CBZ 后端（纯 Rust，`zip` crate）。
//!
//! 选它而不是 7z.exe：zip 是最常见的图片包格式，且**纯 Rust 打开只要几百微秒**
//! （读中央目录），而起一个 7z.exe 进程光 CreateProcess 就要 20–40ms。
//! 浏览器里点开一个目录可能同时要 30 张缩略图，走 CLI 会明显卡。

use std::path::Path;

use crate::{Entry, Pack};

pub struct ZipPack {
    inner: zip::ZipArchive<std::fs::File>,
}

impl ZipPack {
    pub fn open(path: &Path) -> Result<Self, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
        let inner = zip::ZipArchive::new(f).map_err(|e| format!("不是有效的 zip：{e}"))?;
        Ok(Self { inner })
    }
}

impl Pack for ZipPack {
    fn entries(&mut self) -> Result<Vec<Entry>, String> {
        let mut out = Vec::with_capacity(self.inner.len());
        for i in 0..self.inner.len() {
            // `by_index` 出错（损坏条目）就跳过这一个 —— 一个坏条目不该让整包打不开
            let Ok(f) = self.inner.by_index(i) else {
                continue;
            };
            let name = f.name().replace('\\', "/");
            // 目录条目（以 `/` 结尾）跳过
            if name.ends_with('/') {
                continue;
            }
            out.push(Entry {
                name,
                size: f.size(),
            });
        }
        Ok(out)
    }

    fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;

        let mut f = self
            .inner
            .by_name(name)
            .map_err(|e| format!("包里没有 {name}：{e}"))?;
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)
            .map_err(|e| format!("解压 {name} 失败：{e}"))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 zip，再列 + 读，验证「只返回图片、按自然序」这套约定。
    #[test]
    fn 列清单与读取() {
        let dir = std::env::temp_dir().join("pv_arc_zip_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.zip");

        {
            let f = std::fs::File::create(&path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::SimpleFileOptions = Default::default();
            for (name, data) in [
                ("10.jpg", &b"x"[..]),
                ("2.jpg", &b"y"[..]),
                ("readme.txt", &b"z"[..]),
            ] {
                use std::io::Write;
                w.start_file(name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }

        let mut p = ZipPack::open(&path).unwrap();
        let mut names: Vec<String> = p
            .entries()
            .unwrap()
            .into_iter()
            .filter(|e| crate::is_image_name(&e.name))
            .map(|e| e.name)
            .collect();
        names.sort_by(|a, b| crate::natural_cmp(a, b));
        assert_eq!(names, vec!["2.jpg", "10.jpg"]);
        assert_eq!(p.read("2.jpg").unwrap(), b"y");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
