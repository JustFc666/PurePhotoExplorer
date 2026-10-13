//! 画布调整的落盘命令：`save_edited_image`。
//!
//! ## 两种写入模式（2026-10-10 起）
//! | 模式 | 触发 | 源格式限制 | 结果 |
//! |---|---|---|---|
//! | **覆盖原文件** | 不传 `target_path` | 仅 JPEG / PNG / BMP | 原文件被替换 |
//! | **另存为** | 传 `target_path` | 无（RAW / HEIC / WebP 都可以）| 新文件，原文件不动 |
//!
//! ## 语义（与用户 2026-10-07 的决策对齐）
//! 「原图永不修改」的旧原则**已作废**。现在提供**直接覆盖原文件**的保存，
//! 所以这个模块是**唯一会写用户原始文件**的地方，实现上刻意保守：
//!
//! 1. **先编码到内存，全部成功后才写盘**。任何一步失败（解码、变换、编码）
//!    都不会碰到原文件。
//! 2. **写临时文件 + 原子 rename**（见 `pv_decode::write_rgba_atomic`），
//!    避免中途失败留下半截文件。
//! 3. **不支持覆盖的格式直接拒绝**，并把原因说清楚。
//!
//! ## 为什么 RAW 只能「另存为」
//! RAW 画布上的像素是**相机内嵌的 JPEG 预览**（`RawPipeline::extract_preview`），
//! 不是传感器数据。覆盖保存等于用 8bit sRGB 预览替换掉 14bit Bayer 数据 ——
//! **画质永久降级且不可逆**。这条边界由 `can_overwrite_original` 守住，
//! 前端也会把按钮换成「另存为…」，让用户有一个明确的出口。
//!
//! ⚠️ **别把 `can_overwrite_original` 与 `can_encode` 混用**：前者管「能不能覆盖原文件」，
//! 后者管「能不能编码成这个格式」（= 能不能作为另存为的目标）。今天两者值相同，
//! 但接了 DNG 写出处之后 RAW 会变成「能另存为 DNG、仍不能覆盖原 RAW」。
//!
//! ## 为什么不落「角度微调」
//! 见 `pv_decode::transform` 的模块文档：任意角度重采样会导致
//! 「预览所见 ≠ 文件所得」+ 二次采样画质损失。
//! 写回只应用 **90° 整数倍旋转 + 比例裁剪**，两者都是像素无损的置换/裁切。

use std::path::PathBuf;

use pv_core::error::{PvError, Result};
use pv_decode::{
    crop_to_aspect, rotate90_steps, DecodePurpose, DecodeTarget, RgbaImage,
};
use tauri::State;

use super::dto::ErrorDto;
use crate::state::AppState;

/// 保存结果（成功时返回，供前端刷新视图）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResultDto {
    /// 写回后的实际像素宽度（旋转/裁剪后可能与原来不同）
    pub width: u32,
    /// 写回后的实际像素高度
    pub height: u32,
    /// 写回后的文件字节数
    pub file_size: u64,
    /// 实际应用的 90° 旋转步数（0-3，顺时针），供前端同步视图状态
    pub rotation_applied: i32,
    /// 是否应用了比例裁剪
    pub cropped: bool,
    /// **实际写入的路径**。
    ///
    /// 覆盖保存时等于原路径；另存为时是新路径 —— 前端要靠它
    /// 决定「刷新当前图」还是「提示已导出到某处」。
    pub saved_path: String,
    /// 是否是「另存为」（写到了新文件，原文件未被动过）。
    ///
    /// 前端据此**不要刷新画布**：原文件像素没变，刷新等于白刷一次，
    /// 还会把用户刚做的裁剪框冲掉。
    pub exported: bool,
    /// 是否只拿到了**内嵌预览**的分辨率（RAW 恒为 true）。
    ///
    /// ⚠️ 这是必须让用户知道的事：RAW 画布上的像素来自相机内嵌的
    /// JPEG 预览，导出的文件**就是那个分辨率**，不是传感器全尺寸。
    /// 不提示的话，用户会以为"导出后画质变差了"。
    pub preview_resolution: bool,
}

