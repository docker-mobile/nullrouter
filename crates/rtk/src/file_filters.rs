//! File listing filters: find, tree, ls, read-numbered, search-list, build-output.

/// Group find output by parent directory, show basenames, cap 10/dir.
pub(crate) fn compact_find(input: &str) -> String {
    use std::collections::BTreeMap;
    let lines: Vec<&str> = input.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return input.to_owned();
    }
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in &lines {
        let last_sep = path
            .rfind('/')
            .unwrap_or_else(|| path.rfind('\\').unwrap_or(0));
        let (dir, base) = if last_sep == 0 && !path.contains('/') {
            (".", *path)
        } else {
            (&path[..last_sep.max(1)], &path[last_sep + 1..])
        };
        by_dir.entry(dir).or_default().push(base);
    }
    let mut out = format!("{} files in {} dirs:\n\n", lines.len(), by_dir.len());
    for (dir, files) in &by_dir {
        out.push_str(dir);
        out.push_str(":\n");
        for f in files.iter().take(10) {
            out.push_str("  ");
            out.push_str(f);
            out.push('\n');
        }
        if files.len() > 10 {
            out.push_str(&format!("  ... +{} more\n", files.len() - 10));
        }
    }
    out
}

/// Strip tree summary line and trailing blanks.
pub(crate) fn compact_tree(input: &str) -> String {
    let mut filtered: Vec<&str> = Vec::new();
    for line in input.lines() {
        if line.contains("director") && line.contains("file") {
            continue;
        }
        if line.trim().is_empty() && filtered.is_empty() {
            continue;
        }
        filtered.push(line);
    }
    while filtered.last().is_some_and(|l| l.trim().is_empty()) {
        filtered.pop();
    }
    filtered.join("\n")
}

/// Truncate numbered file content (N|content) to head+tail.
pub(crate) fn compact_read_numbered(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    if lines.len() < 100 {
        return input.to_owned();
    }
    let head = 20;
    let tail = 20;
    let cut = lines.len() - head - tail;
    let mut out = lines.get(..head).unwrap_or(&lines).join("\n");
    out.push_str(&format!("\n... +{cut} lines truncated (file continues)\n"));
    let tail_start = lines.len().saturating_sub(tail);
    out.push_str(&lines.get(tail_start..).unwrap_or(&lines).join("\n"));
    out
}

/// Compact search result list, group by directory.
pub(crate) fn compact_search_list(input: &str) -> String {
    use std::collections::BTreeMap;
    let lines: Vec<&str> = input.lines().collect();
    if lines.is_empty() {
        return input.to_owned();
    }
    let mut paths: Vec<&str> = Vec::new();
    for line in lines.get(1..).unwrap_or(&[]) {
        let t = line.trim();
        if let Some(p) = t.strip_prefix("- ") {
            paths.push(p);
        }
    }
    if paths.is_empty() {
        return input.to_owned();
    }
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in &paths {
        let sep = p.rfind('/').unwrap_or(0);
        let (dir, base) = if sep == 0 && !p.contains('/') {
            (".", *p)
        } else {
            (&p[..sep.max(1)], &p[sep + 1..])
        };
        by_dir.entry(dir).or_default().push(base);
    }
    let mut out = format!("{} files in {} dirs:\n\n", paths.len(), by_dir.len());
    for (dir, files) in &by_dir {
        out.push_str(dir);
        out.push_str(":\n");
        for f in files.iter().take(10) {
            out.push_str("  ");
            out.push_str(f);
            out.push('\n');
        }
        if files.len() > 10 {
            out.push_str(&format!("  ... +{} more\n", files.len() - 10));
        }
    }
    out
}

/// Compact `ls -la` output: parse lines, group by type, show names + sizes.
pub(crate) fn compact_ls(input: &str) -> String {
    let mut dirs = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for line in input.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }
        // Parse: perms ... size month day time name
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let perms = parts[0];
        let is_dir = perms.starts_with('d');
        // Size is typically at index 4
        let size: u64 = parts
            .iter()
            .rev()
            .skip(1)
            .find_map(|p| p.parse().ok())
            .unwrap_or(0);
        let name = parts[8..].join(" ");
        if is_dir {
            dirs.push(name);
        } else {
            let size_str = if size >= 1_048_576 {
                format!("{:.1}M", size as f64 / 1_048_576.0)
            } else if size >= 1024 {
                format!("{:.1}K", size as f64 / 1024.0)
            } else {
                format!("{size}B")
            };
            files.push((name, size_str));
        }
    }
    let mut out = String::new();
    if !dirs.is_empty() {
        out.push_str(&format!("Dirs ({}):\n", dirs.len()));
        for d in dirs.iter().take(10) {
            out.push_str("  ");
            out.push_str(d);
            out.push('\n');
        }
    }
    if !files.is_empty() {
        out.push_str(&format!("Files ({}):\n", files.len()));
        for (name, size) in files.iter().take(20) {
            out.push_str(&format!("  {name} ({size})\n"));
        }
        if files.len() > 20 {
            out.push_str(&format!("  ... +{} more\n", files.len() - 20));
        }
    }
    out
}

/// Compact build tool output (npm, cargo, pip, etc.).
/// Keeps errors, warnings, final summary. Strips progress logs.
pub(crate) fn compact_build_output(input: &str) -> String {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut summary: Option<String> = None;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("npm err")
            || lower.starts_with("yarn error")
            || lower.starts_with("error:")
            || lower.starts_with("[error]")
        {
            errors.push(line.to_owned());
        } else if lower.starts_with("npm warn")
            || lower.starts_with("yarn warn")
            || lower.starts_with("warning:")
        {
            if warnings.len() < 5 {
                warnings.push(line.to_owned());
            }
        } else if lower.starts_with("finished")
            || lower.starts_with("successfully")
            || lower.starts_with("build ")
            || lower.contains("compilation finished")
        {
            summary = Some(line.to_owned());
        }
    }
    let mut out = String::new();
    if !errors.is_empty() {
        out.push_str(&format!("Errors ({}):\n", errors.len()));
        for e in &errors {
            out.push_str(e);
            out.push('\n');
        }
    }
    if !warnings.is_empty() {
        out.push_str(&format!("Warnings ({}):\n", warnings.len()));
        for w in &warnings {
            out.push_str(w);
            out.push('\n');
        }
    }
    if let Some(s) = summary {
        out.push_str(&s);
        out.push('\n');
    }
    if out.is_empty() {
        input.to_owned()
    } else {
        out
    }
}
