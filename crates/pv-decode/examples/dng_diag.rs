//! 诊断：DNG / TIFF 类 RAW 的打开链路。
//!
//! 构造几个结构不同的文件，逐层打印 probe / decode 的结果，
//! 定位「为什么某些 DNG 打不开」。
//!
//! 运行：cargo run -p pv-decode --example dng_diag

use std::fs;
use std::path::Path;

use pv_core::format::probe_header;
use pv_decode::{
    BitmapBackend, DecodePurpose, DecodeTarget, DecoderRegistry, EmbeddedPreviewPipeline,
};
use std::sync::Arc;

/// 造一张最小可用的 JPEG（含 DQT/DHT + SOF + SOS + EOI）。
///
/// Huffman 表必须是「合法的规范表」：zune-jpeg 会校验码长表，
/// 全 0 会被判 `Bogus Huffman table definition`。
/// 这里用一组最小合法表（DC: 1 个码长 1 的码；AC 同理）。
fn tiny_jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8]; // SOI
    // DQT
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
    out.extend(std::iter::repeat(0x10).take(64));
    // SOF0
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
    // DHT: DC 表（class 0, id 0）—— 1 个码长 1 的码，值 0
    out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x1F, 0x00]);
    out.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.push(0x00);
    // DHT: AC 表（class 1, id 0）—— 1 个码长 1 的码，值 0（EOB）
    out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x1F, 0x10]);
    out.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.push(0x00);
    // SOS
    out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    // 熵编码数据：每 8 位一个 0 码（EOB），2 万字节足够喂饱解码器
    out.extend(std::iter::repeat(0x00).take(20000));
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}

/// 构造一个 TIFF 容器：8 字节头 + IFD0（含 DNGVersion 或 Make/Model）+ 值区。
///
/// `tags` 是 (tag, type, count, value_bytes) —— value 超过 4 字节时写偏移。
fn build_tiff_le(tags: &[(u16, u16, u32, Vec<u8>)], extra_tail: &[u8]) -> Vec<u8> {
    let n = tags.len() as u16;
    let ifd_off = 8u32;
    let ifd_size = 2 + n as u32 * 12 + 4;
    let mut value_area: Vec<u8> = Vec::new();
    let value_base = ifd_off + ifd_size;

    let mut entries: Vec<u8> = Vec::new();
    for (tag, typ, count, val) in tags {
        entries.extend_from_slice(&tag.to_le_bytes());
        entries.extend_from_slice(&typ.to_le_bytes());
        entries.extend_from_slice(&count.to_le_bytes());
        if val.len() <= 4 {
            let mut v = val.clone();
            v.resize(4, 0);
            entries.extend_from_slice(&v);
        } else {
            let off = value_base + value_area.len() as u32;
            entries.extend_from_slice(&off.to_le_bytes());
            value_area.extend_from_slice(val);
            if value_area.len() % 2 == 1 {
                value_area.push(0);
            }
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"II"); // 小端
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&ifd_off.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&entries);
    out.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0
    out.extend_from_slice(&value_area);
    out.extend_from_slice(extra_tail);
    out
}

fn probe_of(name: &str, bytes: &[u8]) {
    let head = &bytes[..bytes.len().min(32)];
    let p = probe_header(head);
    println!(
        "  {name:<34} format={:<12} raw={} tier={:?}",
        format!("{:?}", p.format),
        p.format.is_raw(),
        p.tier
    );
}

