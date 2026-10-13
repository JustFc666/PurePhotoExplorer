//! 解码目标尺寸与等比缩放计算。

/// 解码请求的目标尺寸。
///
/// **为什么必须显式传目标尺寸**：单图浏览时 6000×4000 的图若整张解码，
/// 峰值内存 96 MB、光栅化要几百毫秒，滚动会卡死。
/// 因此浏览一律请求"能填满视口的最大档位"，而非原图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeTarget {
    /// 约束后的最长边（None = 不限制，按原图）
    pub max_edge: Option<u32>,
    /// 用途：决定选哪一档缓存与解码精度
    pub purpose: DecodePurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePurpose {
    /// 生成缩略图网格用图
    Thumbnail,
    /// 单图浏览显示
    Preview,
    /// 编辑器底图
    Edit,
    /// 导出（要求全尺寸 + 全精度）
    Export,
}

impl DecodePurpose {
    /// 建议的最长边。None = 原始尺寸
    pub fn suggested_max_edge(self) -> Option<u32> {
        match self {
            Self::Thumbnail => Some(256),
            Self::Preview => Some(2048),
            Self::Edit => Some(4096),
            Self::Export => None,
        }
    }
}

impl DecodeTarget {
    pub fn new(purpose: DecodePurpose) -> Self {
        Self {
            max_edge: purpose.suggested_max_edge(),
            purpose,
        }
    }

    /// 强制指定尺寸（导出时用）
    pub fn exact(purpose: DecodePurpose, max_edge: Option<u32>) -> Self {
        Self { max_edge, purpose }
    }

    /// 导出场景必须拿到全尺寸，不接受缩放
    pub fn is_full_res(&self) -> bool {
        self.purpose == DecodePurpose::Export
    }
}

/// 按目标尺寸计算等比缩放后的尺寸
pub fn fit_within(width: u32, height: u32, max_edge: Option<u32>) -> (u32, u32) {
    match max_edge {
        None => (width, height),
        Some(m) if m == 0 => (width, height),
        Some(m) => {
            let longest = width.max(height);
            if longest <= m {
                (width, height)
            } else if width >= height {
                (m, ((height as u64 * m as u64) / width as u64) as u32)
            } else {
                (((width as u64 * m as u64) / height as u64) as u32, m)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 方形图等比缩放() {
        assert_eq!(fit_within(6000, 4000, Some(2048)), (2048, 1365));
    }

    #[test]
    fn 竖构图等比缩放() {
        assert_eq!(fit_within(4000, 6000, Some(2048)), (1365, 2048));
    }

    #[test]
    fn 小图不放大() {
        // 800px 的图请求 2048 时保持原尺寸，避免无意义的插值与内存浪费
        assert_eq!(fit_within(800, 600, Some(2048)), (800, 600));
    }

    #[test]
    fn 导出目标不做缩放() {
        let t = DecodeTarget::new(DecodePurpose::Export);
        assert!(t.is_full_res());
        assert_eq!(fit_within(6000, 4000, t.max_edge), (6000, 4000));
    }

    #[test]
    fn 各用途默认档位递增() {
        let thumb = DecodeTarget::new(DecodePurpose::Thumbnail).max_edge.unwrap();
        let preview = DecodeTarget::new(DecodePurpose::Preview).max_edge.unwrap();
        let edit = DecodeTarget::new(DecodePurpose::Edit).max_edge.unwrap();
        assert!(thumb < preview, "缩略图应小于预览");
        assert!(preview < edit, "预览应小于编辑底图");
    }

    #[test]
    fn 零值目标尺寸不触发除零() {
        assert_eq!(fit_within(6000, 4000, Some(0)), (6000, 4000));
    }
}
