//! 非破坏性编辑：操作栈（见开发文档 §4.1）。
//!
//! 编辑以可序列化操作栈的形式表达，便于撤销/重放/预设复用。
//! **注**：旧的「写入 XMP sidecar、原图永不修改」策略已于 2026-10-07 作废，
//! 详见 [`crate`] 的模块文档。
//! 批量编辑预设本身是操作栈的子集 + 参数约束。

use serde::{Deserialize, Serialize};

/// 绑定原图指纹。原图被外部修改后，操作栈失效并警告用户
/// （避免在已改动的图上叠加旧编辑）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub size: u64,
    pub mtime_ns: i128,
    /// 前 64KB 的 blake3，用于内容级校验
    pub head_hash: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    Crop,
    Rotate,
    Flip,
    Exposure,
    Contrast,
    Saturation,
    Temperature,
    Tint,
    Curve,
    HslAdjust,
    Sharpen,
    Denoise,
    Monochrome,
    MaskErase,
    // RAW 开发流程相关（二期）
    WhiteBalance,
    Demosaic,
    LensCorrection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RotateOp {
    /// 顺时针 90/180/270
    Cw90,
    Cw180,
    Cw270,
    /// 按 EXIF 方向自动
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlipOp {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskRef {
    /// 蒙版图路径（灰度，0=完全编辑，255=完全不编辑）
    pub path: String,
    /// 羽化半径（px）
    pub feather_px: u32,
    pub opacity: f32,
}

/// 单个编辑操作
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditOp {
    pub op: OpKind,
    /// 操作参数，按 op 类型解释
    pub params: serde_json::Value,
    /// 可选蒙版
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<MaskRef>,
}

impl EditOp {
    pub fn new(op: OpKind, params: serde_json::Value) -> Self {
        Self { op, params, mask: None }
    }

    pub fn with_mask(mut self, mask: MaskRef) -> Self {
        self.mask = Some(mask);
        self
    }

    /// 几何操作（裁剪/旋转/翻转）需在调色之前执行
    pub fn is_geometric(&self) -> bool {
        matches!(self.op, OpKind::Crop | OpKind::Rotate | OpKind::Flip)
    }
}

/// 有序操作栈。栈顶为最后应用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditStack {
    /// 绑定原图指纹
    pub base: FileFingerprint,
    /// 有序，索引 0 最先应用
    pub ops: Vec<EditOp>,
    pub updated_at: i64,
}

impl EditStack {
    pub fn new(base: FileFingerprint) -> Self {
        Self {
            base,
            ops: Vec::new(),
            updated_at: 0,
        }
    }

    /// 追加操作
    pub fn push(&mut self, op: EditOp) {
        self.ops.push(op);
    }

    /// 撤销最后一次操作（弹栈）
    pub fn pop(&mut self) -> Option<EditOp> {
        self.ops.pop()
    }

    /// 调整栈中某项参数（滑块拖动，不新增条目）
    pub fn update_params(&mut self, index: usize, params: serde_json::Value) -> bool {
        match self.ops.get_mut(index) {
            Some(op) => {
                op.params = params;
                true
            }
            None => false,
        }
    }

    /// 移除指定操作
    pub fn remove(&mut self, index: usize) -> bool {
        index < self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// 校验：原图是否已被外部修改
    pub fn validate_against(&self, current: &FileFingerprint) -> StackValidity {
        if self.base.size != current.size {
            return StackValidity::SourceChanged;
        }
        if self.base.mtime_ns != current.mtime_ns {
            return StackValidity::SourceChanged;
        }
        if self.base.head_hash != current.head_hash {
            return StackValidity::SourceChanged;
        }
        StackValidity::Valid
    }

    /// 几何操作必须排在前，调色在后。
    /// 顺序错误会导致裁剪区域与调色范围错位。
    pub fn normalized(&self) -> Vec<&EditOp> {
        let mut out: Vec<&EditOp> = self.ops.iter().filter(|o| o.is_geometric()).collect();
        out.extend(self.ops.iter().filter(|o| !o.is_geometric()));
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackValidity {
    Valid,
    /// 原图已被外部修改，操作栈不可直接套用
    SourceChanged,
}

/// 批量编辑预设（FR-2.6）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// 预设内的操作模板
    pub ops: Vec<EditOp>,
    /// 是否应用到原图已存在的编辑栈之上（true）或替换（false）
    pub append: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fp() -> FileFingerprint {
        FileFingerprint {
            size: 1000,
            mtime_ns: 12345,
            head_hash: [7u8; 16],
        }
    }

    #[test]
    fn 空栈可用() {
        let mut s = EditStack::new(fp());
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn 推入与弹出() {
        let mut s = EditStack::new(fp());
        s.push(EditOp::new(OpKind::Exposure, json!({ "ev": 0.5 })));
        s.push(EditOp::new(OpKind::Contrast, json!({ "amount": 10.0 })));
        assert_eq!(s.len(), 2);

        let last = s.pop().unwrap();
        assert_eq!(last.op, OpKind::Contrast, "后进先出");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn 滑块调整不新增条目() {
        let mut s = EditStack::new(fp());
        s.push(EditOp::new(OpKind::Exposure, json!({ "ev": 0.0 })));
        assert!(s.update_params(0, json!({ "ev": 1.2 })));
        assert_eq!(s.len(), 1, "改参数不应增加栈长度");
        assert_eq!(s.ops[0].params, json!({ "ev": 1.2 }));
    }

    #[test]
    fn 越界索引调整返回false() {
        let mut s = EditStack::new(fp());
        assert!(!s.update_params(5, json!({})));
    }

    #[test]
    fn 原图未变则栈有效() {
        let s = EditStack::new(fp());
        assert_eq!(s.validate_against(&fp()), StackValidity::Valid);
    }

    #[test]
    fn 原图大小变化则栈失效() {
        let s = EditStack::new(fp());
        let changed = FileFingerprint { size: 2000, ..fp() };
        assert_eq!(s.validate_against(&changed), StackValidity::SourceChanged);
    }

    #[test]
    fn 原图内容变化则栈失效() {
        let s = EditStack::new(fp());
        let changed = FileFingerprint { head_hash: [9u8; 16], ..fp() };
        assert_eq!(s.validate_against(&changed), StackValidity::SourceChanged);
    }

    #[test]
    fn 几何操作自动排到调色之前() {
        let mut s = EditStack::new(fp());
        // 故意乱序：先调色 → 后裁剪 → 再调色
        s.push(EditOp::new(OpKind::Exposure, json!({ "ev": 1.0 })));
        s.push(EditOp::new(OpKind::Crop, json!({ "x": 0, "y": 0 })));
        s.push(EditOp::new(OpKind::Saturation, json!({ "amount": 20.0 })));

        let norm = s.normalized();
        assert_eq!(norm.len(), 3);
        assert!(norm[0].is_geometric(), "裁剪应排到最前");
        assert!(!norm[1].is_geometric(), "调色应在几何之后");
        assert!(!norm[2].is_geometric());
    }

    #[test]
    fn 多个几何操作保持原有相对顺序() {
        let mut s = EditStack::new(fp());
        s.push(EditOp::new(OpKind::Crop, json!({ "x": 0, "y": 0 })));
        s.push(EditOp::new(OpKind::Exposure, json!({ "ev": 1.0 })));
        s.push(EditOp::new(OpKind::Rotate, json!({ "deg": 90 })));
        s.push(EditOp::new(OpKind::Flip, json!({ "axis": "h" })));

        let norm = s.normalized();
        // 几何操作内部顺序必须稳定：裁剪 → 旋转 → 翻转
        assert_eq!(norm[0].op, OpKind::Crop);
        assert_eq!(norm[1].op, OpKind::Rotate);
        assert_eq!(norm[2].op, OpKind::Flip);
        assert_eq!(norm[3].op, OpKind::Exposure, "调色应在所有几何操作之后");
    }

    #[test]
    fn 操作栈可序列化往返() {
        let mut s = EditStack::new(fp());
        s.push(EditOp::new(OpKind::Temperature, json!({ "delta": 300.0 })));
        let json = serde_json::to_string(&s).unwrap();
        let back: EditStack = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back, "XMP sidecar 依赖序列化往返");
    }

    #[test]
    fn 带蒙版的操作可序列化往返() {
        let mut s = EditStack::new(fp());
        let op = EditOp::new(OpKind::MaskErase, json!({ "strength": 0.5 })).with_mask(MaskRef {
            path: "mask.png".into(),
            feather_px: 32,
            opacity: 0.8,
        });
        s.push(op);
        let back: EditStack = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.ops[0].mask.as_ref().unwrap().feather_px, 32);
    }

    #[test]
    fn 无蒙版时不写入空字段() {
        let op = EditOp::new(OpKind::Exposure, json!({ "ev": 0.0 }));
        let s = serde_json::to_string(&op).unwrap();
        assert!(!s.contains("mask"), "None 蒙版不应写入，减小 XMP 体积");
    }
}
