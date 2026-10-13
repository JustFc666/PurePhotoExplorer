//! 图像相关命令：`probe_image` / `decode_image` / `decode_animation`。
//!
//! 后两个返回 **`tauri::ipc::Response`（裸字节）**，不再 base64 ——
//! 前提是 `tauri.conf.json` 的 CSP 放行了 `ipc: http://ipc.localhost`
//! （详见 `commands/pixel.rs` 模块头部的说明）。前端 `ipc/protocol.ts::toBytes()`
//! 直接拿到 `ArrayBuffer`，零拷贝切视图。

use std::path::PathBuf;

use pv_decode::{DecodePurpose, DecodeTarget};
use tauri::ipc::Response;
use tauri::State;

use super::dto::{io_err, ErrorDto, FileVersionDto, ImageInfoDto, RawInfoDto};
use super::pixel::{pack_animation, pack_pixels, AnimPack};
use crate::state::AppState;

/// 前端传的 `purpose` 字符串 → 解码目标。
///
/// `decode_image` 与 `decode_animation` 共用，避免两处 `match` 写岔
/// （曾经只在一处加档位、另一处漏掉，表现为「动图清晰度与静图不一致」这类怪现象）。
fn decode_target_for(purpose: &str) -> DecodeTarget {
    match purpose {
        "thumbnail" => DecodeTarget::new(DecodePurpose::Thumbnail),
        "edit" => DecodeTarget::new(DecodePurpose::Edit),
        "export" => DecodeTarget::new(DecodePurpose::Export),
        // 默认按预览处理（S1 主要场景）
        _ => DecodeTarget::new(DecodePurpose::Preview),
    }
}

/// 探测图像信息（含 EXIF 拍摄参数）
#[tauri::command]
pub fn probe_image(
    state: State<'_, AppState>,
    path: String,
) -> std::result::Result<ImageInfoDto, ErrorDto> {
    let p = PathBuf::from(&path);
    let info = state.registry().probe(&p).map_err(ErrorDto::from)?;

    let meta = std::fs::metadata(&p).map_err(|e| io_err(&p, e))?;
    let modified_at = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // EXIF 是"锦上添花"：读不到就是 None，绝不影响主流程（错误已在 pv-meta 内部吞掉并记日志）
    let exif = pv_meta::read_exif(&p);

    // GPS 坐标 → 最近的城市。走内置离线库（6.4 万条城市，实测 < 1 ms），
    // **不联网** —— 照片位置属于隐私数据，不该为显示一个地名发出去。
    // 没 GPS 就完全跳过，不做任何多余工作。
    let place = exif
        .as_ref()
        .and_then(|e| e.gps.as_ref())
        .and_then(pv_meta::GpsData::lat_lon)
        .and_then(|(lat, lon)| pv_geo::nearest_place(lat, lon));

    Ok(ImageInfoDto {
        file_name: p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path,
        format: format!("{:?}", info.format).to_lowercase(),
        format_label: info.format.display_name().to_string(),
        width: info.width,
        height: info.height,
        file_size: meta.len(),
        is_raw: info.is_raw(),
        modified_at,
        exif,
        raw: info.raw.as_ref().map(raw_info_dto),
        place,
    })
}

/// `RawMetadata` → 前端 DTO（只挑前端要展示的字段）。
fn raw_info_dto(m: &pv_core::raw::RawMetadata) -> RawInfoDto {
    RawInfoDto {
        raw_format: m.raw_format.clone(),
        has_embedded_preview: m.has_embedded_preview,
        bit_depth: m.bit_depth,
        color_space: color_space_label(m.color_space).to_string(),
    }
}

/// 色彩空间的中文标签（放 Rust 侧写一处，前端只显示）
fn color_space_label(cs: pv_core::raw::RawColorSpace) -> &'static str {
    use pv_core::raw::RawColorSpace as CS;
    match cs {
        CS::Unknown => "未知",
        CS::Srgb => "sRGB",
        CS::AdobeRgb => "Adobe RGB",
        CS::ProPhotoRgb => "ProPhoto RGB",
        CS::CameraNative => "相机原生",
    }
}

