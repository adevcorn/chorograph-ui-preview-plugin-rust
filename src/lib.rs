use chorograph_plugin_sdk_rust::ffi;
use chorograph_plugin_sdk_rust::prelude::*;

mod model;
mod react_parser;
mod swiftui_parser;

use model::UIPreviewPayload;

#[chorograph_plugin]
fn init() {
    log!("[ui-preview] plugin loaded");
}

/// Called after LSP is ready. Scans all Swift/TSX/JSX files under workspaceRoot
/// using a direct filesystem walk (find subprocess) — does not rely on LSP symbols.
#[chorograph_plugin]
fn handle_action(action_id: String, payload: serde_json::Value) {
    if action_id != "identifyUIComponents" {
        return;
    }

    let workspace_root = payload["workspaceRoot"].as_str().unwrap_or("").to_string();
    if workspace_root.is_empty() {
        log!("[ui-preview] identifyUIComponents: no workspaceRoot in payload");
        return;
    }

    log!("[ui-preview] scanning {} for UI components", workspace_root);

    let paths = find_source_files(&workspace_root);
    log!("[ui-preview] find returned {} candidate files", paths.len());

    let mut found = 0;
    for path in &paths {
        let ext = path.rsplit('.').next().unwrap_or("");
        match ext {
            "swift" => {
                if let Some(payload) = scan_swift_file(path) {
                    emit_preview(&payload);
                    found += 1;
                }
            }
            "tsx" | "jsx" => {
                if let Some(payload) = scan_react_file(path) {
                    emit_preview(&payload);
                    found += 1;
                }
            }
            _ => {}
        }
    }

    log!("[ui-preview] found {} UI component file(s)", found);
}

// ---------------------------------------------------------------------------
// File discovery via `find` subprocess
// ---------------------------------------------------------------------------

fn find_source_files(root: &str) -> Vec<String> {
    // find <root> -type f \( -name "*.swift" -o -name "*.tsx" -o -name "*.jsx" \)
    // Exclude common noise dirs: .build, node_modules, .git, DerivedData, Pods
    let proc = match ChildProcess::spawn(
        "find",
        vec![
            root,
            "-type",
            "f",
            "(",
            "-name",
            "*.swift",
            "-o",
            "-name",
            "*.tsx",
            "-o",
            "-name",
            "*.jsx",
            ")",
            "-not",
            "-path",
            "*/.build/*",
            "-not",
            "-path",
            "*/node_modules/*",
            "-not",
            "-path",
            "*/.git/*",
            "-not",
            "-path",
            "*/DerivedData/*",
            "-not",
            "-path",
            "*/Pods/*",
            "-not",
            "-path",
            "*/build/*",
        ],
        Some(root),
        std::collections::HashMap::new(),
    ) {
        Ok(p) => p,
        Err(e) => {
            log!("[ui-preview] find spawn failed: {:?}", e);
            return vec![];
        }
    };

    // Collect stdout with a generous timeout loop
    let mut output = Vec::new();
    loop {
        if proc.wait_for_data(2000) {
            match proc.read(PipeType::Stdout) {
                Ok(ReadResult::Data(bytes)) => output.extend_from_slice(&bytes),
                Ok(ReadResult::EOF) => break,
                Ok(ReadResult::Empty) => {}
                Err(_) => break,
            }
        } else {
            // Check if the process has finished even if wait_for_data timed out
            match proc.get_status() {
                ProcessStatus::Running => continue,
                _ => break,
            }
        }
    }
    // Drain any remaining buffered output after exit
    loop {
        match proc.read(PipeType::Stdout) {
            Ok(ReadResult::Data(bytes)) => output.extend_from_slice(&bytes),
            _ => break,
        }
    }

    String::from_utf8_lossy(&output)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Per-file scanners
// ---------------------------------------------------------------------------

fn scan_swift_file(path: &str) -> Option<UIPreviewPayload> {
    let source = read_host_file(path).ok()?;
    let root = swiftui_parser::parse_swiftui(&source)?;
    let component_name = root.r#type.clone();
    Some(UIPreviewPayload {
        file_path: path.to_string(),
        framework: "swiftui".to_string(),
        component_name,
        root,
    })
}

fn scan_react_file(path: &str) -> Option<UIPreviewPayload> {
    let source = read_host_file(path).ok()?;
    let root = react_parser::parse_react(&source)?;
    let component_name = root.r#type.clone();
    Some(UIPreviewPayload {
        file_path: path.to_string(),
        framework: "react".to_string(),
        component_name,
        root,
    })
}

// ---------------------------------------------------------------------------
// Event emission
// ---------------------------------------------------------------------------

fn emit_preview(payload: &UIPreviewPayload) {
    if let Ok(json) = serde_json::to_string(payload) {
        if let Ok(mut map) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
        {
            map.insert(
                "type".to_string(),
                serde_json::Value::String("uiPreview".to_string()),
            );
            if let Ok(envelope) = serde_json::to_string(&map) {
                log!(
                    "[ui-preview] emitting uiPreview for {} ({} bytes)",
                    payload.file_path,
                    envelope.len()
                );
                unsafe {
                    ffi::host_push_ai_event(
                        "".as_ptr(),
                        0,
                        envelope.as_ptr(),
                        envelope.len() as i32,
                    );
                }
            }
        }
    }
}
