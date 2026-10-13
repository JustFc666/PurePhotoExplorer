//! 像素传输协议的**应用层**验证（静图 PVI1 + 动图 PVA1）。
//!
//! `commands/pixel.rs` 的单测验证了「打包/解包这对函数自洽」；
//! 这条测试再往上走一层，证明：
//! 1. **真实 GIF 走完整链路**（文件 → 解码合成 → 打包 → 解回）后，
//!    帧数、每帧延时、尺寸都不丢；
//! 2. 传输体积**没有 base64 膨胀**（这正是本次性能优化的核心指标）；
//! 3. `tauri.conf.json` 的 CSP 放行了 `ipc: http://ipc.localhost` ——
//!    这是「invoke 走快通道 fetch 而不是慢通道 postMessage/eval」的前提条件。
//!
//! 第 3 条是**回归护栏**：CSP 一旦被改回去，整个 IPC 会静默退化成慢通道
//! （功能不坏、只是大图明显卡顿），极难在功能测试里发现 —— 所以在这里钉死。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![allow(non_snake_case)]

use std::path::{Path, PathBuf};

use photovault_lib::ipc_protocol::{
    decode_animation_blob, pack_animation, AnimPack, ANIM_HEADER_FIXED, ANIM_MAGIC, ANIM_VERSION,
};
use pv_decode::{DecodePurpose, DecodeTarget};

/// 取仓库 `test-image/` 下的样本（小于 4KB 视为 Git LFS 指针，不算）。
///
/// ⚠️ `CARGO_MANIFEST_DIR` 是 `apps/desktop/src-tauri`，**上三层**才是仓库根。
/// 这里先 assert 目录存在 —— 否则路径写错时测试会「静默跳过」并显示 ok，
/// 相当于没有测试（`tests/raw_bridge.rs` 就踩过这个坑，2026-10-09 一起修了）。
fn sample(name: &str) -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../test-image");
    assert!(
        dir.is_dir(),
        "找不到 test-image 目录（推算路径 {}）—— 样本定位逻辑坏了，测试会假通过",
        dir.display()
    );
    let p = dir.join(name);
    if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
        Some(p)
    } else {
        None
    }
}

#[test]
fn 真实GIF打包解包后帧与延时都不丢() {
    let Some(p) = sample("a746f3dcly1gnnxc9hraig206y06yx6s.gif") else {
        eprintln!("跳过：找不到 GIF 样本");
        return;
    };

    let anim = pv_decode::decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("样本 GIF 应能解码");
    assert!(anim.frames.len() > 1, "该样本是动图，应有多帧");
    assert_eq!(anim.frames.len(), 299, "样本帧数固定，变了说明解码器行为变了");

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

    let (hdr, out) = decode_animation_blob(&blob).expect("自己打的包必须能自己解开");

    assert_eq!((hdr.width, hdr.height), (anim.width, anim.height), "尺寸必须一致");
    assert_eq!(out.len(), anim.frames.len(), "帧数必须一致");
    assert_eq!(hdr.total_count as usize, anim.frames.len(), "总帧数必须一致");
    assert_eq!(hdr.start_index, 0, "整段打包应从第 0 帧开始");
    assert_eq!(hdr.truncated, anim.truncated);
    let loop_i32 = hdr.loop_count;

    // 无限循环编码成 -1；有限循环原样带过来
    match anim.loop_count {
        None => assert_eq!(loop_i32, -1, "无限循环应编码成 -1"),
        Some(n) => assert_eq!(loop_i32, i32::from(n)),
    }

    // 每帧延时与像素逐帧对齐（最容易串位的地方）
    for (i, (delay, data)) in out.iter().enumerate() {
        assert_eq!(*delay, anim.frames[i].delay_ms, "第 {i} 帧延时");
        assert_eq!(
            *data,
            anim.frames[i].image.data.as_slice(),
            "第 {i} 帧像素被串位或截断了"
        );
    }
}

