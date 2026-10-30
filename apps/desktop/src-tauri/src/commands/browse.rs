//! 文件浏览器命令（侧栏「文件夹」用的「我的电脑」式导航 + **临时预览**）。
//!
//! 薄壳：枚举/解码逻辑在 [`crate::fs_browse`] 与 `pv-store`/`pv-decode`。
//!
//! ## ⭐ 浏览 ≠ 入库（2026-10-16 用户明确的产品语义）
//! 文件浏览器里点目录只做**临时预览**：
//! - `fs_list_images` 直接读目录，**不建索引行**、不碰 `library.db`；
//! - `fs_thumbnail` 只解码缩放，**不写 L1/L2 缓存**、不编码 JPEG。
//!
//! 真正入库（写索引 + 缩略图落盘缓存）只发生在用户**右键 →「加入图库」**之后，
//! 走的是 `manager_open_folder`。两条路的格式判定是同一套（都走 `pv_scan::walk_dir`），
//! 不会出现「浏览看得到、加库加不进去」。
//!
//! ⚠️ **这些都必须是 `async`**：`read_dir` 在网络驱动器或无响应的盘符上会阻塞很久，
//! 非 `async` 命令跑在主线程上会直接把窗口卡成「未响应」（详见 `manager.rs` 的注释）。

use std::path::Path;

use tauri::ipc::Response;
use tauri::State;

use super::dto::ErrorDto;
use super::manager::parse_tier;
use super::pack_pixels;
use crate::fs_browse::{self, FsNode};
use crate::state::AppState;

/// 浏览列表一次最多返回多少个文件。
///
/// 一个目录直接在根下放几千张图是常见的（本机 `E:\Users\Fc\Pictures` 就是），
/// 全量塞给前端会挂出几千个卡片节点（各自带 IntersectionObserver）。
/// 超出的截断，由 `total` 告诉前端「还有更多」。
///
/// ⚠️ 这个数从 500 提到 20000（= 基本等于「不截断」）是**权衡后的选择**，
/// 依据是实测而不是感觉：卡片有 `content-visibility: auto`，离屏卡片不参与
/// layout/paint，所以开销主要落在「React 建 2000 个 fiber + 节点内存」上，
/// 而不是渲染流水线。1224 张实测见 `docs`（首次挂载 ~xxx ms，滚动不掉帧）。
/// ⛔ 再往上（几万）就该上**按行开窗**了 —— 只有把离屏节点从 DOM 里删掉
/// 才能让内存与 N 脱钩，`content-visibility` 做不到这一点。
const MAX_BROWSE_FILES: usize = 20_000;

/// 首屏先探多少个文件的尺寸。
///
/// ## 为什么不能「全部探完再返回」
/// 磁盘上的文件头探测单张只要 0.1–30ms（机械盘寻道慢），但一个 1224 个文件的目录
/// 串起来就是**几秒到十几秒**。这期间前端拿不到任何数据，界面只能是「正在处理…」。
/// 而用户其实只需要先看到**第一屏**（约 20 张）。
///
/// 所以：**列目录立刻返回，只把前 `probeLimit` 个的尺寸一起带回**，
/// 剩下的由前端在后台用 [`fs_probe_dimensions`] 分批补齐（卡片先少显示一段尺寸，
/// 几秒后补上，不影响使用）。
///
/// 60 ≈ 三屏，够用户滚动时一直有尺寸可看。
const FIRST_SCREEN_PROBE: usize = 60;

/// 侧栏文件树的**系统图标**（data URL，取一次缓存）。
///
/// 用的就是资源管理器那套图标（`SHGetFileInfoW`）：盘符会区分固定盘/可移动盘/
/// 光驱，文件夹是标准的黄色文件夹。前端启动时取一次即可。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeIconsDto {
    pub computer: Option<String>,
    pub folder: Option<String>,
    /// 每个盘符各自的图标：`[["C:\\", "data:image/png;base64,…"], …]`。
    ///
    /// ⛔ 不能共用一个「硬盘」图标：系统盘带 Windows 徽标、可移动盘和光驱
    /// 又是另一种样式，共用会让所有盘看起来都一样（实测过）。
    pub drives: Vec<(String, String)>,
}

