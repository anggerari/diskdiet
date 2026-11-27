//! Numbered toggle picker:
//!
//!   #   sel  type        age    size     path
//!   1   [x]  Node        45d    1.2 GB   ~/projects/foo/node_modules
//!
//!   Toggle by number (e.g. "1 3 8" or "2-6"), a = all, n = none, Enter = confirm, q = quit:

use std::collections::BTreeSet;
use std::io::{self, Write};

use console::{style, Term};

use diskdiet::util::human_bytes;

pub struct Row {
    pub kind: String,
    pub age: String,
    pub size: u64,
    pub size_label: String,
    pub label: String,
    pub note: Option<String>,
    pub selected: bool,
}

/// Parse a toggle line such as `1 3 8`, `2-6`, `1,4`. Returns 0-based indices.
pub fn parse_toggles(input: &str, len: usize) -> Result<BTreeSet<usize>, String> {
    let mut out = BTreeSet::new();
    for tok in input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
    {
        let (a, b) = match tok.split_once('-') {
            Some((a, b)) => (a, b),
            None => (tok, tok),
        };
        let a: usize = a
            .trim()
            .parse()
            .map_err(|_| format!("'{tok}' is not a number"))?;
        let b: usize = b
            .trim()
            .parse()
            .map_err(|_| format!("'{tok}' is not a number"))?;
        let (lo, hi) = (a.min(b), a.max(b));
        if lo == 0 || hi > len {
            return Err(format!("'{tok}' is out of range (1-{len})"));
        }
        out.extend(lo - 1..hi);
    }
    Ok(out)
}

fn render(rows: &[Row], error: Option<&str>) -> Vec<String> {
    let kind_w = rows.iter().map(|r| r.kind.len()).max().unwrap_or(4).max(4);
    let num_w = rows.len().to_string().len().max(1);
    let mut lines = Vec::new();
    lines.push(
        style(format!(
            "  {:>num_w$}  sel  {:<kind_w$}  {:>5}  {:>9}  path",
            "#", "type", "age", "size"
        ))
        .dim()
        .to_string(),
    );
    for (i, r) in rows.iter().enumerate() {
        let mark = if r.selected {
            style("[x]").green().bold().to_string()
        } else {
            "[ ]".to_string()
        };
        let size = format!("{:>9}", r.size_label);
        let size = if r.size >= 1 << 30 {
            style(size).red().bold().to_string()
        } else if r.size >= 100 << 20 {
            style(size).yellow().to_string()
        } else {
            size
        };
        lines.push(format!(
            "  {:>num_w$}  {mark}  {:<kind_w$}  {:>5}  {size}  {}{}",
            i + 1,
            r.kind,
            style(&r.age).dim(),
            r.label,
            note(r)
        ));
    }
    let (n, bytes) = rows
        .iter()
        .filter(|r| r.selected)
        .fold((0, 0u64), |(n, b), r| (n + 1, b + r.size));
    lines.push(String::new());
    lines.push(format!(
        "  {} {n} item(s), ~{}",
        style("Selected:").bold(),
        style(human_bytes(bytes)).bold()
    ));
    lines.push(String::new());
    if let Some(e) = error {
        lines.push(format!("  {}", style(e).red()));
    }
    lines
}

/// Render a row's note: warnings in yellow, hints dimmed.
pub fn note(r: &Row) -> String {
    match &r.note {
        Some(n) if n.contains('⚠') => format!("  {}", style(n).yellow()),
        Some(n) => format!("  {}", style(n).dim()),
        None => String::new(),
    }
}

/// Run the picker. Returns `None` if the user quit.
pub fn pick(rows: &mut [Row]) -> io::Result<Option<()>> {
    let term = Term::stdout();
    let mut error: Option<String> = None;
    let mut drawn = 0;
    loop {
        if drawn > 0 {
            term.clear_last_lines(drawn)?;
        }
        let lines = render(rows, error.as_deref());
        for l in &lines {
            term.write_line(l)?;
        }
        write!(
            &term,
            "  Toggle by number (e.g. \"1 3 8\" or \"2-6\"), a = all, n = none, Enter = confirm, q = quit: "
        )?;
        term.flush()?;
        let input = term.read_line()?;
        drawn = lines.len() + 1;
        error = None;
        match input.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(Some(())),
            "q" | "quit" => return Ok(None),
            "a" | "all" => rows.iter_mut().for_each(|r| r.selected = true),
            "n" | "none" => rows.iter_mut().for_each(|r| r.selected = false),
            other => match parse_toggles(other, rows.len()) {
                Ok(idx) => idx
                    .into_iter()
                    .for_each(|i| rows[i].selected = !rows[i].selected),
                Err(e) => error = Some(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles() {
        let v: Vec<_> = parse_toggles("1 3, 5-6", 10).unwrap().into_iter().collect();
        assert_eq!(v, vec![0, 2, 4, 5]);
        assert_eq!(parse_toggles("6-5", 10).unwrap().len(), 2);
        assert!(parse_toggles("0", 3).is_err());
        assert!(parse_toggles("4", 3).is_err());
        assert!(parse_toggles("x", 3).is_err());
    }
}