#[test]
fn 动图传输体积没有base64膨胀() {
    let Some(p) = sample("a746f3dcly1gnnxc9hraig206y06yx6s.gif") else {
        eprintln!("跳过：找不到 GIF 样本");
        return;
    };

    let anim = pv_decode::decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
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

    let n = anim.frames.len();
    let stride = anim.width as usize * anim.height as usize * 4;
    let expect = ANIM_HEADER_FIXED + n * 4 + n * stride;
    assert_eq!(
        blob.len(),
        expect,
        "裸字节包长应恰为 头 + 延时表 + 像素，多出来的就是编码膨胀"
    );

    // 对照：老方案（每帧 PVI1 13 字节头 + base64）会大出约 1/3
    let old = n * (13 + stride);
    let old_b64 = old.div_ceil(3) * 4;
    let new_bytes = blob.len() as f64;
    assert!(
        new_bytes < old_b64 as f64 * 0.8,
        "新方案应比 base64 方案至少小 20%（实际 {:.1}MB vs {:.1}MB）",
        new_bytes / 1048576.0,
        old_b64 as f64 / 1048576.0
    );
}

#[test]
fn 动图魔数与版本常量与前端一致() {
    // 前端 `ipc/protocol.ts` 里是同名的字面量，改了这里必须同步改那边
    assert_eq!(ANIM_MAGIC, b"PVA1");
    assert_eq!(ANIM_VERSION, 2);
    assert_eq!(ANIM_HEADER_FIXED, 32);
}

/// **分块传输的端到端验证**（2026-10-09 性能优化）。
///
/// 前端把动图拆成两段请求：先要开头 `maxBytes` 字节的帧让画面立刻动起来，
/// 再要剩下的。这条测试证明「两段拼起来」与「一次取完」**逐帧完全等价** ——
/// 否则用户会看到播放到接缝处跳帧或倒回。
#[test]
fn 分块请求与一次性请求逐帧等价() {
    let Some(p) = sample("a746f3dcly1gnnxc9hraig206y06yx6s.gif") else {
        eprintln!("跳过：找不到 GIF 样本");
        return;
    };

    // 与 `commands/image.rs::decode_animation` 相同的切分逻辑：
    // 每帧 width*height*4 字节，预算 8MB。
    let anim = pv_decode::decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
    let total = anim.frames.len();
    let frame_len = anim.width as usize * anim.height as usize * 4;
    let budget = 8 * 1024 * 1024;
    let first = (budget / frame_len).max(1).min(total);
    assert!(first < total, "8MB 预算应切不住整段 71MB（否则这条测试没意义）");

    // 注意：解码出来的帧是借 `blob` 的切片，所以这里复制成 owned 再返回
    let make = |from: usize, to: usize| -> (photovault_lib::ipc_protocol::AnimHeader, Vec<(u32, Vec<u8>)>) {
        let slice = &anim.frames[from..to];
        let blob = pack_animation(AnimPack {
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
        });
        let (hdr, frames) = decode_animation_blob(&blob).expect("自己打的包必须能自己解开");
        (hdr, frames.into_iter().map(|(d, p)| (d, p.to_vec())).collect())
    };

    let (h1, f1) = make(0, first);
    let (h2, f2) = make(first, total);

    // 两段各自的元信息
    assert_eq!((h1.start_index, h1.total_count), (0, total as u32), "第一段");
    assert_eq!((h2.start_index, h2.total_count), (first as u32, total as u32), "第二段");
    assert_eq!(h1.loop_count, h2.loop_count, "两段的循环方式必须一致");
    assert_eq!((h1.width, h1.height), (h2.width, h2.height));
    assert_eq!(f1.len() + f2.len(), total, "两段帧数之和 = 总帧数");

    // 拼起来必须与整段完全一致
    let merged: Vec<_> = f1.into_iter().chain(f2).collect();
    for (i, (delay, data)) in merged.iter().enumerate() {
        assert_eq!(*delay, anim.frames[i].delay_ms, "第 {i} 帧延时");
        assert_eq!(
            *data,
            anim.frames[i].image.data,
            "第 {i} 帧像素（拼接后与整段不一致）"
        );
    }
}

