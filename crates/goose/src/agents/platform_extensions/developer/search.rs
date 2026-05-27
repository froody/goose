use std::fs;
use std::path::{Path, PathBuf};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Glob patterns of files to match (e.g., ["src/**/*.rs"] or ["src/lib.rs#10-50"])
    pub file_glob_patterns: Vec<String>,
    /// Optional regex pattern to search inside matched files (like ripgrep)
    pub content_regex: Option<String>,
    /// Controls what content is returned: "paths_with_content" (default), "file_paths_with_content", or "file_paths_only"
    pub output_mode: Option<String>,
    /// Maximum number of lines to return per file
    pub lines_per_file: Option<usize>,
    /// Maximum number of files to inspect
    pub file_limit: Option<usize>,
    /// Whether the regex pattern should match across multiple lines
    #[serde(default)]
    pub multiline: bool,
    /// Case-insensitive search toggle
    #[serde(default)]
    pub case_insensitive: bool,
}

pub struct SearchTool;

struct SuffixRange {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

impl SearchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn search_with_cwd(&self, params: SearchParams, working_dir: Option<&Path>) -> CallToolResult {
        let root = working_dir
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        // 1. Parse line ranges off exact files
        let mut targets = Vec::new();
        for pat in &params.file_glob_patterns {
            targets.push(parse_suffix_range(pat));
        }

        // 2. Compile glob regexes for wildcard matching
        let mut compiled_globs = Vec::new();
        for t in &targets {
            if t.path.contains('*') || t.path.contains('?') {
                if let Ok(re) = compile_glob(&t.path) {
                    compiled_globs.push(re);
                }
            }
        }

        // 3. Find files matching the glob/exact patterns
        let mut matched_files = Vec::new();
        let file_limit = params.file_limit.unwrap_or(500);

        // First, check exact matches (to avoid walk if they are simple paths)
        for t in &targets {
            if !t.path.contains('*') && !t.path.contains('?') {
                let exact_path = root.join(&t.path);
                if exact_path.is_file() {
                    matched_files.push((exact_path, Some(t)));
                    if matched_files.len() >= file_limit {
                        break;
                    }
                }
            }
        }

        // Second, walk directory to match wildcards (if any compiled globs exist)
        if !compiled_globs.is_empty() && matched_files.len() < file_limit {
            let mut builder = WalkBuilder::new(&root);
            builder.git_ignore(true);
            builder.git_exclude(true);
            builder.git_global(true);
            builder.require_git(false);
            builder.ignore(true);
            builder.hidden(true);

            for entry in builder.build().flatten() {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    let path = entry.path();
                    let rel_path = match path.strip_prefix(&root) {
                        Ok(p) => p.to_string_lossy().to_string(),
                        Err(_) => continue,
                    };

                    // Check if this matches any glob
                    let matches_glob = compiled_globs.iter().any(|re| re.is_match(&rel_path));
                    if matches_glob {
                        // Avoid duplicates
                        if !matched_files.iter().any(|(p, _)| p == path) {
                            matched_files.push((path.to_path_buf(), None));
                            if matched_files.len() >= file_limit {
                                break;
                            }
                        }
                    }
                }
            }
        }

        if matched_files.is_empty() {
            return CallToolResult::success(vec![Content::text("No files matched the specified patterns.")]);
        }

        // 4. Content regex compilation
        let content_re = if let Some(ref r) = params.content_regex {
            let mut builder = RegexBuilder::new(r);
            builder.case_insensitive(params.case_insensitive);
            builder.multi_line(params.multiline);
            match builder.build() {
                Ok(re) => Some(re),
                Err(e) => {
                    return CallToolResult::error(vec![Content::text(format!("Invalid content_regex: {e}"))]);
                }
            }
        } else {
            None
        };

        // 5. Build Output
        let output_mode = params.output_mode.as_deref().unwrap_or("paths_with_content");
        let mut final_output = String::new();
        let lines_per_file = params.lines_per_file.unwrap_or(200);

        let mut printed_count = 0;

