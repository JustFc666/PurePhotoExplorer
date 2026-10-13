//! 动图的**通用类型与资源上限**（GIF / 动画 WebP 共用）。
//!
//! ## 为什么抽出来
//! 两种格式的帧都是**增量合成**语义（每帧只描述自己那块，
//! 叠加到画布上才是一帧完整画面，还要按 dispose 决定下一帧从什么状态开始），
//! 所以解码结果对上层是同一种东西。上层（IPC `decode_animation` → 前端播放器
//! → PVA1 打包 → 渐进加载）只认这一套类型，**新增动图格式时不必再动上层**。
//!
//! ## 内存保护
//! 动图可能几百帧、每帧上千万像素（本项目的 8K 无损 WebP 单帧就是 126MB）。
//! 这里设了两道闸：帧数上限与所有输出帧的**总字节**上限，
//! 任一触发就截断并置 `truncated = true`，由上层提示用户，而不是让进程 OOM。

use crate::decoder::RgbaImage;

/// 单次解码的帧数上限
pub const MAX_FRAMES: usize = 300;
/// 所有输出帧 RGBA 数据的总字节上限（约 400 MB）
pub const MAX_TOTAL_BYTES: usize = 400 * 1024 * 1024;
/// `delay == 0` 时的兜底延时。
///
/// GIF 规范里 0 表示「尽快」，动画 WebP 的 `ANMF` 也允许写 0；
/// 但真实世界里大量文件用 0 表示「约 100ms」，主流浏览器统一按 100ms 处理，
/// 跟随它才不会让动图一闪而过。
pub const DEFAULT_DELAY_MS: u32 = 100;

/// 动图的一帧
pub struct AnimationFrame {
    /// 该帧的显示时长（毫秒，已做过 0 值兜底）
    pub delay_ms: u32,
    /// 合成这一帧之后的**完整画面**（已按 `DecodeTarget` 缩放）
    pub image: RgbaImage,
}

/// 动图解码结果
pub struct Animation {
    /// **帧像素的尺寸**（= 已按 `DecodeTarget` 缩放后的尺寸）
    ///
    /// ⚠️ 这里**不是**原始画布尺寸。PVA1 协议头里的 `width/height` 就是这个值，
    /// 前端按 `stride = width * height * 4` 切帧、命令层按它算分块字节预算 ——
    /// 报原始尺寸会让大动图（>2048）的包长与实际像素对不上，前端直接解析失败。
    pub width: u32,
    pub height: u32,
    pub frames: Vec<AnimationFrame>,
    /// 是否因上限被截断（帧数或内存）
    pub truncated: bool,
    /// `None` = 无限循环；`Some(n)` = 额外循环 n 次
    pub loop_count: Option<u16>,
}

impl Animation {
    /// 只有一帧 —— 按静态图处理
    pub fn is_static(&self) -> bool {
        self.frames.len() <= 1
    }
}
