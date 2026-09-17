//! Git output filters: diff, status, log.

/// Compact unified git diff: keep file headers, cap hunks, count +/-/context.
pub(crate) fn compact_git_diff(diff: &str) -> String {
    let max_lines = 500;
    let max_hunk = 100;
    let mut result = String::with_capacity(diff.len() / 2);
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut hunk_shown = 0u32;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            if added > 0 || removed > 0 {
                result.push_str(&format!("  +{added} -{removed}\n"));
                added = 0;
                removed = 0;
            }
            result.push_str(line);
            result.push('\n');
            hunk_shown = 0;
        } else if line.starts_with("@@") {
            hunk_shown = 0;
            result.push_str(line);
            result.push('\n');
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
            if hunk_shown < max_hunk {
                result.push_str(line);
                result.push('\n');
                hunk_shown += 1;
            }
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
            if hunk_shown < max_hunk {
                result.push_str(line);
                result.push('\n');
                hunk_shown += 1;
            }
        } else if hunk_shown < max_hunk {
            result.push_str(line);
            result.push('\n');
            hunk_shown += 1;
        }
        if result.len() > max_lines * 80 {
            break;
        }
    }
    if added > 0 || removed > 0 {
        result.push_str(&format!("  +{added} -{removed}"));
    }
    result
}

/// Compact git status output.
pub(crate) fn compact_git_status(input: &str) -> String {
    let max_files = 10;
    let lines: Vec<&str> = input.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "Clean working tree".to_owned();
    }
    let mut result = String::new();
    let mut staged = 0u32;
    let mut modified = 0u32;
    let mut untracked = 0u32;
    let mut shown = 0u32;
    for line in &lines {
        if line.starts_with("On branch") || line.starts_with("##") {
            // Keep branch line
            result.push_str(line);
            result.push('\n');
        } else if line.starts_with("Changes to be committed") {
            result.push_str(line);
            result.push('\n');
        } else if line.starts_with('M')
            || line.starts_with('A')
            || line.starts_with('D')
            || line.starts_with('R')
        {
            staged += 1;
            if shown < max_files {
                result.push_str(line);
                result.push('\n');
                shown += 1;
            }
        } else if line.starts_with(" M") || line.starts_with("??") {
            if line.starts_with("??") {
                untracked += 1;
            } else {
                modified += 1;
            }
            if shown < max_files {
                result.push_str(line);
                result.push('\n');
                shown += 1;
            }
        }
    }
    if staged + modified + untracked == 0 && result.trim().is_empty() {
        return "Clean working tree".to_owned();
    }
    result
}

/// Compact git log: keep headers and subjects, cap total lines.
pub(crate) fn compact_git_log(text: &str, max_lines: usize) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut count = 0;
    for line in text.lines() {
        if count >= max_lines {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() && out.is_empty() {
            continue;
        }
        if trimmed.starts_with("commit ")
            || trimmed.starts_with("Author:")
            || trimmed.starts_with("Date:")
            || (!trimmed.is_empty() && !trimmed.starts_with(' '))
        {
            out.push_str(trimmed);
            out.push('\n');
            count += 1;
        }
    }
    out
}
