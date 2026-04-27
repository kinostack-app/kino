#![allow(dead_code)] // Used by indexer engine in later phases

use std::collections::HashMap;

use chrono::Datelike;

/// Context provided to template rendering.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub config: HashMap<String, String>,
    pub query: SearchQuery,
    pub result: HashMap<String, String>,
}

/// Search parameters passed into a template.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub q: String,
    pub keywords: String,
    pub season: Option<String>,
    pub ep: Option<String>,
    pub imdbid: Option<String>,
    pub tmdbid: Option<String>,
    pub tvdbid: Option<String>,
    pub year: Option<String>,
    pub categories: Vec<String>,
}

impl TemplateContext {
    /// Create a new context with empty result map.
    pub fn new(config: HashMap<String, String>, query: SearchQuery) -> Self {
        Self {
            config,
            query,
            result: HashMap::new(),
        }
    }
}

/// Render a Go-style template string using the given context.
///
/// Supports `{{ .Config.x }}`, `{{ .Query.X }}`, `{{ .Result.x }}`,
/// `{{ .Today.X }}`, `{{ .True }}`, `{{ .False }}`, `if/else/end` blocks,
/// boolean functions (`and`, `or`, `eq`, `ne`), and string functions
/// (`re_replace`, `join`).
pub fn render(template: &str, ctx: &TemplateContext) -> String {
    let tokens = tokenize(template);
    let mut output = String::with_capacity(template.len());
    render_tokens(&tokens, ctx, &mut output);
    output
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Token {
    Literal(String),
    Expr(String),
    If(String),
    Else,
    End,
}

/// Split template into literal segments and `{{ ... }}` blocks.
fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut rest = input;

    while let Some(start) = rest.find("{{") {
        if start > 0 {
            tokens.push(Token::Literal(rest[..start].to_owned()));
        }
        rest = &rest[start + 2..];

        let end = rest.find("}}").unwrap_or(rest.len());
        let raw = rest[..end].trim();

        if raw.starts_with("if ") || raw == "if" {
            tokens.push(Token::If(raw.strip_prefix("if").unwrap().trim().to_owned()));
        } else if raw == "else" {
            tokens.push(Token::Else);
        } else if raw == "end" {
            tokens.push(Token::End);
        } else {
            tokens.push(Token::Expr(raw.to_owned()));
        }

        rest = if end + 2 <= rest.len() {
            &rest[end + 2..]
        } else {
            ""
        };
    }

    if !rest.is_empty() {
        tokens.push(Token::Literal(rest.to_owned()));
    }

    tokens
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

fn render_tokens(tokens: &[Token], ctx: &TemplateContext, out: &mut String) {
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Literal(s) => out.push_str(s),
            Token::Expr(expr) => out.push_str(&eval_expr(expr, ctx)),
            Token::If(condition) => {
                // Collect the if body, optional else body, and find the matching end.
                let (if_body, else_body, after) = collect_if_block(tokens, i + 1);
                let truthy = eval_condition(condition, ctx);
                if truthy {
                    render_tokens(&if_body, ctx, out);
                } else {
                    render_tokens(&else_body, ctx, out);
                }
                i = after;
                continue;
            }
            // Else / End should only be encountered inside collect_if_block
            Token::Else | Token::End => {}
        }
        i += 1;
    }
}

/// Collect tokens for the if-true body and optional else body.
/// Returns `(if_body, else_body, index_after_end)`.
fn collect_if_block(tokens: &[Token], start: usize) -> (Vec<Token>, Vec<Token>, usize) {
    let mut depth = 1u32;
    let mut i = start;
    let mut else_idx: Option<usize> = None;

    while i < tokens.len() {
        match &tokens[i] {
            Token::If(_) => depth += 1,
            Token::End => {
                depth -= 1;
                if depth == 0 {
                    let if_end = else_idx.unwrap_or(i);
                    let if_body = tokens[start..if_end].to_vec();
                    let else_body = if let Some(ei) = else_idx {
                        tokens[ei + 1..i].to_vec()
                    } else {
                        Vec::new()
                    };
                    return (if_body, else_body, i + 1);
                }
            }
            Token::Else if depth == 1 => {
                else_idx = Some(i);
            }
            _ => {}
        }
        i += 1;
    }

    // Unterminated if — treat rest as body
    let if_end = else_idx.unwrap_or(tokens.len());
    let if_body = tokens[start..if_end].to_vec();
    let else_body = if let Some(ei) = else_idx {
        tokens[ei + 1..].to_vec()
    } else {
        Vec::new()
    };
    (if_body, else_body, tokens.len())
}

// ---------------------------------------------------------------------------
// Expression evaluation
// ---------------------------------------------------------------------------