fn main() {
    let out_dir = std::env::temp_dir().join("pv_dng_diag");
    fs::create_dir_all(&out_dir).unwrap();

    println!("═══ 1. 魔数判定（probe_header）═══");

    // (a) 标准 DNG 头：TIFF + DNGVersion 标签
    let jpeg = tiny_jpeg(640, 480);
    let dng_std = build_tiff_le(
        &[
            (0x0100, 4, 1, 640u32.to_le_bytes().to_vec()),   // ImageWidth
            (0x0101, 4, 1, 480u32.to_le_bytes().to_vec()),   // ImageLength
            (0xC612, 1, 4, vec![1, 4, 0, 0]),                // DNGVersion
            (0x010F, 2, 6, b"Apple\0".to_vec()),             // Make
        ],
        &jpeg,
    );
    probe_of("标准 DNG（TIFF + DNGVersion）", &dng_std);

    // (b) 纯 TIFF（无 DNG 标签）
    let plain_tiff = build_tiff_le(
        &[(0x0100, 4, 1, 640u32.to_le_bytes().to_vec())],
        &jpeg,
    );
    probe_of("纯 TIFF", &plain_tiff);

    // (c) Git LFS 指针文件
    let lfs = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 6372698\n";
    probe_of("Git LFS 指针文件", lfs);

    // (d) 空文件
    probe_of("空文件（0 字节）", b"");

    println!();
    println!("═══ 2. 端到端 probe / decode ═══");

    let mut registry = DecoderRegistry::new();
    registry.register(Box::new(BitmapBackend));
    registry.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));
    let reg = Arc::new(registry);

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("标准 DNG（含内嵌 JPEG）", dng_std.clone()),
        ("纯 TIFF（含内嵌 JPEG）", plain_tiff.clone()),
        ("DNG 无内嵌预览", build_tiff_le(
            &[(0xC612, 1, 4, vec![1, 4, 0, 0])],
            &[],
        )),
        ("Git LFS 指针文件", lfs.to_vec()),
        ("空文件", vec![]),
    ];

    for (name, bytes) in &cases {
        let path = out_dir.join(format!("{}.dng", name.replace(['（', '）', ' ', '/'], "_")));
        fs::write(&path, bytes).unwrap();

        print!("  {name:<28} ");
        match reg.probe(&path) {
            Ok(info) => {
                print!(
                    "probe OK fmt={:?} {}×{} raw={} ",
                    info.format,
                    info.width,
                    info.height,
                    info.raw.is_some()
                );
                let target = DecodeTarget::new(DecodePurpose::Preview);
                match reg.decode(&path, target) {
                    Ok(img) => println!("decode OK {}×{}", img.width, img.height),
                    Err(e) => println!("decode ERR: {e}"),
                }
            }
            Err(e) => println!("probe ERR: {e}"),
        }
    }

    println!();
    println!("═══ 3. 用真实 JPEG 做内嵌预览的 DNG（最贴近真实场景）═══");
    let real_jpeg = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image/DSC9515.JPG"),
    )
    .ok();

    if let Some(jpeg) = real_jpeg {
        println!("  用 test-image/DSC9515.JPG（{} 字节）作为内嵌预览", jpeg.len());
        // 真实 DNG 布局：TIFF 头 + IFD（含 DNGVersion/Make/Model）+ 内嵌 JPEG
        let dng = build_tiff_le(
            &[
                (0x0100, 4, 1, 5504u32.to_le_bytes().to_vec()), // ImageWidth
                (0x0101, 4, 1, 3096u32.to_le_bytes().to_vec()), // ImageLength
                (0x010E, 2, 20, b"A real embedded jpeg\0\0".to_vec()), // ImageDescription
                (0x010F, 2, 6, b"Canon\0".to_vec()),            // Make
                (0x0110, 2, 14, b"Canon EOS R5\0\0".to_vec()),  // Model
                (0xC612, 1, 4, vec![1, 4, 0, 0]),               // DNGVersion
                (0xC614, 2, 16, b"DNG diag test\0\0\0".to_vec()), // UniqueCameraModel
            ],
            &jpeg,
        );
        let dng_path = out_dir.join("real-preview.dng");
        fs::write(&dng_path, &dng).unwrap();

        print!("  probe: ");
        match reg.probe(&dng_path) {
            Ok(info) => {
                println!(
                    "OK fmt={:?} {}×{} raw={}",
                    info.format, info.width, info.height, info.raw.is_some()
                );
                if let Some(r) = &info.raw {
                    println!(
                        "         raw_format={} make={} model={} preview={} bit={} cs={:?}",
                        r.raw_format, r.make, r.model, r.has_embedded_preview, r.bit_depth, r.color_space
                    );
                }
            }
            Err(e) => println!("ERR: {e}"),
        }

        print!("  decode: ");
        match reg.decode(&dng_path, DecodeTarget::new(DecodePurpose::Preview)) {
            Ok(img) => {
                let nonblack = img.data.chunks(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count();
                let pct = nonblack as f64 / (img.width as f64 * img.height as f64) * 100.0;
                println!("OK {}×{} 非黑 {:.1}%", img.width, img.height, pct);
            }
            Err(e) => println!("ERR: {e}"),
        }

        // EXIF 是否读到
        print!("  EXIF: ");
        match pv_meta::read_exif(&dng_path) {
            Some(e) => println!(
                "make={:?} model={:?} iso={:?} shutter={:?} f={:?}",
                e.make, e.model, e.iso, e.exposure_time, e.f_number
            ),
            None => println!("None（DNG 的 EXIF 未读到）"),
        }
    } else {
        println!("  跳过：未找到 DSC9515.JPG");
    }

    println!();
    println!("═══ 4. 真实样本的魔数对照 ═══");
    let sample_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
    if let Ok(rd) = fs::read_dir(&sample_dir) {
        let mut names: Vec<_> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| {
                        let l = x.to_ascii_lowercase();
                        l == "dng" || l == "cr2" || l == "nef" || l == "arw" || l == "raw"
                    })
                    .unwrap_or(false)
            })
            .collect();
        names.sort();
        for p in names {
            let meta = fs::metadata(&p).unwrap();
            let mut head = vec![0u8; 32];
            if let Ok(mut f) = fs::File::open(&p) {
                use std::io::Read;
                let n = f.read(&mut head).unwrap_or(0);
                head.truncate(n);
            }
            let p2 = probe_header(&head);
            let is_lfs = head.starts_with(b"version https://git-lfs");
            println!(
                "  {:<30} {:>10} B  {:?}  {}",
                p.file_name().unwrap().to_string_lossy(),
                meta.len(),
                p2.format,
                if is_lfs { "← Git LFS 指针，非真 RAW" } else { "" }
            );
        }
    }

    println!();
    println!("输出目录：{}", out_dir.display());
}
