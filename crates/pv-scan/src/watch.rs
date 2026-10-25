//! 文件系统**实时监听**：`notify` 原始事件 → **去抖聚合** → 一批「需重扫的目录」。
//!
//! 分工（为了可测、不 flaky）：
//! - [`Debouncer`] 是**纯逻辑**（用调用方传入的 `Instant`，可确定性单测）；
//! - [`Watcher`] 只是薄壳：把 `notify` 事件喂给 `Debouncer`，静默 `debounce` 后投递一批。
//!
//! ⚠️ 只监听**已加入图库的文件夹**（非递归），因此**无需管理员权限**，
//! 也不用 USN/MFT（见手册 §5.4）。

use crate::Result;
// 匿名导入：`notify::Watcher` 提供 `.watch()`；本模块自己的结构体也叫 `Watcher`，故不引入名字。
use notify::Watcher as _;
use pv_index::IndexError;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// 一批（去抖后）的变更：**需要重扫的目录集合**。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeBatch {
    pub dirs: BTreeSet<PathBuf>,
}

impl ChangeBatch {
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
    pub fn len(&self) -> usize {
        self.dirs.len()
    }
}

/// 取一个变更路径所属的目录（要重扫的就是它）。
pub fn dir_of(path: &Path) -> Option<PathBuf> {
    path.parent().map(|p| p.to_path_buf())
}

/// 去抖聚合器（纯逻辑）。
#[derive(Debug, Default)]
pub struct Debouncer {
    pending: BTreeSet<PathBuf>,
    last_event: Option<Instant>,
}

impl Debouncer {
    /// 记一次变更：`path` 的**所属目录**进待重扫集合，并重置静默计时。
    pub fn on_path(&mut self, path: &Path, now: Instant) {
        if let Some(d) = dir_of(path) {
            self.pending.insert(d);
        }
        self.last_event = Some(now);
    }

    /// 距最后一次事件的静默时长；从未有事件则为 `None`。
    pub fn silence(&self, now: Instant) -> Option<Duration> {
        self.last_event.map(|t| now.saturating_duration_since(t))
    }

    /// 若已静默 ≥ `debounce`，取出一批并清空；否则返回 `None`。
    ///
    /// 注意：**无论如何都会清掉静默计时**，避免"待重扫集合为空但计时未清"造成的忙等。
    pub fn take_ready(&mut self, now: Instant, debounce: Duration) -> Option<ChangeBatch> {
        let ready = self.silence(now).map(|el| el >= debounce).unwrap_or(false);
        if !ready {
            return None;
        }
        self.last_event = None;
        if self.pending.is_empty() {
            return None;
        }
        Some(ChangeBatch {
            dirs: std::mem::take(&mut self.pending),
        })
    }
}

fn watch_err(msg: impl Into<String>) -> IndexError {
    IndexError::Io(std::io::Error::other(msg.into()))
}

/// 监听一组目录；变更经去抖后以 [`ChangeBatch`] 投递。
pub struct Watcher {
    /// 持有它以保证底层监听存活（drop 时会停止并断开事件通道）。
    _watcher: notify::RecommendedWatcher,
    rx: Receiver<ChangeBatch>,
}

impl Watcher {
    /// 开始监听（`RecursiveMode::NonRecursive`，每个目录一个句柄）。
    pub fn spawn(dirs: &[PathBuf], debounce: Duration) -> Result<Self> {
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = raw_tx.send(res);
        })
        .map_err(|e| watch_err(format!("初始化文件监听失败：{e}")))?;

        for d in dirs {
            watcher
                .watch(d, notify::RecursiveMode::NonRecursive)
                .map_err(|e| watch_err(format!("监听 {} 失败：{e}", d.display())))?;
        }

        let (tx, rx) = mpsc::channel::<ChangeBatch>();
        std::thread::spawn(move || {
            let mut deb = Debouncer::default();
            loop {
                // 有事件在计时 → 等到静默期满；否则长阻塞。
                let timeout = match deb.silence(Instant::now()) {
                    Some(el) if el < debounce => debounce - el,
                    Some(_) => Duration::ZERO,
                    None => Duration::from_secs(3600),
                };
                match raw_rx.recv_timeout(timeout) {
                    Ok(Ok(ev)) => {
                        let now = Instant::now();
                        for p in &ev.paths {
                            deb.on_path(p, now);
                        }
                    }
                    Ok(Err(_)) => {} // 单个事件错误，忽略
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(batch) = deb.take_ready(Instant::now(), debounce) {
                            if tx.send(batch).is_err() {
                                break; // 接收端已关闭
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// 非阻塞取一批（UI 轮询用）。
    pub fn try_recv(&self) -> Option<ChangeBatch> {
        self.rx.try_recv().ok()
    }

    /// 阻塞取一批（直到有变更或通道关闭）。
    pub fn recv(&self) -> Option<ChangeBatch> {
        self.rx.recv().ok()
    }

    /// 限时取一批。
    pub fn recv_timeout(&self, timeout: Duration) -> Option<ChangeBatch> {
        self.rx.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 取所属目录() {
        assert_eq!(dir_of(Path::new(r"D:\a\b\x.jpg")), Some(PathBuf::from(r"D:\a\b")));
        assert_eq!(dir_of(Path::new("x.jpg")), Some(PathBuf::from("")));
    }

    #[test]
    fn 去抖聚合_确定性() {
        let t0 = Instant::now();
        let d3 = Duration::from_millis(300);
        let mut deb = Debouncer::default();

        // 无事件时不产出
        assert!(deb.take_ready(t0 + Duration::from_secs(1), d3).is_none());

        deb.on_path(Path::new(r"D:\lib\a\x.jpg"), t0);
        deb.on_path(Path::new(r"D:\lib\a\y.jpg"), t0 + Duration::from_millis(100));
        // 静默不足（最后一次在 +100，现在 +250，差 150 < 300）
        assert!(deb.take_ready(t0 + Duration::from_millis(250), d3).is_none());

        // 另一目录也在同一批里
        deb.on_path(Path::new(r"D:\lib\b\z.png"), t0 + Duration::from_millis(200));
        // 静默期满（+200 → +600，差 400 ≥ 300）
        let batch = deb.take_ready(t0 + Duration::from_millis(600), d3).unwrap();
        assert_eq!(batch.len(), 2, "应聚合出两个目录");
        assert!(batch.dirs.contains(Path::new(r"D:\lib\a")));
        assert!(batch.dirs.contains(Path::new(r"D:\lib\b")));

        // 取走后不重复产出
        assert!(deb.take_ready(t0 + Duration::from_millis(700), d3).is_none());
    }

    #[test]
    fn 空集合不会忙等() {
        // 只喂一个"没有父目录"的路径：待重扫集合为空，但要清掉计时
        let t0 = Instant::now();
        let d3 = Duration::from_millis(100);
        let mut deb = Debouncer::default();
        deb.on_path(Path::new(""), t0);
        assert!(deb.take_ready(t0 + Duration::from_millis(200), d3).is_none());
        // 计时已清 → 下一次也是 None（不会反复 ready）
        assert!(deb.take_ready(t0 + Duration::from_millis(300), d3).is_none());
        assert_eq!(deb.silence(t0 + Duration::from_millis(300)), None);
    }
}