#[tauri::command]
pub async fn fs_tree_icons() -> Result<TreeIconsDto, ErrorDto> {
    #[cfg(windows)]
    {
        let i = crate::shell_icon::tree_icons();
        Ok(TreeIconsDto {
            computer: i.computer,
            folder: i.folder,
            drives: i.drives,
        })
    }
    #[cfg(not(windows))]
    {
        // 非 Windows 平台没有 shell 图标，前端回退到字形
        Ok(TreeIconsDto {
            computer: None,
            folder: None,
            drives: Vec::new(),
        })
    }
}

/// 顶层节点：我的电脑（子节点为盘符）。
#[tauri::command]
pub async fn fs_roots() -> Result<Vec<FsNode>, ErrorDto> {
    Ok(fs_browse::roots())
}

/// 展开一个节点：`path` 省略或传 `__computer__` 时返回盘符，否则返回该目录的子目录。
#[tauri::command]
pub async fn fs_list_dirs(path: Option<String>) -> Result<Vec<FsNode>, ErrorDto> {
    Ok(fs_browse::list_dirs(path.as_deref()))
}

/// 地址栏解析结果：要浏览的目录 + 显示名。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedDirDto {
    pub path: String,
    pub name: String,
}

/// 把地址栏里敲的路径解析成要浏览的目录（`null` = 路径不存在，让前端提示）。
///
/// 规则见 [`fs_browse::resolve_dir`]：目录→自己、文件→父目录、`E:`→`E:\`、
/// 剥引号与空格。**不做 canonicalize**（会带 `\\?\` 前缀，与浏览器的路径对不上）。
#[tauri::command]
pub async fn fs_resolve_dir(path: String) -> Result<Option<ResolvedDirDto>, ErrorDto> {
    Ok(fs_browse::resolve_dir(&path).map(|(path, name)| ResolvedDirDto { path, name }))
}

/// 浏览列表里的一个文件。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsImageDto {
    pub path: String,
    pub name: String,
    pub size: i64,
    /// 修改时间（Unix 纳秒）。浏览模式没有索引，排序只能靠它 —— 必须带给前端。
    pub mtime: i64,
    pub format: String,
    pub is_raw: bool,
    /// 像素尺寸；探不出来为 `null`（卡片就少显示一段）
    pub width: Option<i64>,
    pub height: Option<i64>,
}

/// 目录浏览结果（`items` 可能被 [`MAX_BROWSE_FILES`] 截断）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListingDto {
    /// 目录里**实际**有多少张图
    pub total: usize,
    pub items: Vec<FsImageDto>,
}

/// 列出目录里的图片（**单层、不写索引、不生成缩略图**）—— 临时预览用。
///
/// ## 为什么不一上来就把所有尺寸探完
/// 单张文件头探测只要 0.1–30ms（机械盘寻道慢），但 1224 个文件串起来就是**几秒到十几秒**。
/// 那期间前端拿不到任何数据，界面只能是「正在处理…」，而用户其实只需要先看到第一屏。
///
/// 所以：**列目录立刻返回**，只把前 `probeLimit` 个的尺寸一起带回（默认
/// [`FIRST_SCREEN_PROBE`] = 60 ≈ 三屏），剩下的由前端调 [`fs_probe_dimensions`]
/// 在后台分批补 —— 卡片先少显示一段尺寸，几秒后补上，不影响使用。
///
/// 传 `0` = 一个都不探（最快），省略 = 默认值。
///
/// ⛔ RAW 一律不探（见 [`probe_size`]）：那会读整个 47MB 文件。
#[tauri::command]
pub async fn fs_list_images(
    state: State<'_, AppState>,
    path: String,
    probe_limit: Option<usize>,
) -> Result<DirListingDto, ErrorDto> {
    let all = pv_store::list_dir_images_fast(Path::new(&path))
        .map_err(|e| browse_err(&path, &e))?;
    let total = all.len();
    let probe_n = probe_limit.unwrap_or(FIRST_SCREEN_PROBE).min(MAX_BROWSE_FILES);
    let items = all
        .into_iter()
        .take(MAX_BROWSE_FILES)
        .enumerate()
        .map(|(i, f)| {
            let (width, height) = if i < probe_n {
                probe_size(state.registry(), &f.path, f.is_raw)
            } else {
                (None, None)
            };
            FsImageDto {
                path: f.path,
                name: f.name,
                size: f.size,
                mtime: f.mtime_ns,
                format: f.format,
                is_raw: f.is_raw,
                width,
                height,
            }
        })
        .collect::<Vec<_>>();
    Ok(DirListingDto { total, items })
}

