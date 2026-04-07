/// React / TSX component tree heuristic parser.
///
/// Strategy:
/// 1. Find functional components: `function Foo(` or `const Foo = (` or
///    `const Foo: React.FC` where name starts with uppercase.
/// 2. Inside the component body locate the JSX return block (`return (` or
///    `return <`).
/// 3. Walk JSX tags tracking angle-bracket depth, emitting component nodes.
use crate::model::{UIPreviewCategory, UIPreviewNode};

// ---------------------------------------------------------------------------
// Component classification
// ---------------------------------------------------------------------------

fn classify_jsx(tag: &str) -> UIPreviewCategory {
    match tag {
        // Layout containers (HTML + React Native)
        "div"
        | "main"
        | "section"
        | "article"
        | "aside"
        | "header"
        | "footer"
        | "nav"
        | "ul"
        | "ol"
        | "table"
        | "thead"
        | "tbody"
        | "tr"
        | "View"
        | "ScrollView"
        | "FlatList"
        | "SectionList"
        | "SafeAreaView"
        | "KeyboardAvoidingView"
        | "VStack"
        | "HStack"
        | "Box"
        | "Flex"
        | "Grid"
        | "Stack"
        | "Container"
        | "Row"
        | "Col"
        | "Column"
        | "Fragment" => UIPreviewCategory::Container,

        // Interactive
        "button"
        | "Button"
        | "input"
        | "Input"
        | "select"
        | "textarea"
        | "form"
        | "Form"
        | "a"
        | "Link"
        | "NavLink"
        | "Toggle"
        | "Switch"
        | "Checkbox"
        | "Radio"
        | "Slider"
        | "DatePicker"
        | "Pressable"
        | "TouchableOpacity"
        | "TouchableHighlight"
        | "TouchableNativeFeedback"
        | "TextInput" => UIPreviewCategory::Interactive,

        // Display
        "p" | "span" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "img" | "Image" | "Text"
        | "Label" | "Icon" | "Badge" | "Avatar" | "Divider" | "hr" | "br" | "strong" | "em"
        | "code" | "pre" | "blockquote" | "caption" | "ProgressBar" | "ActivityIndicator"
        | "Spinner" => UIPreviewCategory::Display,

        _ if tag.chars().next().map_or(false, |c| c.is_uppercase()) => {
            // Unknown custom component — treat as container (it probably wraps children).
            UIPreviewCategory::Container
        }
        _ => UIPreviewCategory::Display,
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn parse_react(source: &str) -> Option<UIPreviewNode> {
    let lines: Vec<&str> = source.lines().collect();

    // Find the first functional component declaration.
    let (comp_name, body_start) = find_component(&lines)?;

    // Find the JSX return block within the component.
    let jsx_start = find_jsx_return(&lines, body_start)?;

    // Parse JSX tags into a tree.
    let root_children = parse_jsx_block(&lines, jsx_start);

    Some(UIPreviewNode {
        r#type: comp_name,
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

fn find_component(lines: &[&str]) -> Option<(String, usize)> {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // `export default function Foo(` or `function Foo(`
        if let Some(name) = extract_function_component(trimmed) {
            return Some((name, i));
        }
        // `export const Foo = (` or `const Foo: React.FC`
        if let Some(name) = extract_arrow_component(trimmed) {
            return Some((name, i));
        }
    }
    None
}

fn extract_function_component(line: &str) -> Option<String> {
    let stripped = line
        .trim_start_matches("export default ")
        .trim_start_matches("export ");
    if stripped.starts_with("function ") {
        let after = stripped.trim_start_matches("function ").trim();
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.chars().next()?.is_uppercase() {
            return Some(name);
        }
    }
    None
}

fn extract_arrow_component(line: &str) -> Option<String> {
    let stripped = line
        .trim_start_matches("export default ")
        .trim_start_matches("export ");
    if stripped.starts_with("const ") {
        let after = stripped.trim_start_matches("const ").trim();
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.chars().next()?.is_uppercase()
            && (line.contains("= (")
                || line.contains("= () =>")
                || line.contains(": React.FC")
                || line.contains(": FC<"))
        {
            return Some(name);
        }
    }
    None
}

fn find_jsx_return(lines: &[&str], from: usize) -> Option<usize> {
    for i in from..lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("return (") || trimmed.starts_with("return <") {
            return Some(i);
        }
    }
    None
}

/// Extract an action handler identifier from a JSX tag string.
/// Looks for `onClick={...}`, `onPress={...}`, `onChange={...}`,
/// `onSubmit={...}`, `onValueChange={...}`.
fn extract_jsx_handler(tag_str: &str) -> Option<String> {
    for prop in &[
        "onClick",
        "onPress",
        "onChange",
        "onSubmit",
        "onValueChange",
        "onChangeText",
        "onBlur",
        "onFocus",
    ] {
        let pattern = format!("{}={{", prop);
        if let Some(start) = tag_str.find(&pattern) {
            let rest = &tag_str[start + pattern.len()..];
            // Extract the expression inside { ... }.
            // Handle simple identifiers: {handleFoo} or {props.onFoo}
            // Also arrow functions: {() => doSomething()} → extract "doSomething"
            let inner: String = rest.chars().take_while(|&c| c != '}').collect();
            let inner = inner.trim();
            if inner.is_empty() {
                continue;
            }
            // Arrow function: strip leading `() => ` / `(e) => ` etc.
            let handler = if let Some(arrow_pos) = inner.find("=>") {
                inner[arrow_pos + 2..].trim().to_string()
            } else {
                inner.to_string()
            };
            let handler = handler.trim_matches(|c: char| c == ' ' || c == '{' || c == '}');
            if !handler.is_empty() {
                return Some(handler.to_string());
            }
        }
    }
    None
}

/// Parse JSX tags from `start_line` onward, building a component tree.
/// We track JSX tag nesting via a stack.
fn parse_jsx_block(lines: &[&str], start: usize) -> Vec<UIPreviewNode> {
    // Stack of (tag_name, children_so_far, source_line_1based).
    let mut stack: Vec<(String, Vec<UIPreviewNode>, u32)> = Vec::new();
    // The outermost children (returned when stack is empty again).
    let mut root: Vec<UIPreviewNode> = Vec::new();
    // Simple brace depth to detect end of return block.
    let mut paren_depth: i32 = 0;

    for (line_offset, line) in lines[start..].iter().enumerate() {
        let abs_line = (start + line_offset + 1) as u32; // 1-based
        let trimmed = line.trim();

        // Track parenthesis depth for the return ( ... ) wrapper.
        for ch in trimmed.chars() {
            match ch {
                '(' => paren_depth += 1,
                ')' => {
                    paren_depth -= 1;
                    if paren_depth < 0 {
                        return root;
                    }
                }
                _ => {}
            }
        }

        // Extract opening and closing JSX tags from the line.
        let mut pos = 0;
        let bytes = trimmed.as_bytes();
        while pos < bytes.len() {
            if bytes[pos] == b'<' {
                // Skip `{/* comment */}` and `<!` / `<>` / `</>`.
                let rest = &trimmed[pos..];
                if rest.starts_with("</") {
                    // Closing tag — pop the stack.
                    let tag = extract_close_tag(rest);
                    if !tag.is_empty() {
                        if let Some((popped_name, children, open_line)) = stack.pop() {
                            let node = UIPreviewNode {
                                r#type: popped_name,
                                category: classify_jsx(&tag),
                                label: None,
                                children,
                                source_line: Some(open_line),
                                action_handler: None,
                            };
                            if let Some(parent) = stack.last_mut() {
                                parent.1.push(node);
                            } else {
                                root.push(node);
                            }
                        }
                    }
                    pos += tag.len() + 3; // </tag>
                } else if rest.starts_with("<>") || rest.starts_with("</>") {
                    pos += 1;
                } else if rest.starts_with("<!--") || rest.starts_with("{/*") {
                    pos += 1;
                } else {
                    // Opening tag.
                    let (tag, self_closing) = extract_open_tag(rest);
                    if !tag.is_empty() && tag != "!" {
                        let label = extract_jsx_text_prop(rest);
                        let action_handler = extract_jsx_handler(rest);
                        if self_closing {
                            let node = UIPreviewNode {
                                r#type: tag.clone(),
                                category: classify_jsx(&tag),
                                label,
                                children: vec![],
                                source_line: Some(abs_line),
                                action_handler,
                            };
                            if let Some(parent) = stack.last_mut() {
                                parent.1.push(node);
                            } else {
                                root.push(node);
                            }
                        } else {
                            let tag_len = tag.len();
                            stack.push((tag, vec![], abs_line));
                            pos += tag_len + 1;
                            continue;
                        }
                        pos += tag.len() + 1;
                    } else {
                        pos += 1;
                    }
                }
            } else {
                pos += 1;
            }
        }
    }
    root
}

fn extract_open_tag(s: &str) -> (String, bool) {
    // s starts with '<'
    let inner = &s[1..];
    let tag: String = inner
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    // Self-closing if the line portion up to '>' contains '/>'
    let self_closing = s.contains("/>")
        && s.find("/>").unwrap_or(usize::MAX) < s.find('>').unwrap_or(usize::MAX) + 1;
    (tag, self_closing)
}

fn extract_close_tag(s: &str) -> String {
    // s starts with '</'
    let inner = &s[2..];
    inner
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect()
}

/// Try to extract a text/title prop value from the opening tag string.
fn extract_jsx_text_prop(tag_str: &str) -> Option<String> {
    // Look for `title="..."` or `label="..."` or plain `"..."` content.
    for prop in &[
        "title",
        "label",
        "placeholder",
        "text",
        "name",
        "aria-label",
    ] {
        let pattern = format!("{}=\"", prop);
        if let Some(start) = tag_str.find(&pattern) {
            let rest = &tag_str[start + pattern.len()..];
            if let Some(end) = rest.find('"') {
                let val = &rest[..end];
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}