/// Evaluate a template expression (everything inside `{{ ... }}` that is not if/else/end).
fn eval_expr(expr: &str, ctx: &TemplateContext) -> String {
    let expr = expr.trim();

    // Function calls: re_replace, join
    if let Some(rest) = expr.strip_prefix("re_replace ") {
        return eval_re_replace(rest, ctx);
    }
    if let Some(rest) = expr.strip_prefix("join ") {
        return eval_join(rest, ctx);
    }

    // Variable path
    resolve_value(expr, ctx)
}

/// Evaluate a condition expression. Returns true/false.
fn eval_condition(expr: &str, ctx: &TemplateContext) -> bool {
    let expr = expr.trim();

    // and / or with possible parenthesized arguments
    if let Some(rest) = strip_func_prefix(expr, "and") {
        let args = split_condition_args(rest);
        return args.iter().all(|a| eval_condition(a, ctx));
    }
    if let Some(rest) = strip_func_prefix(expr, "or") {
        let args = split_condition_args(rest);
        return args.iter().any(|a| eval_condition(a, ctx));
    }

    // eq / ne
    if let Some(rest) = strip_func_prefix(expr, "eq") {
        let args = split_condition_args(rest);
        if args.len() >= 2 {
            let a = eval_condition_value(&args[0], ctx);
            let b = eval_condition_value(&args[1], ctx);
            return a == b;
        }
        return false;
    }
    if let Some(rest) = strip_func_prefix(expr, "ne") {
        let args = split_condition_args(rest);
        if args.len() >= 2 {
            let a = eval_condition_value(&args[0], ctx);
            let b = eval_condition_value(&args[1], ctx);
            return a != b;
        }
        return false;
    }

    // Parenthesized expression: (.Keywords)
    if let Some(inner) = strip_parens(expr) {
        return eval_condition(inner, ctx);
    }

    // Plain value — truthy if non-empty
    is_truthy(&resolve_value(expr, ctx))
}

/// Resolve the string value of a condition argument (might be a sub-expression,
/// a quoted string, or a variable path).
fn eval_condition_value(expr: &str, ctx: &TemplateContext) -> String {
    let expr = expr.trim();

    // Parenthesized expression
    if let Some(inner) = strip_parens(expr) {
        return eval_condition_value(inner, ctx);
    }

    // Quoted string
    if let Some(s) = strip_quotes(expr) {
        return s.to_owned();
    }

    // eq / ne inside condition value — return the boolean as string
    if strip_func_prefix(expr, "eq").is_some() || strip_func_prefix(expr, "ne").is_some() {
        return if eval_condition(expr, ctx) {
            "true".to_owned()
        } else {
            String::new()
        };
    }

    resolve_value(expr, ctx)
}

/// Resolve a dotted variable path to its string value.
fn resolve_value(path: &str, ctx: &TemplateContext) -> String {
    let path = path.trim().trim_start_matches('.');

    // Boolean literals
    if path == "True" {
        return "true".to_owned();
    }
    if path == "False" {
        return String::new();
    }

    // .Config.<key>
    if let Some(key) = path.strip_prefix("Config.") {
        return ctx.config.get(key).cloned().unwrap_or_default();
    }

    // .Query.<field>
    if let Some(field) = path.strip_prefix("Query.") {
        return resolve_query_field(&ctx.query, field);
    }

    // .Result.<field>
    if let Some(field) = path.strip_prefix("Result.") {
        return ctx.result.get(field).cloned().unwrap_or_default();
    }

    // .Today.<component>
    if let Some(component) = path.strip_prefix("Today.") {
        return resolve_today(component);
    }

    // Bare name — try query fields (for shorthand like .Keywords)
    let val = resolve_query_field(&ctx.query, path);
    if !val.is_empty() {
        return val;
    }

    // Unknown → empty
    String::new()
}

fn resolve_query_field(q: &SearchQuery, field: &str) -> String {
    match field {
        "Q" => q.q.clone(),
        "Keywords" => q.keywords.clone(),
        "Season" => q.season.clone().unwrap_or_default(),
        "Ep" => q.ep.clone().unwrap_or_default(),
        "IMDBID" => q.imdbid.clone().unwrap_or_default(),
        "TMDBID" => q.tmdbid.clone().unwrap_or_default(),
        "TVDBID" => q.tvdbid.clone().unwrap_or_default(),
        "Year" => q.year.clone().unwrap_or_default(),
        "Categories" => q.categories.join(","),
        _ => String::new(),
    }
}

