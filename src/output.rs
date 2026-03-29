use crate::cli::ColorWhen;
use is_terminal::IsTerminal;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultKind {
    Hack,
    Regular,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckMethod {
    Cache,
    Dns,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DomainResult {
    pub domain: String,
    pub available: bool,
    pub kind: ResultKind,
    pub method: CheckMethod,
}

#[derive(Debug, Clone, Copy)]
pub struct OutputOptions {
    pub json: bool,
    pub show_all: bool,
    pub color: ColorWhen,
}

pub fn visible_results(
    results: &[DomainResult],
    show_all: bool,
    limit: Option<usize>,
) -> Vec<DomainResult> {
    let mut visible = results
        .iter()
        .filter(|result| show_all || result.available)
        .cloned()
        .collect::<Vec<_>>();

    if let Some(limit) = limit {
        visible.truncate(limit);
    }

    visible
}

pub fn render(results: &[DomainResult], options: OutputOptions) -> String {
    if options.json {
        return render_json(results, options.show_all);
    }

    render_plain(results, options)
}

fn render_json(results: &[DomainResult], show_all: bool) -> String {
    let mut lines = Vec::new();
    for result in results.iter().filter(|result| show_all || result.available) {
        let line = serde_json::to_string(result).expect("serializing result should never fail");
        lines.push(line);
    }
    join_lines(lines)
}

fn render_plain(results: &[DomainResult], options: OutputOptions) -> String {
    let use_color = should_use_color(options.color);
    let mut lines = Vec::new();

    for result in results {
        if !options.show_all && !result.available {
            continue;
        }

        let line = if options.show_all {
            let prefix = if result.available { "  " } else { "x " };
            format!("{prefix}{}", result.domain)
        } else {
            result.domain.clone()
        };

        lines.push(colorize(&line, result.available, use_color));
    }

    join_lines(lines)
}

fn should_use_color(color: ColorWhen) -> bool {
    match color {
        ColorWhen::Always => true,
        ColorWhen::Never => false,
        ColorWhen::Auto => std::io::stdout().is_terminal(),
    }
}

fn colorize(line: &str, available: bool, use_color: bool) -> String {
    if !use_color {
        return line.to_string();
    }

    if available {
        format!("\u{1b}[32m{line}\u{1b}[0m")
    } else {
        format!("\u{1b}[2m{line}\u{1b}[0m")
    }
}

fn join_lines(lines: Vec<String>) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut output = lines.join("\n");
        output.push('\n');
        output
    }
}
