use indexmap::IndexMap;
use regex::{Captures, Regex};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct EvalContext<'a> {
    pub env: &'a IndexMap<String, String>,
    pub matrix: &'a IndexMap<String, String>,
    pub secrets: &'a IndexMap<String, String>,
    pub github: &'a IndexMap<String, String>,
    pub runner_os: &'a str,
}

pub fn interpolate(input: &str, ctx: &EvalContext<'_>) -> String {
    static EXPR_RE: OnceLock<Regex> = OnceLock::new();
    let re = EXPR_RE.get_or_init(|| Regex::new(r"\$\{\{\s*(.*?)\s*\}\}").expect("valid regex"));

    re.replace_all(input, |caps: &Captures<'_>| {
        resolve_value(caps.get(1).map(|m| m.as_str()).unwrap_or_default(), ctx)
    })
    .into_owned()
}

pub fn eval_condition(condition: Option<&str>, ctx: &EvalContext<'_>) -> bool {
    let Some(condition) = condition else {
        return true;
    };

    let expr = normalize_expression(condition);
    eval_bool(expr, ctx).unwrap_or(true)
}

fn normalize_expression(input: &str) -> &str {
    let trimmed = input.trim();
    if let Some(inner) = trimmed
        .strip_prefix("${{")
        .and_then(|value| value.strip_suffix("}}"))
    {
        return inner.trim();
    }

    trimmed
}

fn eval_bool(expr: &str, ctx: &EvalContext<'_>) -> Option<bool> {
    let expr = strip_outer_parens(expr.trim());
    if expr.is_empty() {
        return Some(true);
    }

    if let Some(rest) = expr.strip_prefix('!') {
        return eval_bool(rest, ctx).map(|value| !value);
    }

    if let Some((left, right)) = split_top_level(expr, "||") {
        return Some(eval_bool(left, ctx).unwrap_or(true) || eval_bool(right, ctx).unwrap_or(true));
    }

    if let Some((left, right)) = split_top_level(expr, "&&") {
        return Some(eval_bool(left, ctx).unwrap_or(true) && eval_bool(right, ctx).unwrap_or(true));
    }

    for op in ["!=", "=="] {
        if let Some((left, right)) = split_top_level(expr, op) {
            let left = resolve_value(left, ctx);
            let right = resolve_value(right, ctx);
            return Some(if op == "==" {
                left == right
            } else {
                left != right
            });
        }
    }

    if expr.eq_ignore_ascii_case("true") || expr == "always()" || expr == "success()" {
        return Some(true);
    }

    if expr.eq_ignore_ascii_case("false") || expr == "failure()" || expr == "cancelled()" {
        return Some(false);
    }

    if let Some(args) = call_args(expr, "startsWith")
        && args.len() == 2
    {
        return Some(resolve_value(args[0], ctx).starts_with(&resolve_value(args[1], ctx)));
    }

    if let Some(args) = call_args(expr, "contains")
        && args.len() == 2
    {
        return Some(resolve_value(args[0], ctx).contains(&resolve_value(args[1], ctx)));
    }

    let resolved = resolve_value(expr, ctx);
    Some(truthy(&resolved))
}

pub fn resolve_value(input: &str, ctx: &EvalContext<'_>) -> String {
    let expr = normalize_expression(input).trim();
    if let Some(value) = unquote(expr) {
        return value.to_string();
    }

    if expr.eq_ignore_ascii_case("true") || expr.eq_ignore_ascii_case("false") {
        return expr.to_ascii_lowercase();
    }

    if let Some(key) = expr.strip_prefix("matrix.") {
        return ctx.matrix.get(key).cloned().unwrap_or_default();
    }

    if let Some(key) = expr.strip_prefix("env.") {
        return ctx.env.get(key).cloned().unwrap_or_default();
    }

    if let Some(key) = expr.strip_prefix("secrets.") {
        return ctx.secrets.get(key).cloned().unwrap_or_default();
    }

    if let Some(key) = expr.strip_prefix("github.") {
        return ctx.github.get(key).cloned().unwrap_or_default();
    }

    match expr {
        "runner.os" => ctx.runner_os.to_string(),
        "runner.temp" => "/tmp".to_string(),
        "github.workspace" => ctx
            .github
            .get("workspace")
            .cloned()
            .unwrap_or_else(|| "/workspace".to_string()),
        _ => ctx
            .env
            .get(expr)
            .cloned()
            .unwrap_or_else(|| expr.to_string()),
    }
}