fn resolve_today(component: &str) -> String {
    let now = chrono::Local::now();
    match component {
        "Year" => now.year().to_string(),
        "Month" => format!("{:02}", now.month()),
        "Day" => format!("{:02}", now.day()),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Built-in functions
// ---------------------------------------------------------------------------

fn eval_re_replace(args_str: &str, ctx: &TemplateContext) -> String {
    // re_replace <value_expr> "pattern" "replacement"
    let parts = split_func_args(args_str);
    if parts.len() < 3 {
        return String::new();
    }

    let input = resolve_value(&parts[0], ctx);
    let pattern = unquote(&parts[1]);
    let replacement = unquote(&parts[2]);

    match regex::Regex::new(&pattern) {
        Ok(re) => re.replace_all(&input, replacement.as_str()).into_owned(),
        Err(_) => input,
    }
}

fn eval_join(args_str: &str, ctx: &TemplateContext) -> String {
    // join <value_expr> "separator"
    let parts = split_func_args(args_str);
    if parts.len() < 2 {
        return String::new();
    }

    let value = resolve_value(&parts[0], ctx);
    let sep = unquote(&parts[1]);

    // If value is already comma-separated (from Categories), re-join with new separator
    if sep == "," {
        return value;
    }

    value
        .split(',')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(&sep)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_truthy(s: &str) -> bool {
    !s.is_empty() && s != "0" && s != "false"
}

/// Strip a function prefix like "and " returning the rest, or None.
fn strip_func_prefix<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let s = s.trim();
    s.strip_prefix(name).and_then(|rest| {
        (rest.starts_with(' ') || rest.starts_with('(')).then(|| rest.trim_start())
    })
}

/// Strip surrounding parentheses if present: `(expr)` -> `expr`.
fn strip_parens(s: &str) -> Option<&str> {
    let s = s.trim();
    (s.starts_with('(') && s.ends_with(')')).then(|| &s[1..s.len() - 1])
}

/// Strip surrounding double quotes: `"foo"` -> `foo`.
fn strip_quotes(s: &str) -> Option<&str> {
    let s = s.trim();
    (s.len() >= 2 && s.starts_with('"') && s.ends_with('"')).then(|| &s[1..s.len() - 1])
}

/// Remove surrounding quotes if present.
fn unquote(s: &str) -> String {
    strip_quotes(s).unwrap_or(s).to_owned()
}

/// Split condition arguments, respecting parentheses and quotes.
/// E.g. `.Query.Season .Query.Ep` → [`.Query.Season`, `.Query.Ep`]
/// E.g. `(.Keywords) (eq .Config.disablesort .False)` → [`(.Keywords)`, `(eq .Config.disablesort .False)`]
fn split_condition_args(s: &str) -> Vec<String> {
    let s = s.trim();
    let mut args = Vec::new();
    let mut current = String::new();
    let mut paren_depth: u32 = 0;
    let mut in_quote = false;

    for ch in s.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            '(' if !in_quote => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' if !in_quote => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            ' ' | '\t' if !in_quote && paren_depth == 0 => {
                let trimmed = current.trim().to_owned();
                if !trimmed.is_empty() {
                    args.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim().to_owned();
    if !trimmed.is_empty() {
        args.push(trimmed);
    }

    args
}

/// Split function arguments for `re_replace` / `join`, respecting quotes.
fn split_func_args(s: &str) -> Vec<String> {
    let s = s.trim();
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;

    for ch in s.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ' ' | '\t' if !in_quote => {
                let trimmed = current.trim().to_owned();
                if !trimmed.is_empty() {
                    args.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim().to_owned();
    if !trimmed.is_empty() {
        args.push(trimmed);
    }

    args
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> TemplateContext {
        let mut config = HashMap::new();
        config.insert("sitelink".to_owned(), "https://example.com".to_owned());
        config.insert("sort".to_owned(), "seeders".to_owned());
        config.insert("disablesort".to_owned(), String::new());

        let query = SearchQuery {
            q: "test movie".to_owned(),
            keywords: "test+movie".to_owned(),
            season: Some("3".to_owned()),
            ep: Some("5".to_owned()),
            imdbid: Some("tt1234567".to_owned()),
            tmdbid: Some("12345".to_owned()),
            tvdbid: None,
            year: Some("2024".to_owned()),
            categories: vec!["2000".to_owned(), "5000".to_owned()],
        };

        let mut result = HashMap::new();
        result.insert("title".to_owned(), "Test.Movie.2024".to_owned());
        result.insert("cat".to_owned(), "1".to_owned());
        result.insert("field1".to_owned(), "value1".to_owned());
        result.insert("field2".to_owned(), String::new());

        TemplateContext {
            config,
            query,
            result,
        }
    }

    #[test]
    fn config_variable() {
        let ctx = test_ctx();
        assert_eq!(
            render("{{ .Config.sitelink }}/search", &ctx),
            "https://example.com/search"
        );
    }

    #[test]
    fn query_variables() {
        let ctx = test_ctx();
        assert_eq!(render("q={{ .Query.Q }}", &ctx), "q=test movie");
        assert_eq!(render("{{ .Query.Season }}", &ctx), "3");
        assert_eq!(render("{{ .Query.IMDBID }}", &ctx), "tt1234567");
    }

    #[test]
    fn result_variable() {
        let ctx = test_ctx();
        assert_eq!(render("{{ .Result.title }}", &ctx), "Test.Movie.2024");
    }

    #[test]
    fn today_variables() {
        let ctx = test_ctx();
        let rendered = render("{{ .Today.Year }}", &ctx);
        assert!(!rendered.is_empty());
        assert!(rendered.len() == 4); // e.g. "2026"
    }

    #[test]
    fn boolean_literals() {
        let ctx = test_ctx();
        assert_eq!(render("{{ .True }}", &ctx), "true");
        assert_eq!(render("{{ .False }}", &ctx), "");
    }

    #[test]
    fn unknown_variable_empty() {
        let ctx = test_ctx();
        assert_eq!(render("{{ .Config.missing }}", &ctx), "");
        assert_eq!(render("{{ .Query.Nonexistent }}", &ctx), "");
    }

    #[test]
    fn if_true() {
        let ctx = test_ctx();
        let tpl = "{{ if .Query.Season }}S{{ .Query.Season }}{{ end }}";
        assert_eq!(render(tpl, &ctx), "S3");
    }

    #[test]
    fn if_false() {
        let mut ctx = test_ctx();
        ctx.query.season = None;
        let tpl = "{{ if .Query.Season }}S{{ .Query.Season }}{{ end }}";
        assert_eq!(render(tpl, &ctx), "");
    }

    #[test]
    fn if_else() {
        let ctx = test_ctx();
        let tpl = "{{ if .Query.TVDBID }}tvdb={{ .Query.TVDBID }}{{ else }}no-tvdb{{ end }}";
        assert_eq!(render(tpl, &ctx), "no-tvdb");
    }

    #[test]
    fn if_and() {
        let ctx = test_ctx();
        let tpl =
            "{{ if and .Query.Season .Query.Ep }}S{{ .Query.Season }}E{{ .Query.Ep }}{{ end }}";
        assert_eq!(render(tpl, &ctx), "S3E5");
    }

    #[test]
    fn if_and_one_false() {
        let mut ctx = test_ctx();
        ctx.query.ep = None;
        let tpl = "{{ if and .Query.Season .Query.Ep }}found{{ else }}missing{{ end }}";
        assert_eq!(render(tpl, &ctx), "missing");
    }

    #[test]
    fn if_or() {
        let ctx = test_ctx();
        let tpl = "{{ if or .Result.field1 .Result.field2 }}has-field{{ end }}";
        assert_eq!(render(tpl, &ctx), "has-field");
    }

    #[test]
    fn if_eq() {
        let ctx = test_ctx();
        let tpl = r#"{{ if eq .Config.sort "seeders" }}sort=seeders{{ end }}"#;
        assert_eq!(render(tpl, &ctx), "sort=seeders");
    }

    #[test]
    fn if_ne() {
        let ctx = test_ctx();
        let tpl = r#"{{ if ne .Result.cat "0" }}cat={{ .Result.cat }}{{ end }}"#;
        assert_eq!(render(tpl, &ctx), "cat=1");
    }

    #[test]
    fn nested_if() {
        let ctx = test_ctx();
        let tpl = "{{ if .Query.Season }}S{{ if .Query.Ep }}E{{ .Query.Ep }}{{ end }}{{ end }}";
        assert_eq!(render(tpl, &ctx), "SE5");
    }

    #[test]
    fn re_replace_function() {
        let ctx = test_ctx();
        let tpl = r#"{{ re_replace .Query.Q "[^a-zA-Z0-9]+" "%" }}"#;
        assert_eq!(render(tpl, &ctx), "test%movie");
    }

    #[test]
    fn join_function() {
        let ctx = test_ctx();
        let tpl = r#"{{ join .Query.Categories "," }}"#;
        assert_eq!(render(tpl, &ctx), "2000,5000");
    }

    #[test]
    fn parenthesized_condition() {
        let ctx = test_ctx();
        let tpl = r"{{ if and (.Keywords) (eq .Config.disablesort .False) }}keywords={{ .Query.Keywords }}{{ end }}";
        // .Keywords = "test+movie" (truthy)
        // .Config.disablesort = "" and .False = "" => eq is true
        assert_eq!(render(tpl, &ctx), "keywords=test+movie");
    }

    #[test]
    fn categories_variable() {
        let ctx = test_ctx();
        assert_eq!(render("{{ .Query.Categories }}", &ctx), "2000,5000");
    }

    #[test]
    fn multiple_expressions() {
        let ctx = test_ctx();
        let tpl = "{{ .Config.sitelink }}/search?q={{ .Query.Q }}&cat={{ .Query.Categories }}";
        assert_eq!(
            render(tpl, &ctx),
            "https://example.com/search?q=test movie&cat=2000,5000"
        );
    }
}
