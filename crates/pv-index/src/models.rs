//! 数据行的 Rust 结构体（与 [`schema`](crate::schema) 对应）。
//!
//! 约定：`id == 0` 表示"尚未入库"；`insert` 时忽略 `id`，返回自增主键。

/// 卷状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeState {
    /// 已挂载
    Online,
    /// 已拔出 / 未连接
    Offline,
}

impl VolumeState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "online" => Some(Self::Online),
            "offline" => Some(Self::Offline),
            _ => None,
        }
    }
}

/// 条目（文件夹）状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// 正常可见
    Online,
    /// 所属卷未挂载（**不是删除**，保留索引与缓存）
    Offline,
    /// 卷在、但条目真的没了
    Missing,
}

impl EntryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "online" => Some(Self::Online),
            "offline" => Some(Self::Offline),
            "missing" => Some(Self::Missing),
            _ => None,
        }
    }
}

/// 卷（可移动存储身份，**不用盘符**）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub id: i64,
    pub volume_guid: String,
    pub serial: Option<i64>,
    pub label: Option<String>,
    pub fs: Option<String>,
    pub removable: bool,
    pub last_mount_path: Option<String>,
    pub last_seen_ts: Option<i64>,
    pub state: VolumeState,
}

impl Volume {
    /// 新建卷（仅必填 `volume_guid`，其余默认）。
    pub fn new(volume_guid: impl Into<String>) -> Self {
        Self {
            id: 0,
            volume_guid: volume_guid.into(),
            serial: None,
            label: None,
            fs: None,
            removable: false,
            last_mount_path: None,
            last_seen_ts: None,
            state: VolumeState::Online,
        }
    }
}

/// 文件夹（卷 + 相对路径）。绝对路径 = 卷挂载点 + `rel_path`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub volume_id: i64,
    pub rel_path: String,
    pub parent_id: Option<i64>,
    pub is_root: bool,
    pub ignored: bool,
    pub vault_id: Option<i64>,
    pub file_count: i64,
    pub state: EntryState,
}

impl Folder {
    pub fn new(volume_id: i64, rel_path: impl Into<String>) -> Self {
        Self {
            id: 0,
            volume_id,
            rel_path: rel_path.into(),
            parent_id: None,
            is_root: false,
            ignored: false,
            vault_id: None,
            file_count: 0,
            state: EntryState::Online,
        }
    }
}

/// 图像（最小字段集，先用于索引与网格）。
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub id: i64,
    pub dir_id: i64,
    pub rel_name: Option<String>,
    pub size: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub format: Option<String>,
    pub is_raw: bool,
    pub taken_at: Option<i64>,
    pub rating: i64,
    pub flag: i64,
    pub content_key: Option<String>,
    /// 是否已有标签（列表查询时由 `EXISTS(image_tags)` 计算）。
    /// 卡片据此显示「无匹配」——**不改表结构**，只在列表 SQL 里算。
    pub has_tags: bool,
}

impl Image {
    pub fn new(dir_id: i64, rel_name: impl Into<String>) -> Self {
        Self {
            id: 0,
            dir_id,
            rel_name: Some(rel_name.into()),
            size: None,
            mtime_ns: None,
            width: None,
            height: None,
            format: None,
            is_raw: false,
            taken_at: None,
            rating: 0,
            flag: 0,
            content_key: None,
            has_tags: false,
        }
    }
}

/// 图库类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    /// 普通图库：显式包含若干文件夹
    Normal,
    /// 智能图库：由搜索规则动态求值
    Smart,
    /// 加密图库：内容加密，需密码解锁
    Encrypted,
}

impl LibraryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Smart => "smart",
            Self::Encrypted => "encrypted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "normal" => Some(Self::Normal),
            "smart" => Some(Self::Smart),
            "encrypted" => Some(Self::Encrypted),
            _ => None,
        }
    }
}

/// 图库（普通 / 智能 / 加密）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub id: i64,
    pub name: String,
    pub kind: LibraryKind,
    pub rule: Option<String>,
    pub cover_image: Option<i64>,
    pub vault_id: Option<i64>,
    pub sort_field: Option<String>,
    pub created_ts: Option<i64>,
}

impl Library {
    pub fn new(name: impl Into<String>, kind: LibraryKind) -> Self {
        Self {
            id: 0,
            name: name.into(),
            kind,
            rule: None,
            cover_image: None,
            vault_id: None,
            sort_field: None,
            created_ts: None,
        }
    }
}

/// 标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub kind: Option<String>,
    pub color: Option<String>,
}

/// 缩略图索引（像素本体在 L2 磁盘缓存，此表只记元信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    pub image_id: i64,
    pub tier: i64,
    pub cache_path: Option<String>,
    pub bytes: Option<i64>,
    pub generated_ts: Option<i64>,
}

/// 加密容器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    pub id: i64,
    pub name: Option<String>,
    pub kdf_params: Option<String>,
    pub salt: Option<Vec<u8>>,
    pub verifier: Option<Vec<u8>>,
    pub created_ts: Option<i64>,
    pub last_unlock_ts: Option<i64>,
}

impl Vault {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: 0,
            name: Some(name.into()),
            kdf_params: None,
            salt: None,
            verifier: None,
            created_ts: None,
            last_unlock_ts: None,
        }
    }
}

/// 一张人脸（`faces` 表的一行）。
///
/// 坐标是**归一化**的（0..1，相对检测时那一档缩略图），配 `tier` 才能换算回像素 ——
/// 存像素坐标会在换缩略图档位后全部失效。
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub id: i64,
    pub image_id: i64,
    /// 归属的人物；`None` = 还没聚类（或聚类后是单例）
    pub person_id: Option<i64>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub tier: Option<i32>,
    /// 产出这条向量的模型（如 `sface_2021dec`）。**换模型必须整库重建**。
    pub model: String,
    pub score: Option<f32>,
    /// 质量指标（当前存脸宽像素）：小脸/模糊脸的向量不可信，聚类前先过滤。
    pub quality: Option<f32>,
    /// 128 × int8，已 L2 归一化（见 [`faces::quantize`](crate::repo::faces::quantize)）。
    pub embedding: Vec<u8>,
}

impl Face {
    /// 只填几何 + 质量，`embedding` 由调用方随后写入。
    pub fn new(image_id: i64, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            id: 0,
            image_id,
            person_id: None,
            x,
            y,
            w,
            h,
            tier: None,
            model: String::new(),
            score: None,
            quality: None,
            embedding: Vec::new(),
        }
    }
}

/// 一个人物（`persons` 表的一行）：一次聚类产出的簇，名字由用户给。
///
/// ⛔ `name == None` 就显示「未命名 ①②」——**绝不自动猜这是谁**（零幻觉 + 隐私）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub cover_face_id: Option<i64>,
    pub face_count: i64,
    pub cluster_version: Option<String>,
    pub created_ts: Option<i64>,
}

impl Person {
    pub fn new(name: Option<String>, cluster_version: Option<String>) -> Self {
        Self {
            id: 0,
            name,
            cover_face_id: None,
            face_count: 0,
            cluster_version,
            created_ts: None,
        }
    }
}
