use winnow::ascii::multispace0;
use winnow::combinator::opt;
use winnow::prelude::*;
use winnow::token::{literal, take_while};

use crate::wfg_ast::{Rate, RateUnit};

// ---------------------------------------------------------------------------
// Whitespace & comments (// style for .wfg)
// ---------------------------------------------------------------------------

/// Skip whitespace and `// ...` line comments.
pub fn ws_skip(input: &mut &str) -> ModalResult<()> {
    loop {
        let _ = multispace0.parse_next(input)?;
        if opt(literal("//")).parse_next(input)?.is_some() {
            let _ = take_while(0.., |c: char| c != '\n').parse_next(input)?;
        } else {
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rate: NUMBER "/" ("s"|"m"|"h")
// ---------------------------------------------------------------------------

pub fn rate(input: &mut &str) -> ModalResult<Rate> {
    let num = wf_lang::parse_utils::number_literal(input)?;
    let count = num as u64;
    literal("/").parse_next(input)?;
    let unit = winnow::combinator::alt((
        literal("s").value(RateUnit::PerSecond),
        literal("m").value(RateUnit::PerMinute),
        literal("h").value(RateUnit::PerHour),
    ))
    .parse_next(input)?;
    Ok(Rate { count, unit })
}

// ---------------------------------------------------------------------------
// 结构化 JSON 值：`{...}` / `[...]`
// ---------------------------------------------------------------------------

/// 消费一个**平衡**的 JSON 容器（`{...}` 或 `[...]`，尊重字符串与转义），
/// 交给 `serde_json` 解析。
///
/// 用整段扫描而不是递归下降：容器内部是纯 JSON（不是 WFG 语法），逐字符记录
/// 深度与字符串状态最简单，也避免为正则嵌套再写一套 CST。
pub fn json_container(input: &mut &str) -> ModalResult<serde_json::Value> {
    let s: &str = input;
    let first = s.chars().next();
    if !matches!(first, Some('{') | Some('[')) {
        return Err(winnow::error::ErrMode::Backtrack(
            winnow::error::ContextError::new(),
        ));
    }

    let mut depth = 0_i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut end: Option<usize> = None;

    for (idx, c) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(idx + c.len_utf8());
                    break;
                }
                if depth < 0 {
                    break;
                }
            }
            _ => {}
        }
    }

    let Some(end) = end else {
        return Err(winnow::error::ErrMode::Backtrack(
            winnow::error::ContextError::new(),
        ));
    };

    let text = &s[..end];
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| winnow::error::ErrMode::Backtrack(winnow::error::ContextError::new()))?;
    *input = &s[end..];
    Ok(value)
}
