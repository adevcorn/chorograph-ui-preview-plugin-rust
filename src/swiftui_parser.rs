/// SwiftUI component tree heuristic parser.
///
/// Strategy:
/// 1. Find structs conforming to `View` (`: View` or `: some View` in the declaration).
/// 2. For each such struct, walk the body line-by-line tracking brace depth.
/// 3. Emit component nodes for known SwiftUI types, inferring label from the first
///    string literal argument or the variable name on the left side of `=`.
/// 4. For interactive nodes (Button, Toggle, etc.) extract the action handler.
use crate::model::{UIPreviewCategory, UIPreviewNode};

// ---------------------------------------------------------------------------
// Known component classifications
// ---------------------------------------------------------------------------

struct ComponentInfo {
    category: UIPreviewCategory,
    is_container: bool,
}

fn classify(type_name: &str) -> Option<ComponentInfo> {
    let t = type_name.trim_end_matches('{').trim();
    match t {
        // Containers
        "VStack"
        | "HStack"
        | "ZStack"
        | "LazyVStack"
        | "LazyHStack"
        | "ScrollView"
        | "List"
        | "LazyVGrid"
        | "LazyHGrid"
        | "Group"
        | "GroupBox"
        | "Section"
        | "Form"
        | "NavigationStack"
        | "NavigationView"
        | "NavigationSplitView"
        | "TabView"
        | "GeometryReader"
        | "ViewThatFits"
        | "Grid"
        | "GridRow"
        | "ForEach" => Some(ComponentInfo {
            category: UIPreviewCategory::Container,
            is_container: true,
        }),
        // Interactive
        "Button" | "TextField" | "SecureField" | "Toggle" | "Slider" | "Stepper" | "Picker"
        | "DatePicker" | "ColorPicker" | "TextEditor" | "Link" | "Menu" | "ControlGroup"
        | "ShareLink" | "PasteButton" | "EditButton" | "RenameButton" => Some(ComponentInfo {
            category: UIPreviewCategory::Interactive,
            is_container: false,
        }),
        // Display
        "Text" | "Label" | "Image" | "Spacer" | "Divider" | "Color" | "ProgressView"
        | "ProgressIndicator" | "EmptyView" | "AsyncImage" | "Canvas" | "TimelineView"
        | "Chart" | "Map" | "VideoPlayer" | "Icon" | "Badge" | "Tag" => Some(ComponentInfo {
            category: UIPreviewCategory::Display,
            is_container: false,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse `source` (contents of a `.swift` file) and return the root
/// `UIPreviewNode` for the first `View`-conforming struct found, or `None`
/// if no view struct is detected.
pub fn parse_swiftui(source: &str) -> Option<UIPreviewNode> {
    let lines: Vec<&str> = source.lines().collect();

    let (struct_name, body_start) = find_view_struct(&lines)?;
    let components = extract_components(&lines, body_start);
    let root_children = build_tree(&components, 0).0;

    Some(UIPreviewNode {
        r#type: struct_name,
        category: UIPreviewCategory::Container,
        label: None,
        children: root_children,
        source_line: Some((body_start + 1) as u32),
        action_handler: None,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Returns (struct_name, line_index_of_opening_brace).
fn find_view_struct(lines: &[&str]) -> Option<(String, usize)> {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if (trimmed.starts_with("struct ") || trimmed.starts_with("class "))
            && (trimmed.contains(": View")
                || trimmed.contains(": some View")
                || trimmed.contains(", View"))
        {
            let after_keyword = trimmed
                .trim_start_matches("struct ")
                .trim_start_matches("class ");
            let name = after_keyword
                .split(|c: char| c == ':' || c == '<' || c.is_whitespace())
                .next()
                .unwrap_or("View")
                .to_string();

            let brace_line = if trimmed.contains('{') {
                i
            } else {
                lines[i..]
                    .iter()
                    .position(|l| l.contains('{'))
                    .map(|off| i + off)
                    .unwrap_or(i)
            };
            return Some((name, brace_line));
        }
    }
    None
}

/// Flat entry produced during the line walk.
struct FlatEntry {
    depth: usize,
    r#type: String,
    label: Option<String>,
    category: UIPreviewCategory,
    /// 1-based line number in the source file.
    source_line: u32,
    /// Extracted action handler for interactive nodes.
    action_handler: Option<String>,
}

fn extract_components(lines: &[&str], start: usize) -> Vec<FlatEntry> {
    let mut entries = Vec::new();
    let mut depth: i32 = 0;
    let mut inside = false;

    for (offset, line) in lines[start..].iter().enumerate() {
        let abs_line = start + offset; // 0-based absolute line index
        let trimmed = line.trim();

        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    if !inside {
                        inside = true;
                    } else {
                        depth += 1;
                    }
                }
                '}' => {
                    if depth == 0 {
                        return entries;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }

        if !inside {
            continue;
        }

        let token = trimmed
            .split(|c: char| c == '(' || c == '{' || c == ' ')
            .next()
            .unwrap_or("")
            .trim();

        if let Some(info) = classify(token) {
            let label = extract_label(trimmed);
            let action_handler = if info.category == UIPreviewCategory::Interactive {
                extract_swiftui_action(trimmed, lines, abs_line)
            } else {
                None
            };
            entries.push(FlatEntry {
                depth: depth as usize,
                r#type: token.to_string(),
                label,
                category: info.category,
                source_line: (abs_line + 1) as u32, // 1-based
                action_handler,
            });
        }
    }
    entries
}

/// Extract a display label from a component invocation line.
fn extract_label(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let rest = &line[start + 1..];
    let end = rest.find('"')?;
    let s = &rest[..end];
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Extract an action handler expression from a SwiftUI interactive component.
///
/// Handles:
///   Button("Save") { viewModel.save() }         → "viewModel.save()"
///   Button("OK", action: self.submit)            → "self.submit"
///   Button(action: { handleTap() }) { ... }      → "handleTap()"
///   Toggle(isOn: $flag) { ... }                  → no handler (binding, not action)
fn extract_swiftui_action(line: &str, lines: &[&str], line_idx: usize) -> Option<String> {
    // Pattern 1: named `action:` parameter  →  Button("x", action: self.foo)
    if let Some(pos) = line.find("action:") {
        let after = line[pos + 7..].trim();
        // Could be a closure `{ ... }` or a reference `self.foo`
        if after.starts_with('{') {
            // inline closure — extract first expression inside braces
            let inner = after.trim_start_matches('{').trim();
            let expr: String = inner
                .chars()
                .take_while(|&c| c != '}' && c != '\n')
                .collect();
            let expr = expr.trim().to_string();
            if !expr.is_empty() {
                return Some(expr);
            }
        } else {
            // reference: `self.foo` or `viewModel.bar`
            let expr: String = after
                .chars()
                .take_while(|&c| c != ')' && c != ',' && c != '\n' && c != ' ')
                .collect();
            if !expr.is_empty() {
                return Some(expr);
            }
        }
    }

    // Pattern 2: trailing closure on the same line  →  Button("Save") { viewModel.save() }
    if let Some(open) = line.rfind('{') {
        let after = &line[open + 1..];
        if let Some(close) = after.find('}') {
            let expr = after[..close].trim().to_string();
            if !expr.is_empty() && !expr.starts_with("Text") && !expr.starts_with("Label") {
                return Some(expr);
            }
        }
    }

    // Pattern 3: trailing closure on the *next* non-empty line
    //   Button("Save") {
    //       viewModel.save()     ← peek here
    //   }
    if line.trim_end().ends_with('{') {
        for peek in &lines[line_idx + 1..line_idx + 4] {
            let t = peek.trim();
            if t.is_empty() {
                continue;
            }
            if t == "}" {
                break;
            }
            // First non-empty, non-brace line is the action body
            let expr: String = t.chars().take_while(|&c| c != '\n').collect();
            let expr = expr.trim().to_string();
            if !expr.is_empty() {
                return Some(expr);
            }
        }
    }

    None
}

/// Recursively group flat entries into a tree.
fn build_tree(entries: &[FlatEntry], base_depth: usize) -> (Vec<UIPreviewNode>, usize) {
    let mut nodes = Vec::new();
    let mut i = 0;

    while i < entries.len() {
        let entry = &entries[i];
        if entry.depth < base_depth {
            break;
        }
        if entry.depth == base_depth {
            let child_entries = &entries[i + 1..];
            let (children, consumed) = build_tree(child_entries, base_depth + 1);
            nodes.push(UIPreviewNode {
                r#type: entry.r#type.clone(),
                category: entry.category.clone(),
                label: entry.label.clone(),
                children,
                source_line: Some(entry.source_line),
                action_handler: entry.action_handler.clone(),
            });
            i += 1 + consumed;
        } else {
            i += 1;
        }
    }
    (nodes, i)
}