        for (file_path, suffix_range) in matched_files {
            let rel_path = file_path.strip_prefix(&root).unwrap_or(&file_path).to_string_lossy().to_string();
            let file_content = match fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if output_mode == "file_paths_only" {
                if let Some(ref re) = content_re {
                    if re.is_match(&file_content) {
                        final_output.push_str(&format!("{rel_path}\n"));
                        printed_count += 1;
                    }
                } else {
                    final_output.push_str(&format!("{rel_path}\n"));
                    printed_count += 1;
                }
                continue;
            }

            // Slicing lines (either from suffix ranges or lines_per_file limits)
            let start_line = suffix_range.and_then(|r| r.start_line).unwrap_or(1);
            let end_line = suffix_range.and_then(|r| r.end_line);

            let lines: Vec<&str> = file_content.lines().collect();
            let actual_end_line = end_line.unwrap_or(lines.len()).min(lines.len());
            let actual_start_line = start_line.min(actual_end_line).max(1);

            let mut matched_lines = Vec::new();

            for line_idx in (actual_start_line - 1)..actual_end_line {
                let line_text = lines[line_idx];
                let line_num = line_idx + 1;

                if let Some(ref re) = content_re {
                    if re.is_match(line_text) {
                        matched_lines.push((line_num, line_text));
                    }
                } else {
                    matched_lines.push((line_num, line_text));
                }
            }

            if matched_lines.is_empty() {
                continue;
            }

            printed_count += 1;

            if output_mode == "paths_with_content" {
                final_output.push_str(&format!("--- {rel_path} ---\n"));
                let count = matched_lines.len().min(lines_per_file);
                for (num, text) in &matched_lines[..count] {
                    final_output.push_str(&format!("{num:4}: {text}\n"));
                }
                if matched_lines.len() > count {
                    final_output.push_str(&format!("... ({} more matches)\n", matched_lines.len() - count));
                }
                final_output.push_str("\n");
            } else if output_mode == "file_paths_with_content" {
                final_output.push_str(&format!("=== File: {rel_path} ===\n"));
                let count = matched_lines.len().min(lines_per_file);
                for (num, text) in &matched_lines[..count] {
                    final_output.push_str(&format!("{num:4}: {text}\n"));
                }
                if matched_lines.len() > count {
                    final_output.push_str(&format!("... ({} lines truncated)\n", matched_lines.len() - count));
                }
                final_output.push_str("\n");
            }
        }

        if printed_count == 0 {
            CallToolResult::success(vec![Content::text("No content matches found inside the files.")])
        } else {
            CallToolResult::success(vec![Content::text(final_output)])
        }
    }
}

fn parse_suffix_range(pattern: &str) -> SuffixRange {
    if let Some(hash_pos) = pattern.find('#') {
        let (path_part, suffix) = pattern.split_at(hash_pos);
        let range_part = &suffix[1..]; // skip '#'

        let (start_line, end_line) = if let Some(dash_pos) = range_part.find('-') {
            let (start_str, end_str) = range_part.split_at(dash_pos);
            let end_str = &end_str[1..]; // skip '-'
            (start_str.parse::<usize>().ok(), end_str.parse::<usize>().ok())
        } else {
            (range_part.parse::<usize>().ok(), None)
        };

        SuffixRange {
            path: path_part.to_string(),
            start_line,
            end_line,
        }
    } else {
        SuffixRange {
            path: pattern.to_string(),
            start_line: None,
            end_line: None,
        }
    }
}

fn compile_glob(glob: &str) -> Result<regex::Regex, regex::Error> {
    let mut regex_str = String::new();
    regex_str.push('^');
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next(); // consume second '*'
                    if chars.peek() == Some(&'/') {
                        chars.next(); // consume '/'
                        regex_str.push_str("(?:.*/)?");
                    } else {
                        regex_str.push_str(".*");
                    }
                } else {
                    regex_str.push_str("[^/]*");
                }
            }
            '?' => regex_str.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '\\' | '^' | '$' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            _ => regex_str.push(c),
        }
    }
    regex_str.push('$');
    RegexBuilder::new(&regex_str).case_insensitive(true).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_suffix_range() {
        let res = parse_suffix_range("src/main.rs#10-50");
        assert_eq!(res.path, "src/main.rs");
        assert_eq!(res.start_line, Some(10));
        assert_eq!(res.end_line, Some(50));

        let res = parse_suffix_range("src/lib.rs#100");
        assert_eq!(res.path, "src/lib.rs");
        assert_eq!(res.start_line, Some(100));
        assert_eq!(res.end_line, None);

        let res = parse_suffix_range("no_suffix.rs");
        assert_eq!(res.path, "no_suffix.rs");
        assert_eq!(res.start_line, None);
        assert_eq!(res.end_line, None);
    }

    #[test]
    fn test_compile_glob() {
        let re = compile_glob("src/**/*.rs").unwrap();
        assert!(re.is_match("src/main.rs"));
        assert!(re.is_match("src/foo/bar/lib.rs"));
        assert!(!re.is_match("tests/main.rs"));

        let re = compile_glob("src/*.rs").unwrap();
        assert!(re.is_match("src/main.rs"));
        assert!(!re.is_match("src/foo/bar/lib.rs"));
    }
}