fn unquote(input: &str) -> Option<&str> {
    input
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            input
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
}

fn truthy(input: &str) -> bool {
    let input = input.trim();
    !input.is_empty() && input != "false" && input != "0"
}

fn strip_outer_parens(input: &str) -> &str {
    let mut value = input;
    loop {
        let Some(inner) = value
            .strip_prefix('(')
            .and_then(|candidate| candidate.strip_suffix(')'))
        else {
            return value;
        };

        if balanced(inner) {
            value = inner.trim();
        } else {
            return value;
        }
    }
}

fn split_top_level<'a>(input: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0usize;
    let mut in_quote: Option<char> = None;
    let bytes = input.as_bytes();
    let op_bytes = op.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        let ch = input[index..].chars().next()?;
        if let Some(quote) = in_quote {
            if ch == quote {
                in_quote = None;
            }
            index += ch.len_utf8();
            continue;
        }

        match ch {
            '\'' | '"' => in_quote = Some(ch),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {
                if depth == 0 && bytes[index..].starts_with(op_bytes) {
                    return Some((input[..index].trim(), input[index + op.len()..].trim()));
                }
            }
        }

        index += ch.len_utf8();
    }

    None
}

fn call_args<'a>(input: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let prefix = format!("{name}(");
    let inner = input.strip_prefix(&prefix)?.strip_suffix(')')?;
    Some(split_args(inner))
}

fn split_args(input: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut in_quote: Option<char> = None;
    let mut start = 0usize;

    for (index, ch) in input.char_indices() {
        if let Some(quote) = in_quote {
            if ch == quote {
                in_quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => in_quote = Some(ch),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(input[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }

    args.push(input[start..].trim());
    args
}

fn balanced(input: &str) -> bool {
    let mut depth = 0isize;
    let mut in_quote: Option<char> = None;
    for ch in input.chars() {
        if let Some(quote) = in_quote {
            if ch == quote {
                in_quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => in_quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }

        if depth < 0 {
            return false;
        }
    }

    depth == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestContext {
        env: IndexMap<String, String>,
        matrix: IndexMap<String, String>,
        secrets: IndexMap<String, String>,
        github: IndexMap<String, String>,
    }

    fn ctx() -> TestContext {
        let mut env = IndexMap::new();
        env.insert("BRANCH".to_string(), "main".to_string());
        let mut matrix = IndexMap::new();
        matrix.insert("node".to_string(), "20".to_string());
        let mut secrets = IndexMap::new();
        secrets.insert("TOKEN".to_string(), "secret".to_string());
        let mut github = IndexMap::new();
        github.insert("ref".to_string(), "refs/heads/main".to_string());
        TestContext {
            env,
            matrix,
            secrets,
            github,
        }
    }

    #[test]
    fn interpolates_context_values() {
        let fixture = ctx();
        let ctx = EvalContext {
            env: &fixture.env,
            matrix: &fixture.matrix,
            secrets: &fixture.secrets,
            github: &fixture.github,
            runner_os: "Linux",
        };

        assert_eq!(
            interpolate("node=${{ matrix.node }} token=${{ secrets.TOKEN }}", &ctx),
            "node=20 token=secret"
        );
    }

    #[test]
    fn evaluates_basic_conditions() {
        let fixture = ctx();
        let ctx = EvalContext {
            env: &fixture.env,
            matrix: &fixture.matrix,
            secrets: &fixture.secrets,
            github: &fixture.github,
            runner_os: "Linux",
        };

        assert!(eval_condition(
            Some("${{ env.BRANCH == 'main' && startsWith(github.ref, 'refs/heads/') }}"),
            &ctx
        ));
        assert!(!eval_condition(Some("matrix.node == '18'"), &ctx));
    }
}
