//! 解码产物：8bit RGBA 像素图。

use pv_core::error::{PvError, Result};

/// 解码产物：已解码的像素数据（8bit RGBA）
#[derive(Debug, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// 长度 = width * height * 4，通道序 RGBA
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// 分配未初始化缓冲区。尺寸非法时返回 Err 而非 panic（FR-1.6）
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| PvError::Decode("图像尺寸溢出，疑似损坏文件".into()))?;
        Ok(Self {
            width,
            height,
            data: vec![0u8; len],
        })
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + x as usize) * 4;
        Some([
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn RgbaImage_可安全构造() {
        let img = RgbaImage::new(4, 4).unwrap();
        assert_eq!(img.data.len(), 64);
        assert_eq!(img.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(img.pixel(4, 0), None);
    }

    #[test]
    fn RgbaImage_尺寸溢出返回Err不panic() {
        let r = RgbaImage::new(u32::MAX, u32::MAX);
        assert!(r.is_err(), "超大尺寸必须返回 Err");
    }
}
