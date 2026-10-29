//! 人脸聚类：**两阶段**（严阈值出核心簇 → 散脸按簇心并入）。
//!
//! ## 为什么不能单阈值
//! 社媒美颜图（磨皮/大眼/瘦脸）会让不同人的向量一起塌向「平均脸」：
//! 阈值松了张冠李戴，紧了全成单例。两阶段只用**簇心**做二次判定，
//! 比两两比较更能抵抗这种噪声。
//!
//! 算法与 `tools/ai/face_probe.py` 的 `two_stage_cluster` 同构：
//! 1. `chinese_whispers(t_core)` 聚出高纯度核心簇（≥ `min_core` 张）
//! 2. 每簇取 **medoid**（簇内平均相似度最高的那个点）当簇心
//! 3. 不属于核心簇的散脸，按 `sim(散脸, 簇心) > t_join` 并入
//!
//! ## Chinese Whispers 为什么比 DBSCAN 抗误合并
//! 每次迭代让每个点投奔「邻居标签权重**总和**最大」的标签 ——
//! 只有**持续**的强连接才能把两个大簇拉到一起，单条弱边拉不动。

/// 聚类参数。
#[derive(Debug, Clone)]
pub struct ClusterOpts {
    /// 第一阶段阈值（严）：用来找核心簇
    pub t_core: f32,
    /// 第二阶段阈值（松）：散脸并入簇心
    pub t_join: f32,
    /// 核心簇的最少张数（小于它的簇当散脸处理）
    pub min_core: usize,
    /// Chinese Whispers 迭代上限
    pub iters: usize,
}

impl Default for ClusterOpts {
    fn default() -> Self {
        // 与 face_probe.py 的默认一致（文档 §12 实测定稿）
        Self {
            t_core: 0.55,
            t_join: 0.50,
            min_core: 3,
            iters: 30,
        }
    }
}

