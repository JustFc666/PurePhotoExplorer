//! 路径解析 + 缩略图三级回退（**L1 内存 → L2 磁盘 → 现生成**）。
//!
//! 缓存键由「绝对路径 + size + mtime」算出，所以路径解析必须和取图放在一起：
//! 索引行一没就再也算不出键，缓存文件会变成永远无法定位的孤儿。

use std::path::{Path, PathBuf};

use pv_cache::{compute_key, CachedImage, CacheKey, CacheSource, ThumbTier};
use pv_decode::{DecodePurpose, DecodeTarget};
use pv_index::models::Image;
use pv_index::repo;

use crate::{Result, Store, Thumb};

impl Store {
    /// 由索引行还原**绝对路径**：卷挂载点 + 文件夹相对路径 + 文件名。
    ///
    /// 卷未挂载（`last_mount_path` 为空）时返回 `None`——离线状态。
    pub fn abs_path(&self, image: &Image) -> Result<Option<PathBuf>> {
        let Some(dir) = repo::folders::get(&self.conn, image.dir_id)? else {
            return Ok(None);
        };
        let Some(vol) = repo::volumes::get(&self.conn, dir.volume_id)? else {
            return Ok(None);
        };
        let Some(mount) = vol.last_mount_path else {
            return Ok(None);
        };
        let Some(name) = image.rel_name.as_deref() else {
            return Ok(None);
        };
        let mut p = PathBuf::from(mount);
        for part in dir.rel_path.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p.push(name);
        Ok(Some(p))
    }

    /// 取缩略图：**L1 内存 → L2 磁盘 → 现生成**（生成后回填 L1 与 L2）。
    pub fn thumbnail(&self, image: &Image, tier: ThumbTier) -> Result<Option<Thumb>> {
        let Some(abs) = self.abs_path(image)? else {
            return Ok(None);
        };
        let key = self.cache_key(image, &abs, tier);

        // L1：内存位图
        if let Some(c) = self.l1.get(&key) {
            return Ok(Some(Thumb {
                width: c.width,
                height: c.height,
                rgba: c.rgba.to_vec(),
            }));
        }

        // L2：磁盘字节 → 解回位图
        if let Some(bytes) = self.l2.get(&key)? {
            /*
             * ⛔⛔ 必须 `exact(Thumbnail, None)`，**不能** `new(Thumbnail)`。
             *     `DecodePurpose::Thumbnail` 的 `suggested_max_edge()` 是 **256**，
             *     用它解码会把缓存里的 720×1280 又缩成 144×256 —— 所有 L2 命中的档位
             *     一律退化成 256 长边（md/lg/xl 全废），表现为「缩略图和头像都比预期糊」。
             *     2026-10-27 实测：xl 档缓存文件 720×1280，`thumbnail()` 返回 144×256。
             *     缓存文件本身就是该档位的目标尺寸，直接按原始大小解码即可。
             */
            let img = pv_decode::decode_jpeg_from_bytes(
                &bytes,
                DecodeTarget::exact(DecodePurpose::Thumbnail, None),
            )?;
            let t = Thumb {
                width: img.width,
                height: img.height,
                rgba: img.data.clone(),
            };
            self.l1.put(key, CachedImage::new(img.width, img.height, img.data));
            return Ok(Some(t));
        }

        // 都没有：现解码 + 编码，回填 L1/L2
        let (bmp, bytes) =
            pv_decode::make_thumbnail_with_bitmap(&self.registry, &abs, tier.long_edge())?;
        self.l2.put(&key, &bytes)?;
        let t = Thumb {
            width: bmp.width,
            height: bmp.height,
            rgba: bmp.data.clone(),
        };
        self.l1.put(key, CachedImage::new(bmp.width, bmp.height, bmp.data));
        Ok(Some(t))
    }

    /// 缓存键对应的 L2 文件路径（测试/诊断用）。
    pub fn cache_file(&self, image: &Image, tier: ThumbTier) -> Option<PathBuf> {
        let abs = self.abs_path(image).ok().flatten()?;
        Some(self.l2.path_of(&self.cache_key(image, &abs, tier)))
    }

    pub(crate) fn cache_key(&self, image: &Image, abs: &Path, tier: ThumbTier) -> CacheKey {
        compute_key(
            &CacheSource::Plain {
                path: abs.to_path_buf(),
                size: image.size.unwrap_or(0) as u64,
                mtime_ns: image.mtime_ns.unwrap_or(0) as i128,
            },
            tier,
        )
    }
}
