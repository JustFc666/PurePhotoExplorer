//! SQLite DDL 与 schema 版本。
//!
//! **约定**：表结构变更时**不要改旧版的 `DDL_*`**，而是在 [`migrations`](crate::migrations)
//! 里追加新版本并写增量 DDL——保证老库可升级、新库一次建全。

/// v1 全量 DDL（首次建库）。
///
/// 与开发手册 §4 一一对应。注意：
/// - 路径一律「卷 + 相对路径」，**不存盘符**；
/// - 加密图库下 `images.rel_name` 为 NULL，改用 `enc_name`；
/// - `fts_images` 用**普通 FTS5 表**（可 UPDATE/DELETE），不用 contentless。
pub const DDL_V1: &str = r#"
-- ── 卷（可移动存储身份，不用盘符）──
CREATE TABLE volumes (
  id              INTEGER PRIMARY KEY,
  volume_guid     TEXT NOT NULL UNIQUE,
  serial          INTEGER,
  label           TEXT,
  fs              TEXT,
  removable       INTEGER NOT NULL DEFAULT 0,
  last_mount_path TEXT,
  last_seen_ts    INTEGER,
  state           TEXT NOT NULL DEFAULT 'online'
);

-- ── 加密容器 ──
CREATE TABLE vaults (
  id             INTEGER PRIMARY KEY,
  name           TEXT,
  kdf_params     TEXT,
  salt           BLOB,
  verifier       BLOB,
  created_ts     INTEGER,
  last_unlock_ts INTEGER
);

-- ── 文件夹（卷 + 相对路径）──
CREATE TABLE folders (
  id           INTEGER PRIMARY KEY,
  volume_id    INTEGER NOT NULL REFERENCES volumes(id),
  rel_path     TEXT NOT NULL,
  parent_id    INTEGER REFERENCES folders(id),
  is_root      INTEGER NOT NULL DEFAULT 0,
  ignored      INTEGER NOT NULL DEFAULT 0,
  vault_id     INTEGER REFERENCES vaults(id),
  file_count   INTEGER NOT NULL DEFAULT 0,
  last_scan_ts INTEGER,
  state        TEXT NOT NULL DEFAULT 'online',
  UNIQUE (volume_id, rel_path)
);

-- ── 图库 ──
CREATE TABLE libraries (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  kind        TEXT NOT NULL,               -- normal | smart | encrypted
  rule        TEXT,
  cover_image INTEGER,
  vault_id    INTEGER REFERENCES vaults(id),
  sort_field  TEXT DEFAULT 'taken_at',
  created_ts  INTEGER
);

CREATE TABLE library_folders (
  library_id INTEGER NOT NULL REFERENCES libraries(id),
  folder_id  INTEGER NOT NULL REFERENCES folders(id),
  PRIMARY KEY (library_id, folder_id)
);

-- ── 地点（pv-geo 反查）──
CREATE TABLE places (id INTEGER PRIMARY KEY, name TEXT, lat REAL, lon REAL, level INTEGER);
CREATE INDEX idx_places_geo ON places(lat, lon);

-- ── 图像 ──
CREATE TABLE images (
  id          INTEGER PRIMARY KEY,
  dir_id      INTEGER NOT NULL REFERENCES folders(id),
  rel_name    TEXT,                        -- 文件名；加密图库为 NULL
  enc_name    BLOB,                        -- 加密图库：密文名
  file_id     TEXT,                        -- 加密图库：容器内条目 id
  size        INTEGER,
  mtime_ns    INTEGER,
  width       INTEGER,
  height      INTEGER,
  format      TEXT,
  orientation INTEGER,
  is_raw      INTEGER NOT NULL DEFAULT 0,
  taken_at    INTEGER,
  added_ts    INTEGER,
  rating      INTEGER NOT NULL DEFAULT 0,
  flag        INTEGER NOT NULL DEFAULT 0,
  content_key TEXT                         -- volume_guid+rel_path+size+mtime_ns
);
CREATE INDEX idx_images_dir    ON images(dir_id);
CREATE INDEX idx_images_taken  ON images(taken_at);
CREATE INDEX idx_images_rating ON images(rating);

-- ── EXIF ──
CREATE TABLE image_exif (
  image_id INTEGER PRIMARY KEY REFERENCES images(id),
  make TEXT, model TEXT, lens TEXT,
  iso INTEGER, f_number REAL, exposure REAL, focal_len REAL,
  gps_lat REAL, gps_lon REAL, gps_alt REAL,
  place_id INTEGER REFERENCES places(id),
  raw_json TEXT
);

-- ── 标签 ──
CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT UNIQUE, kind TEXT, color TEXT);
CREATE TABLE image_tags (
  image_id   INTEGER NOT NULL REFERENCES images(id),
  tag_id     INTEGER NOT NULL REFERENCES tags(id),
  source     TEXT,
  confidence REAL,
  PRIMARY KEY (image_id, tag_id, source)
);

-- ── AI 分析结果 ──
CREATE TABLE ai_results (
  image_id    INTEGER PRIMARY KEY REFERENCES images(id),
  model       TEXT,
  caption     TEXT,
  keywords    TEXT,
  ocr_text    TEXT,
  embedding   BLOB,
  analyzed_ts INTEGER
);

-- ── 人脸（预留，S2 暂不写入）──
CREATE TABLE persons (id INTEGER PRIMARY KEY, name TEXT, cover_face INTEGER, created_ts INTEGER);
CREATE TABLE faces (
  id        INTEGER PRIMARY KEY,
  image_id  INTEGER REFERENCES images(id),
  person_id INTEGER REFERENCES persons(id),
  x INTEGER, y INTEGER, w INTEGER, h INTEGER,
  embedding BLOB
);
CREATE INDEX idx_faces_image  ON faces(image_id);
CREATE INDEX idx_faces_person ON faces(person_id);

