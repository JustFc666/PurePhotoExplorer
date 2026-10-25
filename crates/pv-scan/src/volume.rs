//! **卷身份**：把一个绝对路径解析到它所在的「卷」。
//!
//! 关键：以 **Volume GUID**（`\\?\Volume{GUID}\`）为身份，**不用盘符**——
//! 移动硬盘换盘符（E:→F:）后凭 GUID 仍能认出来（手册 §5.5）。
//!
//! 实现拆成 trait + 平台实现，好处是**递归扫描逻辑可以用假解析器单测**，
//! 不依赖真实盘符/挂载点。

use crate::Result;
use std::path::{Path, PathBuf};

/// 一个卷的稳定信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// `\\?\Volume{GUID}\`（跨插拔稳定）
    pub volume_guid: String,
    pub serial: Option<i64>,
    pub label: Option<String>,
    pub fs: Option<String>,
    pub removable: bool,
    /// 挂载点（如 `C:\`）；**会变**，只用于拼绝对路径。
    pub mount_path: PathBuf,
}

/// 把路径解析到卷。
///
/// 要求 `Send + Sync`：它会随 `Store` 跨线程共享（Tauri 命令在多个线程执行）。
pub trait VolumeResolver: Send + Sync {
    /// 解析失败或无对应卷时返回 `Ok(None)`。
    fn resolve(&self, path: &Path) -> Result<Option<VolumeInfo>>;
}

/// 相对卷根的路径（**统一用 `/` 分隔**，卷根为 `""`）。
///
/// 归一化分隔符是为了让 `rel_path` 在跨平台/跨工具下一致，
/// 也让它能直接进缓存键（缓存键同样不含盘符）。
pub fn relative_to_mount(mount: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(mount).ok()?;
    if rel.as_os_str().is_empty() {
        return Some(String::new());
    }
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

#[cfg(windows)]
mod win {
    use super::{VolumeInfo, VolumeResolver};
    use crate::Result;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    fn wide(s: &Path) -> Vec<u16> {
        s.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        OsString::from_wide(&buf[..len]).to_string_lossy().into_owned()
    }

    /// Windows 实现：`GetVolumePathNameW` → `GetVolumeNameForVolumeMountPointW` → `GetVolumeInformationW`。
    pub struct WindowsVolumeResolver;

    impl VolumeResolver for WindowsVolumeResolver {
        fn resolve(&self, path: &Path) -> Result<Option<VolumeInfo>> {
            let wpath = wide(path);

            // 1) 路径 → 挂载点（如 "C:\"）
            let mut mount = vec![0u16; 260];
            let ok = unsafe {
                GetVolumePathNameW(wpath.as_ptr(), mount.as_mut_ptr(), mount.len() as u32)
            };
            if ok == 0 {
                return Ok(None);
            }
            let mount_path = PathBuf::from(from_wide(&mount));
            let wmount = wide(&mount_path);

            // 2) 挂载点 → 卷 GUID 路径（"\\?\Volume{GUID}\"）
            let mut vname = vec![0u16; 260];
            let ok = unsafe {
                GetVolumeNameForVolumeMountPointW(
                    wmount.as_ptr(),
                    vname.as_mut_ptr(),
                    vname.len() as u32,
                )
            };
            if ok == 0 {
                return Ok(None);
            }
            let volume_guid = from_wide(&vname);

            // 3) 卷信息（序列号 / 卷标 / 文件系统）
            let mut serial: u32 = 0;
            let mut label = vec![0u16; 260];
            let mut fs = vec![0u16; 260];
            let ok = unsafe {
                GetVolumeInformationW(
                    wmount.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    &mut serial,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs.as_mut_ptr(),
                    fs.len() as u32,
                )
            };
            let (label, fs, serial) = if ok != 0 {
                (Some(from_wide(&label)), Some(from_wide(&fs)), Some(serial as i64))
            } else {
                (None, None, None)
            };

            // 4) 是否可移动（GetDriveTypeW：2 = DRIVE_REMOVABLE）
            let removable = unsafe { GetDriveTypeW(wmount.as_ptr()) } == 2;

            Ok(Some(VolumeInfo {
                volume_guid,
                serial,
                label,
                fs,
                removable,
                mount_path,
            }))
        }
    }
}

#[cfg(windows)]
pub use win::WindowsVolumeResolver;