/// 自由框选的裁剪区（**归一化坐标**，0~1，相对旋转后的图像）。
///
/// 与 `aspect` 互斥：`aspect === "free"` 时才可能带这个字段，
/// 其余比例走中心对齐的比例裁剪。见 [`save_impl`] 的分支。
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropRectDto {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 把当前画布调整（90° 旋转 + 裁剪）写回磁盘。
///
/// ## 两种模式，由 `target_path` 区分
/// | `target_path` | 行为 | 允许的源格式 |
/// |---|---|---|
/// | `None` | **覆盖原文件** | 仅 `can_overwrite_original`（JPEG/PNG/BMP）|
/// | `Some(路径)` | **另存为**新文件 | 任何能解码的格式（含 RAW / HEIC / WebP）|
///
/// 裁剪有两种表达，**优先看 `crop`**：
/// - `crop` 非空 → 按用户框选的归一化矩形裁（「自由」模式）；
/// - 否则看 `aspect` → `1:1`/`3:4`/`4:3`/`16:9` 走居中比例裁剪；
/// - 都没有 → 只做旋转。
///
/// ## 为什么 RAW 只能另存为
/// RAW 画布上的像素是**相机内嵌的 JPEG 预览**（见 `RawPipeline::extract_preview`），
/// 不是传感器数据。若允许覆盖，等于用 8bit sRGB 预览替换掉 14bit Bayer 数据，
/// **画质永久降级且不可逆**。所以 `target_path = None` 时 RAW 直接被拒。
/// 想要全保真的输出只能等 S8 接LibRaw 走真开发+ 写 DNG。
///
/// **注意**：前端的角度微调（±45°）不参与写回，见模块文档。
#[tauri::command]
pub fn save_edited_image(
    state: State<'_, AppState>,
    path: String,
    rotation_steps: i32,
    aspect: String,
    crop: Option<CropRectDto>,
    // 另存为的目标路径。`None` = 覆盖原文件。
    // （参数上不能用 `///` doc 注释 —— `#[doc]` 不在函数参数的合法属性白名单内）
    target_path: Option<String>,
) -> std::result::Result<SaveResultDto, ErrorDto> {
    save_impl(
        state.registry(),
        &PathBuf::from(&path),
        rotation_steps,
        &aspect,
        crop,
        target_path.as_deref().map(PathBuf::from),
    )
    .map_err(ErrorDto::from)
}

/// 与命令解耦的实现，便于单测（不需要构造 Tauri `State`）。
fn save_impl(
    registry: &pv_decode::DecoderRegistry,
    path: &std::path::Path,
    rotation_steps: i32,
    aspect: &str,
    crop: Option<CropRectDto>,
    target_path: Option<PathBuf>,
) -> Result<SaveResultDto> {
    // ── 1. 探测格式 ──
    let info = registry.probe(path)?;
    let src_format = info.format;
    let exported = target_path.is_some();

    // ── 2. 判定写到哪里、定成什么格式 ──
    //覆盖与另存为的判据不同：RAW 只能另存为，而另存为的**目标格式
    // 由用户选的扩展名决定**（源文件是 .cr3，导出成 .jpg 就得编 JPEG）。
    let (dest_path, dest_format) = match &target_path {
        Some(t) => {
            let ext = t.extension().and_then(|e| e.to_str()).unwrap_or("");
            let f = pv_decode::format_from_extension(ext).ok_or_else(|| {
                PvError::NotWritable(format!(
                    "无法从扩展名「{ext}」判断要导出的格式 —— 请用 .jpg / .jpeg / .png / .bmp 结尾"
                ))
            })?;
            (t.clone(), f)
        }
        None => {
            if !pv_decode::can_overwrite_original(src_format) {
                return Err(PvError::NotWritable(pv_decode::not_writable_reason(src_format)));
            }
            (path.to_path_buf(), src_format)
        }
    };
    // 另存为时目标格式也得能编码（扩展名已保证，这里是防御性断言）
    if !pv_decode::can_encode(dest_format) {
        return Err(PvError::NotWritable(pv_decode::not_writable_reason(dest_format)));
    }

    let steps = rotation_steps.rem_euclid(4);

    // ── 3. 解码原尺寸像素（Export = 不缩限）──
    // 代理图（预览）写回会把原图降级成 2048px，必须用 Export 拿全尺寸。
    // ⚠️ RAW 也走这里：它返回的是**内嵌预览的全尺寸**（不是 2048 代理图），
    // 但仍是相机渲染的那份 JPEG，不是传感器数据。
    let src = registry.decode(path, DecodeTarget::new(DecodePurpose::Export))?;

    // ── 4. 几何变换（纯内存，失败也不影响任何文件）──
    let (out, cropped) = apply_edits(&src, steps, aspect, crop)?;

    // ── 5. 搬运源文件的拍摄参数（EXIF）──
    //
    // 用户 2026-10-11 要求：「另存为的时候带上 exif 信息」。
    //
    // ⚠️ 搬运前必须 `align`：
    //   ① **用户旋转过 → `Orientation` 置 1**。画布旋转是**真的重采样像素**，
    //      源 EXIF 的方向标记若原样带走，别的软件会在已转正的画面上再转一次。
    //   ② **没旋转 → 方向标记原样保留**（像素一个字节没变，改它等于凭空丢失
    //      「这张竖拍照片要转 90°」的信息）。
    //   ③ 两种情况都切断 IFD1 缩略图（那是编辑前的旧画面，且占掉大半体积）。
    let exif = pv_meta::read_exif_raw(path).map(|mut t| {
        if pv_meta::align_exif_with_pixels(&mut t, steps)
            == pv_meta::ExifRewriteOutcome::Unparsable
        {
            tracing::debug!(
                target: "photovault::save",
                "EXIF 结构无法解析，原样搬运（方向标记可能与像素不一致）"
            );
        }
        t
    });

    // ── 6. 编码 + 原子写盘 ──
    pv_decode::write_rgba_atomic_with_exif(&out, dest_format, &dest_path, exif.as_deref())?;

    let file_size = std::fs::metadata(&dest_path)
        .map(|m| m.len())
        .unwrap_or(0);

    tracing::info!(
        target: "photovault::save",
        src = %path.display(),
        dest = %dest_path.display(),
        exported,
        src_format = %src_format.display_name(),
        dest_format = %dest_format.display_name(),
        rotation_steps = steps,
        aspect,
        free_crop = crop.is_some(),
        exif = exif.as_ref().map(|e| e.len()).unwrap_or(0),
        from = %format!("{}x{}", src.width, src.height),
        to = %format!("{}x{}", out.width, out.height),
        bytes = file_size,
        "画布调整已写盘"
    );

    Ok(SaveResultDto {
        width: out.width,
        height: out.height,
        file_size,
        rotation_applied: steps,
        cropped,
        saved_path: dest_path.to_string_lossy().into_owned(),
        exported,
        preview_resolution: src_format.is_raw(),
    })
}

/// 把「90° 旋转 + 裁剪」这套几何变换应用到一张已解码的图像上。
///
/// **写回（[`save_edited_image`]）与打印共用这一份实现** —— 两边各写一遍
/// 迟早会分叉，而分叉的表现是「保存出来的图和打印出来的图不一样」，
/// 属于最难查的那类不一致。
///
/// 顺序固定为**先旋转再裁剪**：归一化裁剪坐标是相对「旋转后的图像」定义的，
/// 前端 `cropToScreenRect` 也是这个顺序。改动顺序会让自由裁剪框整体错位。
///
/// 返回 `(结果图, 是否发生了裁剪)`。
pub(crate) fn apply_edits(
    src: &RgbaImage,
    rotation_steps: i32,
    aspect: &str,
    crop: Option<CropRectDto>,
) -> Result<(RgbaImage, bool)> {
    let rotated = rotate90_steps(src, rotation_steps.rem_euclid(4));
    match crop {
        Some(c) => Ok((
            pv_decode::crop_normalized(&rotated, c.x, c.y, c.w, c.h)?,
            true,
        )),
        None => match aspect_ratio(aspect) {
            Some(r) => Ok((crop_to_aspect(&rotated, r)?, true)),
            None => Ok((rotated, false)),
        },
    }
}

/// 比例标识 → 宽高比数值。`free` 或无法识别时返回 `None`（表示不裁剪）。
///
/// ⚠️ 必须与前端 `ASPECT_VALUES`（`hooks/useImageView.ts`）**逐项对齐**：
/// 这里少一项而前端有，用户选了那个比例保存时会被静默忽略（不裁剪），
/// 而不是报错 —— 所以改动任一侧都要同步另一侧。
fn aspect_ratio(aspect: &str) -> Option<f64> {
    match aspect {
        "1:1" => Some(1.0),
        "3:4" => Some(3.0 / 4.0),
        "4:3" => Some(4.0 / 3.0),
        "16:9" => Some(16.0 / 9.0),
        "9:16" => Some(9.0 / 16.0),
        "2.35:1" => Some(2.35),
        "2:3" => Some(2.0 / 3.0),
        // "free" 与任何未识别值都不裁剪
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_decode::{BitmapBackend, DecoderRegistry};

    fn registry() -> DecoderRegistry {
        let mut reg = DecoderRegistry::new();
        reg.register(Box::new(BitmapBackend));
        // ⚠️ RAW 走的是**独立管线**，不装的话 `probe`/`decode` 会直接返回
        // 「RAW 解码器未接入」。这条曾经漏装，导致下面两个 RAW 另存为测试
        // 报的是「未接入」而不是真正要测的行为 —— 看起来像功能坏了。
        // 与 `state.rs` 的注册方式保持一致。
        reg.set_raw_pipeline(Box::new(pv_decode::EmbeddedPreviewPipeline::new()));
        reg
    }

    /// 用已有的真实样本复制一份到临时目录（绝不碰 test-image 原文件）。
    fn copy_sample(name: &str, suffix: &str) -> Option<PathBuf> {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-image")
            .join(name);
        if !src.is_file() || std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0) <= 4096 {
            return None;
        }
        let dir = std::env::temp_dir().join("pv-save-test");
        std::fs::create_dir_all(&dir).ok()?;
        let dst = dir.join(format!("{}-{suffix}", std::process::id()));
        std::fs::copy(&src, &dst).ok()?;
        Some(dst)
    }

    /// ⭐ 用户 2026-10-11 需求：「另存为的时候带上 exif 信息」。
    ///
    /// 直接调**产品函数** `save_impl`（不是手工串链路），确保真接上了。
    #[test]
    fn 另存为后拍摄参数仍在() {
        let Some(src) = copy_sample("DSC9515.JPG", "exif-src.jpg") else { return };
        let before = pv_meta::read_exif(&src).expect("样本应有 EXIF");
        let dst = src.with_file_name(format!("{}-exif-out.jpg", std::process::id()));
        let _ = std::fs::remove_file(&dst);

        let r = save_impl(&registry(), &src, 0, "free", None, Some(dst.clone())).unwrap();
        assert!(r.exported, "指定了目标路径即为另存为");

        let after = pv_meta::read_exif(&dst).expect("另存为的输出应当带 EXIF");
        assert_eq!(after.make, before.make, "厂商");
        assert_eq!(after.model, before.model, "型号");
        assert_eq!(after.iso, before.iso, "ISO");
        assert_eq!(after.exposure_time, before.exposure_time, "快门");
        assert_eq!(after.f_number, before.f_number, "光圈");
        assert_eq!(after.date_time_original, before.date_time_original, "拍摄时间");
        assert_eq!(after.gps, before.gps, "GPS");
        // 没旋转 → 方向信息原样保留（改了等于数据损失）
        assert_eq!(after.orientation, before.orientation, "未旋转时方向不该变");

        let _ = std::fs::remove_file(&dst);
    }

    /// 旋转过再另存：方向标记必须变成「正常」。
    ///
    /// 否则输出文件在别的软件里会被**再转一次** —— 照片看起来横了，
    /// 而用户明明在画布上把它转正了。
    #[test]
    fn 旋转后另存方向标记为正常() {
        let Some(src) = copy_sample("DSC9515.JPG", "exif-rot-src.jpg") else { return };
        let dst = src.with_file_name(format!("{}-exif-rot.jpg", std::process::id()));
        let _ = std::fs::remove_file(&dst);

        save_impl(&registry(), &src, 1, "free", None, Some(dst.clone())).unwrap();

        let after = pv_meta::read_exif(&dst).expect("应带 EXIF");
        assert_eq!(after.orientation.as_deref(), Some("正常"));
        let _ = std::fs::remove_file(&dst);
    }

    /// **覆盖原文件**也要带 EXIF —— 不是只管「另存为」。
    ///
    /// 覆盖路径若把元数据丢了，用户拍的原片（机型/时间/GPS）就没了。
    #[test]
    fn 覆盖保存也带拍摄参数() {
        let Some(src) = copy_sample("DSC9515.JPG", "exif-inplace.jpg") else { return };
        let before = pv_meta::read_exif(&src).expect("样本应有 EXIF");

        // target_path = None → 覆盖原文件（这里操作的是副本，不动 test-image）
        save_impl(&registry(), &src, 0, "free", None, None).unwrap();

        let after = pv_meta::read_exif(&src).expect("覆盖后仍应带 EXIF");
        assert_eq!(after.model, before.model);
        assert_eq!(after.iso, before.iso);
        assert_eq!(after.date_time_original, before.date_time_original);
        let _ = std::fs::remove_file(&src);
    }

    /// 源文件没有 EXIF（截图等）→ 输出也不该凭空多出。
    ///
    /// 源图**现场生成**而不是拿样本：图库里那张 PNG 恰好带 `eXIf` 段，
    /// 用它会让测试的前提（"源没有元数据"）不成立。
    #[test]
    fn 无EXIF的源另存后仍无EXIF() {
        let dir = std::env::temp_dir().join("pv-save-test");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(format!("{}-noexif-src.png", std::process::id()));
        let dst = dir.join(format!("{}-noexif-out.png", std::process::id()));
        let _ = std::fs::remove_file(&dst);

        // 只写像素，不带任何元数据段
        let mut img = pv_decode::RgbaImage::new(32, 24).unwrap();
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = (i % 251) as u8;
        }
        pv_decode::write_rgba_atomic(&img, pv_core::format::ImageFormat::Png, &src).unwrap();
        assert!(pv_meta::read_exif_raw(&src).is_none(), "源图本身就不该有元数据");

        save_impl(&registry(), &src, 0, "free", None, Some(dst.clone())).unwrap();
        assert!(pv_meta::read_exif(&dst).is_none(), "不该无中生有");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }

    /// ⭐ 裁剪坐标是相对**旋转后的图**（先旋转再裁剪），不是原图。
    ///
    /// 与前端 `useImageView::rectToCrop` / `aspectCenterCrop` 是**同一个约定** ——
    /// 两边一旦不一致，表现就是「框里看到的 ≠ 存出来的」：2026-10-11 修过一次
    /// （前端按原图归一化，与后端差一次 90° 的宽高互换，于是 90° 下
    /// 「预置框看着是正方形、裁出来是 0.44 的扁图」，拖框时比例还会跳变）。
    #[test]
    fn 裁剪坐标相对旋转后的图而非原图() {
        // 4×2：只有左上角 (0,0) 是红色，其余黑 —— 转 90° 后红点会跑到
        // 旋转后图（2×4）的右上角 (1,0)。两个坐标系下位置完全不同，
        // 所以「按哪个坐标系裁」是**可观测**的。
        let mut data = vec![0u8; 4 * 2 * 4];
        data[3] = 255; // 第一个像素的 A 通道
        let src = RgbaImage {
            width: 4,
            height: 2,
            data,
        };

        // 顺时针 90° + 取旋转后图的「上半」（归一化 y 0..0.5、整宽）
        let crop = CropRectDto {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 0.5,
        };
        let (out, cropped) = apply_edits(&src, 1, "free", Some(crop)).unwrap();
        assert!(cropped);
        // 旋转后是 2×4；上半 = 2×2
        assert_eq!((out.width, out.height), (2, 2), "应先旋转再裁剪");

        let at = |x: u32, y: u32| -> u8 {
            out.data[((y * out.width + x) * 4 + 3) as usize]
        };
        assert_eq!(at(1, 0), 255, "红点应在旋转后图的右上角");
        assert_eq!(at(0, 0), 0);

        // 反证：若误把 crop 当「原图的归一化坐标」裁（先裁后转），
        // 结果会是 4×1（原图上半只有 1 行）—— 尺寸就不一样。
        let (wrong_order, _) = pv_decode::crop_normalized(&src, 0.0, 0.0, 1.0, 0.5)
            .map(|c| (c, ()))
            .unwrap();
        assert_eq!(wrong_order.width, 4);
        assert_ne!(wrong_order.width, out.width, "两种坐标系必须可区分");
    }

    #[test]
    fn 比例标识解析正确() {        assert_eq!(aspect_ratio("1:1"), Some(1.0));
        assert!((aspect_ratio("16:9").unwrap() - 16.0 / 9.0).abs() < 1e-9);
        assert!((aspect_ratio("9:16").unwrap() - 9.0 / 16.0).abs() < 1e-9);
        assert!((aspect_ratio("2.35:1").unwrap() - 2.35).abs() < 1e-9);
        assert!((aspect_ratio("2:3").unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(aspect_ratio("free"), None);
        assert_eq!(aspect_ratio("乱写"), None, "未知值按不裁剪处理");
    }

    /// 前端 `ASPECT_VALUES` 的键必须与后端一一对应 ——
    /// 少一项不会报错，只会让保存时**静默不裁剪**，属于最难发现的一类 bug。
    #[test]
    fn 八个比例标识全部可识别() {
        let all = ["1:1", "3:4", "4:3", "16:9", "9:16", "2.35:1", "2:3"];
        for a in all {
            assert!(aspect_ratio(a).is_some(), "{a} 未被后端识别");
        }
        assert_eq!(all.len(), 7, "除 free 外交互面板共 7 个固定比例");
    }

    /// RAW **覆盖原文件**必须被拒绝，且原文件一个字节都不能变。
    ///
    /// 理由：RAW 的像素来自内嵌 JPEG 预览，覆盖 = 用预览替换传感器数据，
    /// 画质永久降级。这是方案 A（另存为）存在的根本原因。
    #[test]
    fn RAW覆盖原文件被拒绝且原文件不变() {
        let Some(p) = copy_sample("canon-cr3-sample.CR3", "raw.cr3") else {
            eprintln!("跳过：CR3 样本不可用");
            return;
        };
        let before = std::fs::read(&p).unwrap();

        let err = save_impl(&registry(), &p, 1, "free", None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("RAW"), "应说明是 RAW: {msg}");

        let after = std::fs::read(&p).unwrap();
        assert_eq!(before, after, "被拒绝的保存绝不能改动原文件");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 顺时针90度写回后宽高互换且文件可再解码() {
        let Some(p) = copy_sample("DSC9515.JPG", "rot90.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let reg = registry();
        let before = reg.probe(&p).unwrap();

        let res = save_impl(&reg, &p, 1, "free", None, None).unwrap();
        assert_eq!(res.rotation_applied, 1);
        assert!(!res.cropped);
        // 宽高互换
        assert_eq!(res.width, before.height);
        assert_eq!(res.height, before.width);

        // 写回的文件仍能被正常解码（说明编码产物合法）
        let after = reg.probe(&p).unwrap();
        assert_eq!((after.width, after.height), (res.width, res.height));
        assert!(reg.decode(&p, DecodeTarget::new(DecodePurpose::Preview)).is_ok());

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 逆时针90度两次等价于180度() {
        let Some(p) = copy_sample("DSC9515.JPG", "ccw.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let reg = registry();
        let before = reg.probe(&p).unwrap();

        save_impl(&reg, &p, -1, "free", None, None).unwrap();
        let mid = reg.probe(&p).unwrap();
        assert_eq!(mid.height, before.width, "逆时针 90° 后宽高互换");

        save_impl(&reg, &p, 3, "free", None, None).unwrap(); // 再逆时针 90°（= 顺时针 3 步）
        let after = reg.probe(&p).unwrap();
        assert_eq!(
            (after.width, after.height),
            (before.width, before.height),
            "两次逆时针 90° 应回到原始尺寸"
        );

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 裁剪为1比1后宽高相等() {
        let Some(p) = copy_sample("DSC9515.JPG", "crop11.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let reg = registry();

        let res = save_impl(&reg, &p, 0, "1:1", None, None).unwrap();
        assert!(res.cropped);
        assert_eq!(res.width, res.height, "1:1 裁剪后宽高必须相等");

        let after = reg.probe(&p).unwrap();
        assert_eq!((after.width, after.height), (res.width, res.height));

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 不存在的文件返回错误不panic() {
        let p = std::env::temp_dir().join("pv-绝对不存在-12345.jpg");
        let _ = std::fs::remove_file(&p);
        assert!(save_impl(&registry(), &p, 0, "free", None, None).is_err());
    }

    #[test]
    fn 自由框选裁剪按指定区域写回() {
        let Some(p) = copy_sample("DSC9515.JPG", "freecrop.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let reg = registry();
        let before = reg.probe(&p).unwrap();

        // 取左上四分之一。注意尺寸要按 round 算 —— 样本宽高可能是奇数
        // （5847），`w/2` 的整数除法会差 1 像素，不能用它当期望值。
        let crop = CropRectDto {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        };
        let res = save_impl(&reg, &p, 0, "free", Some(crop), None).unwrap();
        assert!(res.cropped, "框选裁剪后 cropped 应为 true");
        assert_eq!(res.width, (before.width as f64 * 0.5).round() as u32);
        assert_eq!(res.height, (before.height as f64 * 0.5).round() as u32);

        // 写回的文件仍可解码
        let after = reg.probe(&p).unwrap();
        assert_eq!((after.width, after.height), (res.width, res.height));

        let _ = std::fs::remove_file(&p);
    }

    /// 自由框选**优先于**比例：两个都传时按框选裁。
    /// 前端理论上不会同时传，但后端必须有确定的语义，避免出现
    /// 「界面上框选着、保存却按 1:1」这种不可解释的行为。
    #[test]
    fn 自由框选优先于比例参数() {
        let Some(p) = copy_sample("DSC9515.JPG", "crop-priority.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let reg = registry();
        let before = reg.probe(&p).unwrap();

        // 传了 1:1（会裁成正方形），但同时给了 1/4 的框选
        let crop = CropRectDto {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        };
        let res = save_impl(&reg, &p, 0, "1:1", Some(crop), None).unwrap();
        assert_ne!(res.width, res.height, "应按框选裁，而非按 1:1 裁成正方形");
        assert_eq!(res.width, (before.width as f64 * 0.5).round() as u32);
        assert_eq!(res.height, (before.height as f64 * 0.5).round() as u32);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 框选区域非法时原文件不变() {
        let Some(p) = copy_sample("DSC9515.JPG", "crop-invalid.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let before = std::fs::read(&p).unwrap();

        let crop = CropRectDto {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.5,
        };
        assert!(save_impl(&registry(), &p, 0, "free", Some(crop), None).is_err());

        let after = std::fs::read(&p).unwrap();
        assert_eq!(before, after, "非法框选绝不能改动原文件");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 框选字段键名与前端一致() {
        // 前端 `invoke('save_edited_image', { crop: { x, y, w, h } })`，
        // 键名对不上会静默变成 undefined → 裁剪区消失。这里钉死契约。
        let dto: CropRectDto = serde_json::from_value(serde_json::json!({
            "x": 0.1, "y": 0.2, "w": 0.3, "h": 0.4
        }))
        .expect("前端传来的 camelCase 字段应能反序列化");
        assert!((dto.x - 0.1).abs() < 1e-9);
        assert!((dto.h - 0.4).abs() < 1e-9);
    }

    #[test]
    fn 保存结果DTO键名与前端一致() {
        let dto = SaveResultDto {
            width: 100,
            height: 50,
            file_size: 2048,
            rotation_applied: 1,
            cropped: true,
            saved_path: "C:/x.jpg".into(),
            exported: false,
            preview_resolution: false,
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in [
            "width",
            "height",
            "fileSize",
            "rotationApplied",
            "cropped",
            "savedPath",
            "exported",
            "previewResolution",
        ] {
            assert!(v.get(key).is_some(), "SaveResultDto 缺少字段 {key}");
        }
    }

    // ────────────────────────────方案 A：另存为 ────────────────────────────

    /// 临时输出目录（另存为的目标都放这儿，绝不碰 test-image）。
    fn out_dir() -> PathBuf {
        let d = std::env::temp_dir().join("pv-export-test");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// **RAW 可以另存为**，且**原文件一个字节都不变** —— 这是方案 A 的核心承诺。
    #[test]
    fn RAW另存为成功且原文件不变() {
        let Some(p) = copy_sample("canon-cr3-sample.CR3", "raw-export.cr3") else {
            eprintln!("跳过：CR3 样本不可用");
            return;
        };
        let before = std::fs::read(&p).unwrap();
        let dest = out_dir().join(format!("{}-cr3.jpg", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        let res = save_impl(&registry(), &p, 0, "free", None, Some(dest.clone())).unwrap();

        assert!(res.exported, "另存为时 exported 应为 true");
        assert_eq!(res.saved_path, dest.to_string_lossy());
        assert!(
            res.preview_resolution,
            "RAW 导出必须标记为「预览分辨率」，否则前端不会提示用户画质已降级"
        );
        assert!(dest.is_file(), "目标文件应已写出");

        // 核心断言：原 RAW 完好无损
        assert_eq!(
            std::fs::read(&p).unwrap(),
            before,
            "另存为绝不能改动原 RAW —— 这是整个方案 A 存在的理由"
        );

        // 导出的 JPEG 必须真的可解码（别写出一堆坏字节）
        let reg = registry();
        let out = reg.probe(&dest).unwrap();
        assert_eq!((out.width, out.height), (res.width, res.height));
        assert!(reg
            .decode(&dest, DecodeTarget::new(DecodePurpose::Preview))
            .is_ok());

        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&dest);
    }

    /// 另存为时**目标格式由扩展名决定**，不是由源格式决定。
    /// .cr3 源 → .png 目标，必须真的编成 PNG。
    #[test]
    fn 另存为按目标扩展名决定格式() {
        let Some(p) = copy_sample("canon-cr3-sample.CR3", "ext.cr3") else {
            eprintln!("跳过：CR3 样本不可用");
            return;
        };
        let dest = out_dir().join(format!("{}-from-raw.png", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        let res = save_impl(&registry(), &p, 0, "free", None, Some(dest.clone())).unwrap();
        assert_eq!((res.width, res.height), (res.width, res.height));

        // 魔数必须是 PNG（\x89PNG），不能是 JPEG —— 源是 CR3，很容易搞错
        let bytes = std::fs::read(&dest).unwrap();
        assert!(
            bytes.starts_with(&[0x89, b'P', b'N', b'G']),
            "目标扩展名是 .png，产物必须是 PNG 魔数，实际前4 字节={:?}",
            &bytes[..4.min(bytes.len())]
        );

        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&dest);
    }

    /// 认不出的扩展名必须**明确报错**，不能静默退回 JPEG/PNG。
    /// 静默退回是最糟的：用户选了 .webp 却拿到 JPEG 内容，还会以为坏了。
    #[test]
    fn 无法识别的目标扩展名被拒绝且不产出文件() {
        let Some(p) = copy_sample("DSC9515.JPG", "badext.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let dest = out_dir().join(format!("{}-bad.webp", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        let err = save_impl(&registry(), &p, 0, "free", None, Some(dest.clone())).unwrap_err();
        assert!(
            err.to_string().contains("webp"),
            "错误信息要点名用户选的扩展名: {err}"
        );
        assert!(!dest.exists(), "被拒绝的另存为不能留下半截文件");

        let _ = std::fs::remove_file(&p);
    }

    /// 覆盖保存时 `exported` 必须为 false，且 `savedPath` 就是原路径。
    #[test]
    fn 覆盖保存标记为非导出() {
        let Some(p) = copy_sample("DSC9515.JPG", "cov.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let res = save_impl(&registry(), &p, 0, "free", None, None).unwrap();
        assert!(!res.exported);
        assert_eq!(res.saved_path, p.to_string_lossy());
        assert!(
            !res.preview_resolution,
            "普通 JPEG 不是预览分辨率（那条只针对 RAW 内嵌预览）"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// JPEG 也可以另存为 —— 「另存为」不是 RAW 专属功能。
    #[test]
    fn JPEG另存为PNG有效() {
        let Some(p) = copy_sample("DSC9515.JPG", "jpg2png.jpg") else {
            eprintln!("跳过：JPEG 样本不可用");
            return;
        };
        let dest = out_dir().join(format!("{}-converted.png", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        let res = save_impl(&registry(), &p, 0, "free", None, Some(dest.clone())).unwrap();
        assert!(res.exported && !res.preview_resolution);
        assert!(std::fs::read(&dest).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));

        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&dest);
    }
}