/// **回归护栏**：CSP 必须放行 IPC 自定义协议。
///
/// Tauri 的 invoke 在 Windows 上是
/// `fetch('http://ipc.localhost/<cmd>', ...)`；CSP 若不含
/// `connect-src ipc: http://ipc.localhost`，这个 fetch 会被 WebView2 拦掉，
/// Tauri 随即**永久回退**到 `window.ipc.postMessage` 通道 ——
/// 该通道对大响应走 `webview.eval()`（把数据拼成 JS 源码再执行），
/// 几百 MB 的像素数据在那儿要花好几秒，且这条退化是**静默的**。
#[test]
fn CSP放行了IPC自定义协议() {
    let conf_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let text = std::fs::read_to_string(&conf_path).expect("应能读到 tauri.conf.json");
    let v: serde_json::Value = serde_json::from_str(&text).expect("tauri.conf.json 应是合法 JSON");

    let csp = v["app"]["security"]["csp"]
        .as_str()
        .expect("应配置 app.security.csp（设成 null 会让 fetch 通道被原样放开，但也就失去了脚本注入防护）");

    assert!(
        csp.contains("connect-src"),
        "CSP 必须显式声明 connect-src，否则会落到 default-src，IPC 的 fetch 会被拦。当前：{csp}"
    );
    assert!(
        csp.contains("ipc:"),
        "CSP 的 connect-src 必须含 `ipc:` 源（macOS/Linux 用）。当前：{csp}"
    );
    assert!(
        csp.contains("http://ipc.localhost"),
        "CSP 的 connect-src 必须含 `http://ipc.localhost`（Windows/Android 用）。当前：{csp}"
    );
}