/// 解码图像，返回 **PVI1 打包的裸字节**。
///
/// 前端 `invoke` 侧按响应体的 `content-type` 分派：裸字节是
/// `application/octet-stream` → `response.arrayBuffer()`，**不经过
/// base64、也不经过 JSON**。这是 2026-10-09 修好 CSP 之后才成立的
/// 快路径（此前 CSP 漏了 `connect-src ipc: http://ipc.localhost`，
/// fetch 被拦 → 回退到 postMessage/eval 通道 → 大图明显卡顿）。
///
/// `purpose` 由前端传：`thumbnail` / `preview` / `edit` / `export`
#[tauri::command]
pub fn decode_image(
    state: State<'_, AppState>,
    path: String,
    purpose: String,
) -> std::result::Result<Response, ErrorDto> {
    let p = PathBuf::from(&path);
    let target = decode_target_for(&purpose);

    let img = state.registry().decode(&p, target).map_err(ErrorDto::from)?;
    Ok(Response::new(pack_pixels(&img)))
}

/// 取文件版本（`stat`，**不解码**）。
///
/// 前端拿它做**解码缓存的校验**：缓存条目记住「解码时那个文件的版本」，
/// 命中时先比对当前版本，不一致就重新解码。
///
/// 没有这一步时，缓存键只有路径 —— 文件被覆盖后（别的程序改的、
/// 或本程序"另存为"覆盖了同目录已有文件）界面会一直显示**旧像素**，
/// 用户以为"没保存成功"。2026-10-11 实测复现：预取过的图被覆盖成
/// 另一种颜色，翻过去仍是旧颜色。
#[tauri::command]
pub fn file_version(path: String) -> std::result::Result<FileVersionDto, ErrorDto> {
    let p = PathBuf::from(&path);
    let meta = std::fs::metadata(&p).map_err(|e| io_err(&p, e))?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(FileVersionDto {
        modified_ms,
        size: meta.len(),
    })
}

/// 按「字节预算 + 时长下限」算出本块应覆盖的帧区间右端点 `to`（左端是 `from`）。
///
/// 抽成纯函数是为了能单测 —— 这段切分逻辑一旦写错，表现是
/// 「动画播到一半卡一下」或「首批还是几十 MB」，很难从界面上定位。
///
/// 规则：**至少一帧**；之后一旦「字节够了」**且**「时长够了」就收尾。
/// 任一位 0 表示该维度不设限；两者都为 0 表示取到结尾。
pub(crate) fn plan_chunk_end(
    delays: &[u32],
    from: usize,
    frame_len: usize,
    max_bytes: usize,
    min_duration_ms: u32,
) -> usize {
    let total = delays.len();
    if from >= total {
        return total;
    }
    if max_bytes == 0 && min_duration_ms == 0 {
        return total;
    }

    let min_dur = u64::from(min_duration_ms);
    let mut to = from;
    let (mut bytes, mut dur) = (0usize, 0u64);
    while to < total {
        // 已经有至少一帧，且两个预算都满足了 → 到此为止
        let bytes_ok = max_bytes == 0 || bytes >= max_bytes;
        let dur_ok = min_duration_ms == 0 || dur >= min_dur;
        if to > from && bytes_ok && dur_ok {
            break;
        }
        bytes += frame_len;
        dur += u64::from(delays[to]);
        to += 1;
    }
    to
}

