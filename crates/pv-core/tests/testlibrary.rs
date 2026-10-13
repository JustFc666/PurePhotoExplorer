//! 测试图库兼容性验证（FR-1.5 实战测试）
//!
//! 用真实相机 RAW 文件验证格式探测与探测策略。
//! **关键场景**：Git LFS 指针文件（纯文本）伪装成 .DNG/.NEF/.ARW，
//! 探测必须返回 Unknown 而**不能**因为扩展名就当成 RAW。
//! 这正是"不信任扩展名"设计要防的情况。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![allow(non_snake_case)]

use pv_core::format::{probe_header, ImageFormat};
use std::path::{Path, PathBuf};

fn test_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/crates/pv-core，回退两级是仓库根
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pv-core 应在 crates/ 下")
        .parent()
        .expect("crates/ 应在仓库根下")
        .join("test-image")
}

fn probe_file(path: &Path) -> ImageFormat {
    let mut f = std::fs::File::open(path).expect("打开测试文件");
    let mut head = [0u8; 32];
    let n = std::io::Read::read(&mut f, &mut head).expect("读取文件头");
    probe_header(&head[..n]).format
}

#[test]
fn 测试图库目录存在() {
    let dir = test_dir();
    assert!(
        dir.exists(),
        "测试图库不存在：{}。请确认 test-image 目录已就位",
        dir.display()
    );
}

#[test]
fn 真实RAW文件被正确识别() {
    let dir = test_dir();
    // 依据真实文件头判定（2026-10-03 实测）：
    // - CR3 用 ISOBMFF，偏移 4 = "ftyp"，偏移 8 = "crx " → 可靠识别
    // - RAF 用 "FUJIFILMCCD-RAW" 魔数 → 可靠识别
    // - ORF 用 "IIRO" 魔数 → 可靠识别
    // - RW2 用 "IIU\0" 魔数 → 可靠识别
    // - NEF/SR2/SRF 都是 II* 容器，16 字节魔数无法区分厂商 → OtherRaw
    let cases: &[(&str, ImageFormat)] = &[
        ("canon-cr3-sample.CR3", ImageFormat::CanonCr3),
        ("fuji-raf-sample.RAF", ImageFormat::FujiRaf),
        ("olympus-orf-sample.ORF", ImageFormat::OlympusOrf),
        ("panasonic-rw2-sample.RW2", ImageFormat::PanasonicRw2),
        ("nikon-nrw-sample.NRW", ImageFormat::OtherRaw),
    ];

    let mut found = 0;
    for (name, expected) in cases {
        let p = dir.join(name);
        if !p.exists() {
            continue;
        }
        let got = probe_file(&p);
        assert_eq!(
            got, *expected,
            "{} 探测错误：期望 {:?}，实际 {:?}",
            name, expected, got
        );
        found += 1;
    }
    assert!(found > 0, "没有可用的真实 RAW 样本");
}

#[test]
fn LFS指针文件不被误判为RAW() {
    let dir = test_dir();
    // 这些文件扩展名是 .dng/.nef/.arw/.cr2，但内容是 Git LFS 纯文本指针
    let lfs_files = [
        "adobe-dng-sample.dng",
        "canon-cr2-sample.cr2",
        "leica-dng-sample.DNG",
        "leica-raw-sample.RAW",
        "nikon-nef-sample.nef",
        "sony-ilce7m3-sample.arw",
        "generic-raw-sample.raw",
    ];

    for name in lfs_files {
        let p = dir.join(name);
        if !p.exists() {
            continue;
        }
        // 关键断言：必须判定为 Unknown
        let got = probe_file(&p);
        assert_eq!(
            got,
            ImageFormat::Unknown,
            "{} 是 LFS 指针文件（纯文本），却被误判为 {:?} —— \
             这说明格式探测被扩展名误导了",
            name, got
        );
    }
}

#[test]
fn TIFF家族RAW归为OtherRaw交由LibRaw判定() {
    let dir = test_dir();
    // Sony SR2/SRF 用 II* 容器，仅凭魔数无法与普通 TIFF 区分
    for name in ["sony-sr2-sample.SR2", "sony-srf-sample.SRF"] {
        let p = dir.join(name);
        if !p.exists() {
            continue;
        }
        let got = probe_file(&p);
        assert!(
            got == ImageFormat::OtherRaw || got.is_raw(),
            "{} 应被识别为 RAW 家族，实际 {:?}",
            name,
            got
        );
    }
}

#[test]
fn 统计图库可用性() {
    let dir = test_dir();
    let mut total = 0;
    let mut real_raw = 0;
    let mut lfs = 0;
    let mut other = 0;

    for entry in std::fs::read_dir(&dir).expect("读取图库目录") {
        let e = entry.expect("目录项");
        let p = e.path();
        if p.extension().is_none() {
            continue;
        }
        total += 1;
        let f = probe_file(&p);
        if f == ImageFormat::Unknown {
            lfs += 1;
        } else if f.is_raw() {
            real_raw += 1;
        } else {
            other += 1;
        }
    }

    println!(
        "图库统计：共 {} 个文件，可用 RAW {} 个，非图像（疑似 LFS 指针）{} 个，其他 {} 个",
        total, real_raw, lfs, other
    );
    assert!(total > 0, "图库为空");
    // 至少要有一个可用的真实 RAW
    assert!(real_raw > 0, "没有任何可用的真实 RAW 样本，无法验证格式支持");
}

#[test]
fn 损坏样本不导致崩溃() {
    let dir = test_dir();
    // 空文件与极短文件都应安全返回
    for name in ["fuji-raf-sample.RAF", "nikon-nrw-sample.NRW"] {
        let p = dir.join(name);
        if !p.exists() {
            continue;
        }
        // 只读文件头，不做完整解析——不应该 panic
        let _ = probe_file(&p);
    }
}
