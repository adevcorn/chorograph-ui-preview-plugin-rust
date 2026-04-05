/// SwiftUI component tree heuristic parser.
///
/// Strategy:
/// 1. Find structs conforming to `View` (`: View` or `: some View` in the declaration).
/// 2. For each such struct, walk the body line-by-line tracking brace depth.
/// 3. Emit component nodes for known SwiftUI types, inferring label from the first
///    string literal argument or the variable name on the left side of `=`.
use crate::model::{UIPreviewCategory, UIPreviewNode};

// ---------------------------------------------------------------------------
// Known component classifications
// ---------------------------------------------------------------------------

struct ComponentInfo {
    category: UIPreviewCategory,
    /// True if this is a container that can hold children (increases depth expectation).
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

    // Find the first struct / class declaration that conforms to View.
    let (struct_name, body_start) = find_view_struct(&lines)?;

    // Walk the body and build a flat list of (depth, component) pairs.
    let components = extract_components(&lines, body_start);

    // Build the tree from the flat list.
    let root_children = build_tree(&components, 0).0;

    Some(UIPreviewNode {
        r#type: struct_name,
        category: UIPreviewCategory::Container,
        label: None,
        children: root_children,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Returns (struct_name, line_index_of_opening_brace).
fn find_view_struct(lines: &[&str]) -> Option<(String, usize)> {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Match `struct Foo: View` or `struct Foo: SomeProtocol, View`
        if (trimmed.starts_with("struct ") || trimmed.starts_with("class "))
            && (trimmed.contains(": View")
                || trimmed.contains(": some View")
                || trimmed.contains(", View"))
        {
            // Extract the struct name.
            let after_keyword = trimmed
                .trim_start_matches("struct ")
                .trim_start_matches("class ");
            let name = after_keyword
                .split(|c: char| c == ':' || c == '<' || c.is_whitespace())
                .next()
                .unwrap_or("View")
                .to_string();

            // Find the opening brace (may be on the same line or next).
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
}

fn extract_components(lines: &[&str], start: usize) -> Vec<FlatEntry> {
    let mut entries = Vec::new();
    // depth tracks brace nesting relative to the opening brace of the view body.
    // We start *inside* the body so the first { is already consumed.
    let mut depth: i32 = 0;
    let mut inside = false;

    for line in &lines[start..] {
        let trimmed = line.trim();

        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    if !inside {
                        inside = true; // consume the struct's own opening brace
                    } else {
                        depth += 1;
                    }
                }
                '}' => {
                    if depth == 0 {
                        return entries; // end of the view body
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }

        if !inside {
            continue;
        }

        // Try to detect a component invocation on this line.
        // Pattern: optional leading whitespace + TypeName( or TypeName {
        let token = trimmed
            .split(|c: char| c == '(' || c == '{' || c == ' ')
            .next()
            .unwrap_or("")
            .trim();

        if let Some(info) = classify(token) {
            let label = extract_label(trimmed);
            entries.push(FlatEntry {
                depth: depth as usize,
                r#type: token.to_string(),
                label,
                category: info.category,
            });
        }
    }
    entries
}

/// Extract a display label from a component invocation line.
/// Strategy: grab the first double-quoted string literal argument.
fn extract_label(line: &str) -> Option<String> {
    // Find the first `"..."` in the line.
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

/// Recursively group flat entries into a tree.
/// Returns (children, next_index).
fn build_tree(entries: &[FlatEntry], base_depth: usize) -> (Vec<UIPreviewNode>, usize) {
    let mut nodes = Vec::new();
    let mut i = 0;

    while i < entries.len() {
        let entry = &entries[i];
        if entry.depth < base_depth {
            break;
        }
        if entry.depth == base_depth {
            // Collect children that are deeper.
            let child_entries = &entries[i + 1..];
            let (children, consumed) = build_tree(child_entries, base_depth + 1);
            nodes.push(UIPreviewNode {
                r#type: entry.r#type.clone(),
                category: entry.category.clone(),
                label: entry.label.clone(),
                children,
            });
            i += 1 + consumed;
        } else {
            // Deeper than expected — skip (already consumed by a recursive call).
            i += 1;
        }
    }
    (nodes, i)
}
