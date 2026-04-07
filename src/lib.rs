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
    match action_id.as_str() {
        "identifyUIComponents" => handle_identify(payload),
        "invokeUIAction" => handle_invoke(payload),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// identifyUIComponents
// ---------------------------------------------------------------------------

fn handle_identify(payload: serde_json::Value) {
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
// invokeUIAction
// ---------------------------------------------------------------------------

fn handle_invoke(payload: serde_json::Value) {
    let handler = payload["handler"].as_str().unwrap_or("").to_string();
    let file_path = payload["filePath"].as_str().unwrap_or("").to_string();
    let framework = payload["framework"]
        .as_str()
        .unwrap_or("swiftui")
        .to_string();

    if handler.is_empty() {
        log!("[ui-preview] invokeUIAction: missing handler");
        return;
    }

    log!(
        "[ui-preview] invokeUIAction handler={} framework={} file={}",
        handler,
        framework,
        file_path
    );

    let (status, output) = match framework.as_str() {
        "react" => invoke_react_handler(&handler),
        _ => invoke_swiftui_handler(&handler, &file_path),
    };

    emit_ui_action_result(&handler, &status, &output);
}

/// Sanitise a handler name so it is safe to embed in a Darwin notification name.
/// Strips everything except alphanumerics, `.`, `_`, and `-`.
fn sanitise_handler(raw: &str) -> String {
    // Use the first word/identifier if the handler is an expression like "viewModel.save()"
    let base = raw
        .split(|c: char| c == '(' || c == ' ' || c == '{')
        .next()
        .unwrap_or(raw);
    base.chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}

/// Invoke a SwiftUI action via Darwin notification; LLDB fallback if notifyd fails.
fn invoke_swiftui_handler(handler: &str, _file_path: &str) -> (String, String) {
    let notif_name = format!("com.chorograph.uiAction.{}", sanitise_handler(handler));

    // --- Attempt 1: notifyd_post (fire-and-forget Darwin notification) ---
    let notif_ok = run_command("notifyd_post", &[&notif_name]);
    if notif_ok.0 {
        log!("[ui-preview] Darwin notif sent: {}", notif_name);
        return (
            "invoked".to_string(),
            format!("Darwin notification: {}", notif_name),
        );
    }

    log!(
        "[ui-preview] notifyd_post failed ({}), trying LLDB fallback",
        notif_ok.1
    );

    // --- Attempt 2: LLDB attach ---
    let pid = {
        let (ok, out) = run_command("pgrep", &["-x", "Simulator"]);
        if ok && !out.trim().is_empty() {
            out.trim().lines().next().unwrap_or("").trim().to_string()
        } else {
            // Try finding any running app with a Simulator-style process
            let (_, out2) = run_command("pgrep", &["-f", "iPhone Simulator"]);
            out2.trim().lines().next().unwrap_or("").trim().to_string()
        }
    };

    if pid.is_empty() {
        log!("[ui-preview] LLDB: no simulator PID found");
        return (
            "error".to_string(),
            "No simulator process found".to_string(),
        );
    }

    log!("[ui-preview] LLDB: attaching to PID {}", pid);
    let expr = format!("expr -l swift -- {}()", handler.trim_end_matches("()"));
    let (lldb_ok, lldb_out) = run_command(
        "xcrun",
        &[
            "lldb",
            "--attach-pid",
            &pid,
            "-o",
            &expr,
            "-o",
            "detach",
            "-o",
            "quit",
        ],
    );

    if lldb_ok {
        ("lldb".to_string(), lldb_out)
    } else {
        ("error".to_string(), lldb_out)
    }
}

/// Invoke a React action via Metro CDP (Node.js WebSocket evaluation).
fn invoke_react_handler(handler: &str) -> (String, String) {
    // Spawn node to connect to Metro dev tools and call Runtime.evaluate.
    let script = format!(
        r#"
const ws = require('ws');
fetch('http://localhost:8081/json')
  .then(r => r.json())
  .then(targets => {{
    const t = targets.find(x => x.webSocketDebuggerUrl);
    if (!t) {{ console.error('no CDP target'); process.exit(1); }}
    const c = new ws(t.webSocketDebuggerUrl);
    let id = 1;
    c.on('open', () => {{
      c.send(JSON.stringify({{ id: id++, method: 'Runtime.evaluate', params: {{ expression: '({handler})();', includeCommandLineAPI: false }} }}));
      setTimeout(() => c.close(), 1000);
    }});
    c.on('close', () => process.exit(0));
  }})
  .catch(e => {{ console.error(e.message); process.exit(1); }});
"#,
        handler = handler
    );

    let (ok, out) = run_command("node", &["-e", &script]);
    if ok {
        ("invoked".to_string(), out)
    } else {
        // Best-effort: also try notifyd_post for React Native apps
        let notif_name = format!("com.chorograph.uiAction.{}", sanitise_handler(handler));
        let (notif_ok, _) = run_command("notifyd_post", &[&notif_name]);
        if notif_ok {
            (
                "invoked".to_string(),
                format!("Darwin notification: {}", notif_name),
            )
        } else {
            ("error".to_string(), out)
        }
    }
}

/// Spawn a command and wait for it to finish.  Returns (success, stdout+stderr).
fn run_command(cmd: &str, args: &[&str]) -> (bool, String) {
    let proc = match ChildProcess::spawn(cmd, args.to_vec(), None, std::collections::HashMap::new())
    {
        Ok(p) => p,
        Err(e) => {
            return (false, format!("spawn error: {:?}", e));
        }
    };

    let mut output = Vec::new();
    // Generous 10 s timeout for LLDB
    let deadline = 10;
    let mut waited = 0u32;
    loop {
        if proc.wait_for_data(500) {
            loop {
                match proc.read(PipeType::Stdout) {
                    Ok(ReadResult::Data(bytes)) => output.extend_from_slice(&bytes),
                    Ok(ReadResult::EOF) | Ok(ReadResult::Empty) => break,
                    Err(_) => break,
                }
            }
        }
        match proc.get_status() {
            ProcessStatus::Running => {
                waited += 1;
                if waited > deadline * 2 {
                    break;
                }
            }
            _ => break,
        }
    }
    // Drain remaining
    loop {
        match proc.read(PipeType::Stdout) {
            Ok(ReadResult::Data(bytes)) => output.extend_from_slice(&bytes),
            _ => break,
        }
    }
    let status = proc.get_status();
    let text = String::from_utf8_lossy(&output).to_string();
    let ok = matches!(status, ProcessStatus::Exited(0));
    (ok, text)
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

fn emit_ui_action_result(handler: &str, status: &str, output: &str) {
    let map = serde_json::json!({
        "type":    "uiActionResult",
        "handler": handler,
        "status":  status,
        "output":  output,
    });
    if let Ok(envelope) = serde_json::to_string(&map) {
        log!(
            "[ui-preview] emitting uiActionResult handler={} status={}",
            handler,
            status
        );
        unsafe {
            ffi::host_push_ai_event("".as_ptr(), 0, envelope.as_ptr(), envelope.len() as i32);
        }
    }
}