/// 解码**动图**（GIF）的一个**帧区间**，返回 **PVA1 打包的裸字节**。
///
/// ## 为什么是「区间」而不是「全部」（2026-10-09）
/// 这个 GIF 实测 299 帧 / 250×250 → 71MB 裸字节，**光传输就要 0.75–1.2 秒**
/// （实测约 95MB/s）。一次性全传，用户就得对着空白干等一秒多。
///
/// 所以改成两段：
/// 1. `start=0` + 字节/时长预算 → 头一批帧（实测 **167ms** 到位），画面立刻动起来；
/// 2. 剩下的在后台用 `start=上一块的末尾, max_bytes=0, min_duration_ms=0` 一次取完，
///    到达后接在播放列表尾部 —— 此时播放才消耗掉几帧，完全接得上。
///
/// 头部带 `start`/`total`，前端据此知道该往哪儿拼、还差多少（见
/// `commands/pixel.rs` 的 PVA1 协议）。
///
/// ## 两个预算为什么都要（不能只按字节切）
/// 首批必须**足够播到第二批到达**，否则播放会提前"播完"然后跳回开头。
/// 需要多少帧取决于**帧延时**：延时 100ms 时 33 帧能播 3.3 秒（绰绰有余），
/// 延时 20ms 时同样 33 帧只够 0.66 秒（会露馅）。而帧延时只有解码后才知道，
/// 所以判断放在 Rust 侧：
/// - `max_bytes` —— 字节上限，决定传输耗时；
/// - `min_duration_ms` —— 时长下限，保证「够播」。
///
/// 取「**两者都满足才停**」，于是长延时的动图只传几 MB 就够（快），
/// 短延时的动图自动多传一些（稳）。**至少会给一帧**。
/// 两个值同时为 0 表示不设限（取到结尾）。
///
/// - `start`：起始帧序号（超出总帧数则返回空块）
/// - `max_bytes`：本块像素的字节预算（0 = 不限）
/// - `min_duration_ms`：本块应至少覆盖的播放时长（0 = 不限）
///
/// 非动图（PNG/JPEG）调用它会返回「不含可解码的帧」的错误 —— 前端只在
/// 探测到 GIF 时才调它。
#[tauri::command]
pub fn decode_animation(
    path: String,
    purpose: String,
    start: Option<u32>,
    max_bytes: Option<usize>,
    min_duration_ms: Option<u32>,
) -> std::result::Result<Response, ErrorDto> {
    let p = PathBuf::from(&path);
    let target = decode_target_for(&purpose);

    // 按文件头分发（GIF / WebP）：命令层不必关心是哪种动图格式
    let anim = pv_decode::decode_animation(&p, target).map_err(ErrorDto::from)?;

    let total = anim.frames.len();
    let from = (start.unwrap_or(0) as usize).min(total);
    let frame_len = anim.width as usize * anim.height as usize * 4;

    let delays: Vec<u32> = anim.frames.iter().map(|f| f.delay_ms).collect();
    let to = plan_chunk_end(
        &delays,
        from,
        frame_len,
        max_bytes.unwrap_or(0),
        min_duration_ms.unwrap_or(0),
    );

    let slice = &anim.frames[from..to];
    Ok(Response::new(pack_animation(AnimPack {
        width: anim.width,
        height: anim.height,
        loop_count: anim.loop_count,
        truncated: anim.truncated,
        start_index: from as u32,
        total_count: total as u32,
        frames: slice
            .iter()
            .map(|f| (f.delay_ms, f.image.data.as_slice()))
            .collect(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_core::raw::{RawColorSpace, RawMetadata};

    /// 找一个可用的真实样本（排除 Git LFS 指针小文件）。
    fn sample(name: &str) -> Option<PathBuf> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-image")
            .join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    // ─────────────── 分块策略（plan_chunk_end）───────────────

    /// 造 n 帧、每帧延时 d 毫秒的延时表
    fn delays(n: usize, d: u32) -> Vec<u32> {
        vec![d; n]
    }

    #[test]
    fn 分块_两个预算都为0时取到结尾() {
        assert_eq!(plan_chunk_end(&delays(100, 50), 0, 1000, 0, 0), 100);
        assert_eq!(plan_chunk_end(&delays(100, 50), 40, 1000, 0, 0), 100);
    }

    #[test]
    fn 分块_只有字节预算时按字节切() {
        // 每帧 1000B，预算 8000B → 8 帧
        assert_eq!(plan_chunk_end(&delays(100, 100), 0, 1000, 8000, 0), 8);
    }

    #[test]
    fn 分块_只有时长下限时按时长切() {
        // 50ms × 40 帧 = 2000ms ≥ 1800ms
        assert_eq!(plan_chunk_end(&delays(100, 50), 0, 1000, 0, 1800), 36);
    }

    /// 核心：短延时的动图会被**时长下限**顶上去，而不是被字节预算切得太短。
    #[test]
    fn 分块_时长下限能顶破字节预算() {
        let d = delays(299, 20); // 20ms/帧
        let frame_len = 250 * 250 * 4; // 250KB
        let to = plan_chunk_end(&d, 0, frame_len, 8 * 1024 * 1024, 3000);
        let frames = to;
        // 8MB / 250KB = 33 帧（只够 0.66 秒）→ 必须被顶到 ≥3 秒
        assert!(
            frames >= 150,
            "3 秒下限应至少给出 150 帧（20ms/帧），实际 {frames}"
        );
        let dur_ms: u64 = d[..to].iter().map(|x| u64::from(*x)).sum();
        assert!(dur_ms >= 3000, "本块时长应 ≥ 3000ms，实际 {dur_ms}ms");
    }

    /// 长延时的动图不该被时长下限拖着多传 —— 两个预算几乎同时满足就收尾。
    #[test]
    fn 分块_长延时时两个预算几乎同时满足() {
        let d = delays(299, 100); // 100ms/帧
        let frame_len = 250 * 250 * 4; // 250KB/帧
        let to = plan_chunk_end(&d, 0, frame_len, 8 * 1024 * 1024, 3000);
        // 250KB×34 = 8.5MB ≥ 8MB；34×100ms = 3.4s ≥ 3s → 第 34 帧同时满足两者
        assert_eq!(to, 34);
        assert!(to * frame_len >= 8 * 1024 * 1024, "应已满足字节预算");
        assert!(to as u64 * 100 >= 3000, "应已满足时长下限");
    }

    /// 单帧就超预算时也必须给出它 —— 否则超大动图永远拿不到数据。
    #[test]
    fn 分块_至少给一帧() {
        let to = plan_chunk_end(&delays(10, 100), 0, 100 * 1024 * 1024, 1024, 0);
        assert_eq!(to, 1, "单帧 100MB 超预算，但至少得给一帧");
    }

    /// 时长下限永远达不到时（整段都不够长）→ 取到结尾，而不是死循环。
    #[test]
    fn 分块_时长下限无法满足时取到结尾() {
        let to = plan_chunk_end(&delays(10, 100), 0, 1024, 1024, 60_000);
        assert_eq!(to, 10, "整段才 1 秒，够不到 60 秒下限，应把能给的都给完");
    }

    #[test]
    fn 分块_起始越界返回末尾() {
        assert_eq!(plan_chunk_end(&delays(10, 100), 10, 1024, 4096, 0), 10);
        assert_eq!(plan_chunk_end(&delays(10, 100), 999, 1024, 4096, 0), 10);
    }

    #[test]
    fn 分块_从中间起切也只给一帧() {
        // from=9 时只剩 1 帧
        assert_eq!(plan_chunk_end(&delays(10, 100), 9, 1024, 4096, 0), 10);
    }

    #[test]
    fn 分块_总帧数为0时返回0() {
        assert_eq!(plan_chunk_end(&[], 0, 1024, 4096, 0), 0);
    }

    /// 临时基准：动图解码 + 打包各阶段耗时
    /// （`cargo test -p photovault --lib --release bench_ -- --nocapture`）
    #[test]
    #[ignore = "基准测试，手动跑：cargo test -p photovault --lib --release -- --ignored bench_ --nocapture"]
    fn bench_动图解码与打包耗时() {
        use std::time::Instant;
        let Some(p) = sample("a746f3dcly1gnnxc9hraig206y06yx6s.gif") else {
            eprintln!("跳过：找不到 GIF 样本");
            return;
        };

        let t0 = Instant::now();
        let anim = pv_decode::decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
        let t_decode = t0.elapsed();

        let t1 = Instant::now();
        let blob = pack_animation(AnimPack {
            width: anim.width,
            height: anim.height,
            loop_count: anim.loop_count,
            truncated: anim.truncated,
            start_index: 0,
            total_count: anim.frames.len() as u32,
            frames: anim
                .frames
                .iter()
                .map(|f| (f.delay_ms, f.image.data.as_slice()))
                .collect(),
        });
        let t_pack = t1.elapsed();

        // 自检：打好的包必须能被解回来（协议自洽）
        let (hdr, out) = super::super::pixel::decode_animation_blob(&blob).unwrap();
        assert_eq!((hdr.width, hdr.height), (anim.width, anim.height));
        assert_eq!(hdr.total_count as usize, anim.frames.len());
        assert_eq!(out.len(), anim.frames.len());

        // 顺带把包落盘，供外部量「可压缩性 / 逐帧冗余」
        let _ = std::fs::write(std::env::temp_dir().join("pv-anim.pva1"), &blob);

        eprintln!("── 基准 GIF {} 帧 ──", anim.frames.len());
        eprintln!("  解码+合成  : {:?}", t_decode);
        eprintln!("  PVA1 打包  : {:?}", t_pack);
        eprintln!("  裸字节合计 : {:.1} MB", blob.len() as f64 / 1048576.0);
        eprintln!("  （旧路径 base64 后约 {:.1} MB）", blob.len() as f64 * 4.0 / 3.0 / 1048576.0);
    }

    #[test]
    fn RawMetadata映射成前端DTO() {
        let m = RawMetadata {
            make: "Canon".into(),
            model: "Canon EOS M50".into(),
            raw_format: "CR3".into(),
            has_embedded_preview: true,
            bit_depth: 8,
            color_space: RawColorSpace::Srgb,
            width: 6000,
            height: 4000,
            ..Default::default()
        };
        let dto = raw_info_dto(&m);
        assert_eq!(dto.raw_format, "CR3");
        assert!(dto.has_embedded_preview);
        assert_eq!(dto.color_space, "sRGB");

        // 键名必须和前端 RawInfo 对上
        let v = serde_json::to_value(&dto).unwrap();
        for k in ["rawFormat", "hasEmbeddedPreview", "bitDepth", "colorSpace"] {
            assert!(v.get(k).is_some(), "缺字段 {k}");
        }
    }

    #[test]
    fn 色彩空间标签覆盖全部枚举() {
        // 每个变体都要有中文标签（漏一个前端就显示空洞）
        for (cs, expect) in [
            (RawColorSpace::Unknown, "未知"),
            (RawColorSpace::Srgb, "sRGB"),
            (RawColorSpace::AdobeRgb, "Adobe RGB"),
            (RawColorSpace::ProPhotoRgb, "ProPhoto RGB"),
            (RawColorSpace::CameraNative, "相机原生"),
        ] {
            assert_eq!(color_space_label(cs), expect);
        }
    }

    /// 端到端：真实 CR3 → registry 探测 → 映射成 DTO。
    ///
    /// 这条覆盖「应用层真的能拿到 RAW 元数据」这件事，
    /// 包含 CR3 的 CMT 合成路径（否则曝光参数会是空的）。
    #[test]
    fn 真实CR3经注册表探测后DTO完整() {
        let Some(p) = sample("canon-cr3-sample.CR3") else {
            eprintln!("跳过：CR3 样本不可用");
            return;
        };

        let mut reg = pv_decode::DecoderRegistry::new();
        reg.register(Box::new(pv_decode::BitmapBackend));
        reg.set_raw_pipeline(Box::new(pv_decode::EmbeddedPreviewPipeline::new()));

        let info = reg.probe(&p).expect("CR3 探测");
        assert!(info.is_raw(), "应判定为 RAW");

        let dto = raw_info_dto(info.raw.as_ref().expect("应带 RAW 元数据"));
        assert_eq!(dto.raw_format, "CR3");
        assert!(dto.has_embedded_preview);
        assert_eq!(dto.color_space, "sRGB");
        assert_eq!((info.width, info.height), (6000, 4000), "内嵌预览是全尺寸");

        // EXIF 也要能读到（信息面板的拍摄参数靠它）
        let exif = pv_meta::read_exif(&p).expect("CR3 应能读到 EXIF");
        assert!(exif.exposure_time.is_some(), "CR3 曝光参数应完整");
        assert!(exif.iso.is_some());
    }
}
