//! 智能图库**规则 DSL**（搜索语法子集，见手册 §9.2）。
//!
//! 空格分隔、**AND** 组合：
//!
//! | 写法 | 含义 |
//! |---|---|
//! | `tag:风景` | 含该标签 |
//! | `rating:>=4` / `rating:4` | 评分下限 |
//! | `date:2024` / `date:2024-05` / `date:2024-05-01..2024-05-31` | 拍摄时间（含区间） |
//! | `type:raw` / `type:image` | 是否 RAW |
//! | `ext:jpg` / `ext:cr3` | 格式 |
//! | 其它裸词 | 全文关键词（交给 FTS，与关键词/OCR/地点一起匹配） |

use pv_index::repo::query::ImageFilter;

use crate::{LibError, Result};

/// 解析后的规则：**结构化过滤** + **全文关键词**。
#[derive(Debug, Clone, Default)]
pub struct Rule {
    pub filter: ImageFilter,
    pub keywords: Vec<String>,
}

/// 解析规则文本。
pub fn parse_rule(text: &str) -> Result<Rule> {
    let mut r = Rule::default();
    for tok in text.split_whitespace() {
        let Some((key, val)) = tok.split_once(':') else {
            r.keywords.push(tok.to_string());
            continue;
        };
        match key {
            "tag" => r.filter.tags_any.push(val.to_string()),
            "rating" => r.filter.min_rating = Some(parse_rating(val)?),
            "type" => match val {
                "raw" => r.filter.is_raw = Some(true),
                "image" => r.filter.is_raw = Some(false),
                other => return Err(LibError::Rule(format!("type 只支持 raw / image，收到 {other}"))),
            },
            "ext" => {
                let f = ext_to_format(val)
                    .ok_or_else(|| LibError::Rule(format!("不认识的扩展名：{val}")))?;
                r.filter.formats.push(f.to_string());
            }
            "date" => {
                let (from, to) = parse_date_range(val)?;
                r.filter.taken_from = from;
                r.filter.taken_to = to;
            }
            // 未知前缀当全文词处理（不报错，避免"写错一个词就整条规则失效"）
            _ => r.keywords.push(tok.to_string()),
        }
    }
    Ok(r)
}

fn parse_rating(v: &str) -> Result<i64> {
    let s = v.strip_prefix(">=").unwrap_or(v);
    s.parse::<i64>()
        .map_err(|_| LibError::Rule(format!("评分应为整数：{v}")))
}

/// 扩展名 → 索引里的格式标签（与 `pv-scan` 的 `indexable_tag` 对齐）。
fn ext_to_format(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        "gif" => "gif",
        "bmp" => "bmp",
        "webp" => "webp",
        "tif" | "tiff" => "tiff",
        "avif" => "avif",
        "heic" | "heif" => "heif",
        "psd" => "psd",
        "cr2" => "cr2",
        "cr3" => "cr3",
        "nef" => "nef",
        "arw" => "arw",
        "raf" => "raf",
        "dng" => "dng",
        "pef" => "pef",
        "rw2" => "rw2",
        "orf" => "orf",
        "iiq" => "iiq",
        _ => return None,
    })
}

/// `YYYY` / `YYYY-MM` / `YYYY-MM-DD` / `A..B` → epoch 秒**闭区间**。
fn parse_date_range(v: &str) -> Result<(Option<i64>, Option<i64>)> {
    if let Some((a, b)) = v.split_once("..") {
        let from = if a.trim().is_empty() {
            None
        } else {
            Some(parse_bound(a)?)
        };
        let to = if b.trim().is_empty() {
            None
        } else {
            Some(parse_bound(b)? + 86_400 - 1) // 含当天最后一秒
        };
        return Ok((from, to));
    }
    let from = parse_bound(v)?;
    let to_excl = match v.matches('-').count() {
        0 => epoch(parse_i(v, "年")? + 1, 1, 1),
        1 => {
            let (y, m) = split_ym(v)?;
            if m == 12 {
                epoch(y + 1, 1, 1)
            } else {
                epoch(y, m + 1, 1)
            }
        }
        _ => from + 86_400,
    };
    Ok((Some(from), Some(to_excl - 1)))
}

fn parse_i(s: &str, what: &str) -> Result<i64> {
    s.parse::<i64>()
        .map_err(|_| LibError::Rule(format!("{what}不是整数：{s}")))
}

fn split_ym(v: &str) -> Result<(i64, u32)> {
    let (y, m) = v
        .split_once('-')
        .ok_or_else(|| LibError::Rule(format!("日期格式错：{v}")))?;
    Ok((parse_i(y, "年")?, parse_i(m, "月")? as u32))
}

/// 解析 `YYYY` / `YYYY-MM` / `YYYY-MM-DD` 的**起点**。
fn parse_bound(v: &str) -> Result<i64> {
    let parts: Vec<&str> = v.split('-').collect();
    match parts.as_slice() {
        [y] => Ok(epoch(parse_i(y, "年")?, 1, 1)),
        [y, m] => Ok(epoch(parse_i(y, "年")?, parse_i(m, "月")? as u32, 1)),
        [y, m, d] => Ok(epoch(
            parse_i(y, "年")?,
            parse_i(m, "月")? as u32,
            parse_i(d, "日")? as u32,
        )),
        _ => Err(LibError::Rule(format!("日期格式错：{v}"))),
    }
}

/// 公历 → 天数（Howard Hinnant 算法），再 ×86400 得 epoch 秒。
fn epoch(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析标签与评分() {
        let r = parse_rule("tag:风景 rating:>=4").unwrap();
        assert_eq!(r.filter.tags_any, vec!["风景"]);
        assert_eq!(r.filter.min_rating, Some(4));
        assert!(r.keywords.is_empty());
    }

    #[test]
    fn 解析格式与RAW() {
        let r = parse_rule("ext:jpg type:raw").unwrap();
        assert_eq!(r.filter.formats, vec!["jpeg"]);
        assert_eq!(r.filter.is_raw, Some(true));
    }

    #[test]
    fn 裸词进全文关键词() {
        let r = parse_rule("海边 日落").unwrap();
        assert_eq!(r.keywords, vec!["海边", "日落"]);
    }

    #[test]
    fn 未知前缀当全文词() {
        let r = parse_rule("foo:bar").unwrap();
        assert_eq!(r.keywords, vec!["foo:bar"]);
    }

    #[test]
    fn 日期整年为闰年366天() {
        let r = parse_rule("date:2024").unwrap();
        let from = r.filter.taken_from.unwrap();
        let to = r.filter.taken_to.unwrap();
        assert_eq!(to - from + 1, 366 * 86_400, "2024 是闰年");
    }

    #[test]
    fn 日期整月与区间() {
        let r = parse_rule("date:2024-05").unwrap();
        assert_eq!(
            r.filter.taken_to.unwrap() - r.filter.taken_from.unwrap() + 1,
            31 * 86_400
        );

        let r = parse_rule("date:2024-05-01..2024-05-31").unwrap();
        assert_eq!(
            r.filter.taken_to.unwrap() - r.filter.taken_from.unwrap() + 1,
            31 * 86_400
        );

        // 开区间
        let r = parse_rule("date:2024-01-01..").unwrap();
        assert!(r.filter.taken_from.is_some());
        assert!(r.filter.taken_to.is_none());
    }

    #[test]
    fn 非法规则报错() {
        assert!(parse_rule("type:music").is_err(), "type 只支持 raw/image");
        assert!(parse_rule("rating:x").is_err());
        assert!(parse_rule("ext:xyz").is_err(), "不认识的扩展名");
    }
}
