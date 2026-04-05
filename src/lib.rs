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

/// Called after LSP is ready so workspace_symbols_from_host is available.
/// Scans all source files in the workspace for SwiftUI views and React
/// components, emits a "uiPreview" event for each one found.
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

    // Collect candidate file paths via workspace symbols.
    // We use the symbol list to find files quickly rather than a directory walk.
    let symbols = match workspace_symbols_from_host(&workspace_root) {
        Ok(s) => s,
        Err(_) => {
            log!("[ui-preview] workspace symbols unavailable — falling back to root files");
            vec![]
        }
    };

    // Deduplicate file paths from symbol list.
    let mut paths: Vec<String> = symbols.iter().map(|s| s.file_path.clone()).collect();
    paths.sort();
    paths.dedup();

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
        // Inject "type":"uiPreview" into the serialised object so TelemetryManager
        // can identify it via json["type"].
        if let Ok(mut map) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
        {
            map.insert(
                "type".to_string(),
                serde_json::Value::String("uiPreview".to_string()),
            );
            if let Ok(envelope) = serde_json::to_string(&map) {
                // Call host_push_ai_event directly with an empty session_id so the
                // envelope lands in onAiEvent() as the raw eventJson string.
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