/// 对一批**已 L2 归一化**的向量做两阶段聚类，返回每个点的簇号（从 0 连续编号）。
///
/// 单例也会各占一个簇号 —— 是否给它建「人物」由调用方按 `min_core` 决定。
pub fn two_stage_cluster(embs: &[Vec<f32>], opts: &ClusterOpts) -> Vec<i32> {
    let n = embs.len();
    if n == 0 {
        return Vec::new();
    }

    // 相似度矩阵（对称，只算上三角再镜像）。N=5000 时约 100MB —— 可接受，
    // 但**别在调用方再存一份**：这一份就够整个聚类用。
    let mut sim = vec![0f32; n * n];
    for i in 0..n {
        sim[i * n + i] = 1.0;
        for j in (i + 1)..n {
            let s = dot(&embs[i], &embs[j]);
            sim[i * n + j] = s;
            sim[j * n + i] = s;
        }
    }

    let labels = chinese_whispers(&sim, n, opts.t_core, opts.iters);

    // 统计簇大小
    let mut counts: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for &l in &labels {
        *counts.entry(l).or_insert(0) += 1;
    }
    let core: Vec<i32> = counts
        .iter()
        .filter(|(_, &c)| c >= opts.min_core)
        .map(|(&l, _)| l)
        .collect();
    if core.is_empty() {
        return renumber(&labels);
    }

    // 每个核心簇的 medoid：簇内平均相似度最高的点
    let mut medoid: Vec<(i32, usize)> = Vec::new();
    for &l in &core {
        let idx: Vec<usize> = labels
            .iter()
            .enumerate()
            .filter(|(_, &x)| x == l)
            .map(|(i, _)| i)
            .collect();
        let mut best = (idx[0], f32::MIN);
        for &a in &idx {
            let mut sum = 0.0f32;
            for &b in &idx {
                sum += sim[a * n + b];
            }
            let avg = sum / idx.len() as f32;
            if avg > best.1 {
                best = (a, avg);
            }
        }
        medoid.push((l, best.0));
    }

    // 散脸并入：只和**簇心**比，不与簇内其他点比（抗链式误合并的关键）
    let mut out = labels.clone();
    for i in 0..n {
        if counts.get(&labels[i]).copied().unwrap_or(0) >= opts.min_core {
            continue;
        }
        let mut best: Option<(i32, f32)> = None;
        for &(l, m) in &medoid {
            let w = sim[i * n + m];
            if w > opts.t_join && best.map(|(_, bw)| w > bw).unwrap_or(true) {
                best = Some((l, w));
            }
        }
        if let Some((l, _)) = best {
            out[i] = l;
        }
    }

    renumber(&out)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn renumber(labels: &[i32]) -> Vec<i32> {
    let mut map: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    let mut next = 0;
    labels
        .iter()
        .map(|&l| {
            *map.entry(l).or_insert_with(|| {
                let v = next;
                next += 1;
                v
            })
        })
        .collect()
}

/// Chinese Whispers：每轮按（伪）随机顺序，让每个点投奔邻居权重和最大的标签。
///
/// ⚠️ 顺序用固定种子的 LCG 打乱 —— 不必和 numpy 的 `default_rng(42)` 逐位一致，
/// 聚类本身是启发式的，只要「持续强连接才合并」这个性质在就行。
fn chinese_whispers(sim: &[f32], n: usize, threshold: f32, iters: usize) -> Vec<i32> {
    // 邻接：相似度过阈值且非自身
    let neigh: Vec<Vec<usize>> = (0..n)
        .map(|i| {
            (0..n)
                .filter(|&j| j != i && sim[i * n + j] > threshold)
                .collect()
        })
        .collect();

    let mut labels: Vec<i32> = (0..n as i32).collect();
    let mut order: Vec<usize> = (0..n).collect();
    let mut seed: u64 = 42;

    for _ in 0..iters {
        let mut changed = 0usize;
        // Fisher-Yates
        for i in (1..n).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            order.swap(i, j);
        }
        for &i in &order {
            if neigh[i].is_empty() {
                continue;
            }
            // 按标签累加权重
            let mut acc: std::collections::HashMap<i32, f32> = std::collections::HashMap::new();
            for &j in &neigh[i] {
                *acc.entry(labels[j]).or_insert(0.0) += sim[i * n + j];
            }
            let mut best_label = labels[i];
            let mut best_w = -1.0f32;
            for (&lb, &w) in &acc {
                if w > best_w + 1e-9 {
                    best_label = lb;
                    best_w = w;
                }
            }
            if labels[i] != best_label {
                labels[i] = best_label;
                changed += 1;
            }
        }
        if changed == 0 {
            break; // 收敛
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造 3 个簇，簇内相似 0.9、簇间 0.1。
    fn synth() -> Vec<Vec<f32>> {
        // 用 8 维单位基向量做「簇心」，簇内加点噪声
        let mut v = Vec::new();
        for c in 0..3 {
            for k in 0..4 {
                let mut e = vec![0.0f32; 8];
                e[c * 2] = 1.0;
                e[c * 2 + 1] = 0.1 * (k as f32);
                let n: f32 = e.iter().map(|x| x * x).sum::<f32>().sqrt();
                for x in &mut e {
                    *x /= n;
                }
                v.push(e);
            }
        }
        v
    }

    #[test]
    fn 三个分离的簇各归各位() {
        let embs = synth();
        let labels = two_stage_cluster(&embs, &ClusterOpts::default());
        assert_eq!(labels.len(), 12);
        // 同簇的标签必须相同
        for c in 0..3 {
            let base = labels[c * 4];
            for k in 0..4 {
                assert_eq!(labels[c * 4 + k], base, "第 {c} 簇内部被拆开了");
            }
        }
        // 不同簇必须不同
        assert_ne!(labels[0], labels[4]);
        assert_ne!(labels[0], labels[8]);
        assert_ne!(labels[4], labels[8]);
    }

    #[test]
    fn 空输入不崩() {
        assert!(two_stage_cluster(&[], &ClusterOpts::default()).is_empty());
    }

    #[test]
    fn 单点也返回一个簇() {
        let v = vec![vec![1.0f32, 0.0, 0.0]];
        assert_eq!(two_stage_cluster(&v, &ClusterOpts::default()), vec![0]);
    }
}
