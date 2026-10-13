//! PurePhotoExplorer 非破坏性编辑。
//!
//! 这里提供的是**操作栈**这套抽象（编辑步骤可序列化、可回放、可撤销）。
//!
//! ## 落盘策略（2026-10-07 变更）
//! 原先的设计是「所有编辑写入 XMP sidecar，原图永不修改」。
//! 该策略**已作废** —— 用户明确要求「覆盖原图」，实现在
//! `apps/desktop/src-tauri/src/commands/edit.rs`，直接写回原文件
//! （先编码到内存、再原子 rename，保证失败不损坏原图）。
//!
//! 本 crate 的操作栈因此暂时**不参与落盘**，保留作为未来
//! 「可撤销的多步编辑」的基础设施。判断能否写回文件的能力在
//! `pv_decode::can_encode`。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod opstack;

pub use opstack::{
    EditOp, EditStack, FileFingerprint, FlipOp, MaskRef, OpKind, Preset, RotateOp, StackValidity,
};