/// 批量探测像素尺寸（后台补齐用）。
///
/// 返回值与 `paths` **一一对应**：探得出是 `[w, h]`，探不出 / RAW 是 `null`。
/// 前端拿到后按索引原地合并进已有列表，不重新拉整页。
#[tauri::command]
pub async fn fs_probe_dimensions(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<Option<(i64, i64)>>, ErrorDto> {
    Ok(paths
        .iter()
        .map(|p| {
            // 这里拿不到 `is_raw` 标记，靠 `probe_dimensions` 内部的 RAW 拦截兜底
            // （解码层直接返回 UnsupportedFormat，不会去读那 47MB）。
            pv_decode::probe_dimensions(state.registry(), Path::new(p))
                .ok()
                .map(|(w, h)| (i64::from(w), i64::from(h)))
        })
        .collect())
}

/// 批量读取**拍摄时间**（Unix 秒），返回值与 `paths` 一一对应，读不出是 `null`。
///
/// ## 为什么浏览模式需要它
/// 浏览列表不经过索引，没有 `taken_at` 列。用户选「按拍摄时间排序」时，
/// 只能现读 EXIF —— 但那是**每个文件一次 `open`**，1200 张要好几秒。
///
/// 所以前端**分批**调这个命令（每批 240 个），回来一批就合并重排一次：
/// 用户先看到「按修改时间近似」的顺序，随后逐步归位到正确的拍摄时间顺序。
///
/// ⚠️ 只在用户**主动选了**「按拍摄时间」时才调 —— 别在列目录时顺手读，
/// 那会把刚优化好的首屏（89ms）重新拖成几秒。
#[tauri::command]
pub async fn fs_probe_times(paths: Vec<String>) -> Result<Vec<Option<i64>>, ErrorDto> {
    Ok(paths
        .iter()
        .map(|p| pv_meta::read_exif(Path::new(p)).and_then(|e| e.taken_at_unix()))
        .collect())
}

/// 探一个文件的尺寸。**RAW 一律跳过**。
///
/// ⛔ 2026-10-17 实测：`probe_dimensions` 对 RAW 要把整个文件读进来扫内嵌 JPEG 预览，
/// 一张 ARW 47MB —— 611 个 ARW 的目录要读 28.7GB、界面假死 287 秒。
/// 而且内嵌预览的尺寸并不是传感器尺寸（1616×1080 vs 6000×4000），显示了反而是错的。
///
/// 解码层 [`pv_decode::probe_dimensions`] 也有一道同样的拦截（防调用点漏判），
/// 这里提前判断是为了连一次函数调用和一次 `open` 都省掉。
/// 把「浏览目录」的底层错误翻成用户能懂的一句话。
///
/// 原始的 `IO 错误：拒绝访问。(os error 5)` 对用户没有信息量 ——
/// 看到它只能猜是不是软件坏了。这里按常见成因给建议：
/// - 拒绝访问(5)：受保护目录 / 加密盘未解锁 / 需要管理员
/// - 设备未就绪(21 / 1117 / 1167)：光驱空、读卡器没插卡、移动盘掉线
///
/// ⚠️ 最常见的「拒绝访问」**已经不再是错误**（2026-10-30 修）：卷根上的
/// `pagefile.sys` / `hiberfil.sys` / `DumpStack.log` 读不了，会让整个目录
/// 列举失败 —— 现在扫描层单个文件探测失败就跳过它，正常盘一定能列出来。
fn browse_err(path: &str, e: &pv_store::StoreError) -> ErrorDto {
    let io_err = match e {
        pv_store::StoreError::Io(io) => Some(io),
        pv_store::StoreError::Index(pv_index::IndexError::Io(io)) => Some(io),
        _ => None,
    };
    let msg = match io_err.and_then(|io| io.raw_os_error()) {
        Some(5) => format!(
            "无法读取 {path}：系统拒绝访问。可能该目录受保护、所在盘被加密（BitLocker）未解锁，或以普通权限读不了"
        ),
        Some(21) | Some(1117) | Some(1167) => {
            format!("无法读取 {path}：设备未就绪（空光驱 / 读卡器没插卡 / 移动盘已断开）")
        }
        _ => format!("无法读取 {path}：{e}"),
    };
    ErrorDto {
        code: "browseError".into(),
        message: msg,
        user_visible: true,
    }
}

fn probe_size(
    reg: &pv_decode::DecoderRegistry,
    path: &str,
    is_raw: bool,
) -> (Option<i64>, Option<i64>) {
    if is_raw {
        return (None, None);
    }
    pv_decode::probe_dimensions(reg, Path::new(path))
        .map(|(w, h)| {
            // 6/8（竖拍）要把宽高换过来 —— 这和 `Store::probe_dims` 是同一个约定：
            // DTO 里的宽高一律是**显示尺寸**，否则浏览模式的卡片比例也是反的。
            let (w, h) = (i64::from(w), i64::from(h));
            match pv_meta::read_orientation(Path::new(path)) {
                Some(6) | Some(8) => (Some(h), Some(w)),
                _ => (Some(w), Some(h)),
            }
        })
        .unwrap_or((None, None))
}

/// 临时缩略图：**只解码缩放，不落任何缓存**。
///
/// 与 `manager_thumbnail`（入库那条路）的三点不同：
/// 1. 吃**绝对路径**而不是索引 id —— 浏览的目录可能根本没入过库；
/// 2. 不查 L1/L2，也不回填；
/// 3. 不做 JPEG 编码（编码只为写 L2），所以这条其实比入库那条还快。
#[tauri::command]
pub async fn fs_thumbnail(
    state: State<'_, AppState>,
    path: String,
    tier: String,
) -> Result<Response, ErrorDto> {
    let tier = parse_tier(&tier).ok_or_else(|| ErrorDto {
        code: "browseError".into(),
        message: format!("未知缩略图档位：{tier}"),
        user_visible: true,
    })?;
    let img = pv_decode::decode_scaled(state.registry(), Path::new(&path), tier.long_edge())
        .map_err(ErrorDto::from)?;
    Ok(Response::new(pack_pixels(&img)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> pv_decode::DecoderRegistry {
        let mut reg = pv_decode::DecoderRegistry::new();
        reg.register(Box::new(pv_decode::BitmapBackend));
        reg.set_raw_pipeline(Box::new(pv_decode::EmbeddedPreviewPipeline::new()));
        reg
    }

    /// ⛔ RAW 的尺寸探测必须**在打开文件之前**就被挡掉。
    ///
    /// 这条护栏的由来：`probe_dimensions` 对 RAW 要读整个文件（一张 ARW 47MB），
    /// 611 张就是 ~28GB、界面假死 287 秒。之前靠调用点自觉跳过，
    /// 一次回滚就把判断丢了（见 2026-10-17 日志）。
    ///
    /// 用**不存在的路径**断言 `(None, None)`：如果实现里先 `open` 再判 `is_raw`，
    /// 拿到的仍是 None，但 `is_raw` 判断若被删掉、且路径真实存在，就会真的去读文件 ——
    /// 所以再加一条「真实 RAW 样本也必须是 None」。
    #[test]
    fn RAW不探尺寸() {
        let reg = registry();
        assert_eq!(probe_size(&reg, "Z:/绝对不存在/x.ARW", true), (None, None));

        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-image/canon-cr3-sample.CR3");
        if sample.is_file() {
            let s = sample.to_string_lossy().into_owned();
            assert_eq!(
                probe_size(&reg, &s, true),
                (None, None),
                "RAW 行不该有尺寸（宁缺毋假：内嵌预览尺寸 ≠ 传感器尺寸）"
            );
        }
    }

    /// 非 RAW 走正常探测（证明上一条不是把整个函数改成恒返回 None）
    #[test]
    fn 非RAW正常探测() {
        let reg = registry();
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-image/prime-gaming-nekomata.webp");
        if !sample.is_file() {
            return;
        }
        let s = sample.to_string_lossy().into_owned();
        assert_eq!(probe_size(&reg, &s, false), (Some(7680), Some(4320)));
    }
}