/// WebP 走**统一动图入口**的完整链路验证。
///
/// 项目里的 WebP 样本是 7680×4320 的**无损静态图**，所以这里断言的是
/// 「静态 WebP 也能安全地走 PVA1 管线」，而不是多帧。
/// 真正多帧的情况由 `pv-decode` 的单元测试与 `image-webp` 自己的合成逻辑覆盖 ——
/// 我们要保证的是**接进本项目管线后不丢东西**。
#[test]
fn 统一动图入口能处理WebP() {
    let Some(p) = sample("prime-gaming-nekomata.webp") else {
        eprintln!("跳过：找不到 WebP 样本");
        return;
    };

    // ⚠️ 走的是命令层用的那个入口（不是 `decode_gif`），这样才覆盖真实路径
    let anim = pv_decode::decode_animation(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("样本 WebP 应能通过统一入口解码");

    assert_eq!(anim.frames.len(), 1, "该样本是静态 WebP，应只有一帧");
    assert!(anim.is_static());
    // 预览档 max_edge = 2048 → 7680×4320 等比缩到 2048×1152。
    // `width/height` 必须与**帧像素**一致：PVA1 头里的这两个字段就是给前端
    // 算 stride 用的（`width * height * 4`），报原始画布尺寸会让包长对不上。
    assert_eq!(
        (anim.width, anim.height),
        (2048, 1152),
        "尺寸应是帧像素尺寸（不是原始 7680×4320）"
    );
    assert_eq!(
        (anim.frames[0].image.width, anim.frames[0].image.height),
        (anim.width, anim.height),
        "帧像素尺寸必须与 Animation 声明的尺寸一致"
    );
    assert!(!anim.truncated);
    assert!(anim.loop_count.is_none(), "静态 WebP 没有循环语义，应为 None");

    // PVA1 打包 → 解回，帧与像素逐字节一致
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
    let (hdr, out) = decode_animation_blob(&blob).expect("自己打的包必须能自己解开");

    assert_eq!((hdr.width, hdr.height), (anim.width, anim.height));
    assert_eq!(out.len(), 1);
    assert_eq!(hdr.loop_count, -1, "None 应编码成 -1（无限循环）");
    assert_eq!(hdr.start_index, 0);
    assert_eq!(hdr.total_count, 1);
    assert_eq!(out[0].0, anim.frames[0].delay_ms, "延时");
    assert_eq!(
        out[0].1,
        anim.frames[0].image.data.as_slice(),
        "WebP 帧像素经 PVA1 往返后被改动了"
    );

    // 体积核算：裸字节包 = 头 32 + 延时表 4 + 像素；没有 base64 膨胀
    let stride = 2048 * 1152 * 4;
    assert_eq!(blob.len(), ANIM_HEADER_FIXED + 4 + stride);
}

/// **新增格式时最容易漏的一步**：统一动图入口必须仍然认 GIF。
///
/// 它是个 `match` 分发（WebP 一条、其余落 GIF），改动时很容易把默认分支写坏 ——
/// 而 GIF 那边有 299 帧、路径完全不同，坏了不容易一眼看出来。
#[test]
fn 统一动图入口仍然认GIF() {
    let Some(p) = sample("a746f3dcly1gnnxc9hraig206y06yx6s.gif") else {
        eprintln!("跳过：找不到 GIF 样本");
        return;
    };
    let via_union = pv_decode::decode_animation(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("GIF 应能通过统一入口解码");
    let direct = pv_decode::decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("GIF 直接入口应能解码");

    assert_eq!(via_union.frames.len(), direct.frames.len(), "帧数应与直接入口一致");
    assert_eq!(via_union.frames.len(), 299);
    assert_eq!(
        (via_union.width, via_union.height),
        (direct.width, direct.height),
        "尺寸应与直接入口一致"
    );
    assert_eq!(
        via_union.loop_count, direct.loop_count,
        "循环方式应与直接入口一致"
    );
}

/// **钉住前端分支判据**：前端 `useImageLoader` 用
/// `meta.format === 'gif' || meta.format === 'webp'` 决定走不走动图通道。
///
/// 这个字符串不是手写的常数，而是后端 `format!("{:?}", ImageFormat).to_lowercase()`
/// 生成的。枚举一旦改名、或者哪天有人把它换成 `display_name()`，
/// 前端分支会**静默失效** —— 静态图照样能显示（走普通通道），
/// 但**动画 WebP 只会显示第一帧**，很难被发现。
#[test]
fn 前端判格式用的字符串与后端枚举一致() {
    use pv_core::format::ImageFormat;
    for (format, expected) in [
        (ImageFormat::Gif, "gif"),
        (ImageFormat::WebP, "webp"),
    ] {
        assert_eq!(
            format!("{format:?}").to_lowercase(),
            expected,
            "前端按 '{expected}' 判断动图通道，改了这个字符串要同步改 useImageLoader",
        );
    }
}

/// **回归测试**：`Animation.width/height` 必须是**帧像素**尺寸。
///
/// 这个语义曾经是错的：GIF 解码器报的是**原始画布**尺寸，而帧像素是按
/// `DecodeTarget` 缩放过的。样本恰好是 250×250（≤ 预览档 2048，帧没被缩放），
/// 两者相等，所以一直没暴露 —— 一旦遇到大于目标档位的动图，
/// PVA1 的包长就与实际像素对不上，前端按 `width*height*4` 切帧会直接解析失败。
///
/// 这里用 `DecodeTarget::exact(..., Some(100))` **强制缩小**来锁住语义：
/// 只要尺寸字段再被改回原始画布，下面的断言与打包自检都会挂。
#[test]
fn 动图尺寸字段必须是帧像素尺寸而非原始画布() {
    let Some(p) = sample("a746f3dcly1gnnxc9hraig206y06yx6s.gif") else {
        eprintln!("跳过：找不到 GIF 样本");
        return;
    };
    let small = DecodeTarget::exact(DecodePurpose::Preview, Some(100));
    let anim = pv_decode::decode_animation(&p, small).expect("应能解码");

    assert_eq!(
        (anim.width, anim.height),
        (100, 100),
        "应报缩放后的帧尺寸（原始画布是 250×250）"
    );
    for (i, f) in anim.frames.iter().enumerate() {
        assert_eq!(
            (f.image.width, f.image.height),
            (anim.width, anim.height),
            "第 {i} 帧的像素尺寸与声明不符"
        );
    }

    // 真正的判据：按声明尺寸打包后必须能自洽解开
    // （尺寸报错时这一步会以「长度与声明的帧数/尺寸不匹配」失败）
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
    let (hdr, out) = decode_animation_blob(&blob)
        .expect("声明尺寸与帧像素不一致时，这里会以长度不匹配失败");
    assert_eq!((hdr.width, hdr.height), (100, 100));
    assert_eq!(out.len(), anim.frames.len());
}