-- ── 缩略图索引（像素本体在 L2 磁盘）──
CREATE TABLE thumbs (
  image_id     INTEGER NOT NULL REFERENCES images(id),
  tier         INTEGER NOT NULL,
  cache_path   TEXT,
  bytes        INTEGER,
  generated_ts INTEGER,
  PRIMARY KEY (image_id, tier)
);

-- ── 全文检索（普通表，可 UPDATE/DELETE）──
CREATE VIRTUAL TABLE fts_images USING fts5(
  image_id UNINDEXED, filename, keywords, ocr_text, place
);

-- ── 键值设置 ──
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
"#;

/// v2：给 `images` 加 `meta_checked`。
///
/// ## 为什么需要这一列
/// 「拍摄时间回填」靠 `taken_at IS NULL` 找待补的行。但 `NULL` 有两种含义：
/// 1. **还没读过 EXIF**（新扫进来的、老库迁移来的）→ 该补
/// 2. **读过了，这文件本来就没有 EXIF 日期**（截图、下载图、扫描件）→ 补不出来
///
/// 不区分的话，第 2 类每次启动都会被当成待补，把它们的 EXIF 重新读一遍。
/// 一个一万张的库里有几千张没 EXIF，那就是每次启动几十秒的纯浪费。
///
/// `meta_checked = 1` 表示「这一行的拍摄时间已经盖棺定论」（读到或确认没有）。
///
/// ⚠️ 新库也走这条迁移（`DDL_V1` 里故意不含此列，避免「建表含列 + 迁移再加列」撞车）。
pub const DDL_V2: &str = r#"
ALTER TABLE images ADD COLUMN meta_checked INTEGER NOT NULL DEFAULT 0;
"#;

/// v3：`image_places` —— 每张图解析出的**地点**（离线城市名）。
///
/// ## 为什么单独一张表，而不是给 `images` 加列
/// 加列要动 `COLS` / `insert` / `map_row` 三处，且绝大多数图**没有 GPS**；
/// 单独一张表则天然表达「查过没有？」：
/// - **有行** = 这张图已经解析过（`place` 为 NULL 表示没有 GPS / 附近没城市）；
/// - **没有行** = 还没查过，`backfill_places` 会补。
///
/// 这样回填判据就是 `NOT IN (SELECT image_id ...)`，不需要额外的 checked 列。
pub const DDL_V3: &str = r#"
CREATE TABLE IF NOT EXISTS image_places (
  image_id INTEGER PRIMARY KEY REFERENCES images(id),
  place    TEXT,
  lat      REAL,
  lon      REAL
);
CREATE INDEX IF NOT EXISTS idx_places_name ON image_places(place);
"#;

/// v4：重建 `faces` / `persons`（2026-10-24，人脸归纳的表结构定稿）。
///
/// ## 为什么是「DROP 再建」而不是 ALTER
/// v1 里的这两张表是**预留空壳**（注释写明「S2 暂不写入」），从来没写过一行；
/// 而新结构要改列类型（x/y/w/h 从像素 INTEGER → 归一化 REAL）、加 NOT NULL，
/// SQLite 不支持改列类型 → 只能重建。空表重建零风险。
///
/// ## 每列都被实测教训过（`tools/ai/face_probe.py`，745 张真实图库）
/// - **坐标归一化 + `tier`**：像素坐标依赖缩略图档位，换档位就失效；
/// - **`model`**：换模型必须整库重建（向量空间不同不能混），没有这列就无从判定；
/// - **`score` / `quality`**：小脸、模糊脸的向量不可信，聚类前必须能过滤 ——
///   实测 1024 长边下小脸让同图负样本相似度从 0.18 抬到 0.29（噪声）；
/// - **`embedding` 存 int8**：SFace 是 128 维，int8 = 128 B/脸；f32 要 512 B，
///   10 万张脸就差 40 MB，而且聚类时反量化成 f32 只是一次 `as f32 / 127`。
///
/// ⛔ 不建「坐标唯一索引」：浮点坐标做唯一键不可靠。重跑分析由
/// [`repo::faces::replace_for_image`](crate::repo::faces::replace_for_image)
/// 先删后插保证幂等。
pub const DDL_V4: &str = r#"
DROP TABLE IF EXISTS faces;
DROP TABLE IF EXISTS persons;

CREATE TABLE persons (
  id              INTEGER PRIMARY KEY,
  name            TEXT,            -- 用户命名；NULL = 未命名（⛔ 绝不自动猜名字）
  cover_face_id   INTEGER,
  face_count      INTEGER NOT NULL DEFAULT 0,
  cluster_version TEXT,            -- 哪次聚类产出的（重聚类后可对比/回滚）
  created_ts      INTEGER
);

CREATE TABLE faces (
  id        INTEGER PRIMARY KEY,
  image_id  INTEGER NOT NULL REFERENCES images(id),
  person_id INTEGER REFERENCES persons(id),
  x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,  -- 归一化 0..1
  tier      INTEGER,               -- 在哪一档缩略图上量的
  model     TEXT NOT NULL DEFAULT '',
  score     REAL,                  -- 检测置信度
  quality   REAL,                  -- 脸宽像素（过滤小脸用）
  embedding BLOB                   -- 128 × int8，已 L2 归一化
);
CREATE INDEX idx_faces_image  ON faces(image_id);
CREATE INDEX idx_faces_person ON faces(person_id);
"#;
