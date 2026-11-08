//! Serveur MCP stdio pour Unity Package Extractor.
//! Outils: list_packages, extract_package, extract_queue

use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use unity_extractor::{
    extract_unitypackage, resolve_output_dir, ExtractOptions, QueueItem,
};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Notifications: pas de réponse
        if msg.get("id").is_none() {
            continue;
        }

        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));

        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "unity-extractor",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Extracteur .unitypackage. Outils: list_packages, extract_package, extract_queue. Permet de choisir folder_name et une file d'attente."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_defs() })),
            "tools/call" => handle_tool_call(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err(json!({
                "code": -32601,
                "message": format!("Method not found: {other}")
            })),
        };

        let response = match result {
            Ok(value) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": value
            }),
            Err(err) => {
                // tools/call may return isError content instead of protocol error
                if err.get("isErrorContent").and_then(|v| v.as_bool()) == Some(true) {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": err.get("content").cloned().unwrap_or(json!([])),
                            "isError": true
                        }
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": err
                    })
                }
            }
        };

        // MCP stdio exige un \n seul (pas \r\n Windows)
        let payload = format!("{response}\n");
        if stdout.write_all(payload.as_bytes()).is_err() {
            break;
        }
        let _ = stdout.flush();
    }
}

fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_packages",
            "description": "Liste les fichiers .unitypackage dans un dossier (non récursif).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "Chemin du dossier à scanner"
                    }
                },
                "required": ["directory"]
            }
        }),
        json!({
            "name": "extract_package",
            "description": "Extrait un .unitypackage vers un dossier. folder_name permet de choisir le nom du dossier créé.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "package_path": {
                        "type": "string",
                        "description": "Chemin du fichier .unitypackage"
                    },
                    "output_parent": {
                        "type": "string",
                        "description": "Dossier parent où créer le dossier extrait (défaut: même dossier que le package)"
                    },
                    "folder_name": {
                        "type": "string",
                        "description": "Nom du dossier de sortie (défaut: nom du package sans extension)"
                    },
                    "include_meta": {
                        "type": "boolean",
                        "description": "Inclure les fichiers .meta (défaut: true)"
                    },
                    "include_preview": {
                        "type": "boolean",
                        "description": "Inclure les aperçus preview.png (défaut: false)"
                    }
                },
                "required": ["package_path"]
            }
        }),
        json!({
            "name": "extract_queue",
            "description": "Extrait une file d'attente de plusieurs .unitypackage séquentiellement. Chaque item peut avoir son propre folder_name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "description": "Liste des extractions à enchaîner",
                        "items": {
                            "type": "object",
                            "properties": {
                                "package_path": { "type": "string" },
                                "output_parent": { "type": "string" },
                                "folder_name": { "type": "string" }
                            },
                            "required": ["package_path"]
                        }
                    },
                    "include_meta": { "type": "boolean" },
                    "include_preview": { "type": "boolean" }
                },
                "required": ["items"]
            }
        }),
    ]
}

fn handle_tool_call(params: &Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        "list_packages" => tool_list_packages(&args),
        "extract_package" => tool_extract_package(&args),
        "extract_queue" => tool_extract_queue(&args),
        other => Err(json!({
            "code": -32602,
            "message": format!("Unknown tool: {other}")
        })),
    }
}

fn tool_ok(text: String) -> Result<Value, Value> {
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

fn tool_err(text: String) -> Result<Value, Value> {
    Err(json!({
        "isErrorContent": true,
        "content": [{ "type": "text", "text": text }]
    }))
}

fn tool_list_packages(args: &Value) -> Result<Value, Value> {
    let directory = args
        .get("directory")
        .and_then(|v| v.as_str())
        .ok_or_else(|| json!({"code": -32602, "message": "directory requis"}))?;

    let dir = PathBuf::from(directory);
    if !dir.is_dir() {
        return tool_err(format!("Dossier introuvable: {directory}"));
    }

    let mut packages = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("unitypackage"))
                == Some(true)
            {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                packages.push(json!({
                    "path": path.to_string_lossy(),
                    "name": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    "size_bytes": size,
                    "default_folder_name": path.file_stem().and_then(|s| s.to_str()).unwrap_or("extracted")
                }));
            }
        }
    }
    packages.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    tool_ok(serde_json::to_string_pretty(&json!({
        "directory": directory,
        "count": packages.len(),
        "packages": packages
    })).unwrap_or_else(|_| "{}".into()))
}

#[derive(Debug, Deserialize)]
struct ExtractArgs {
    package_path: String,
    output_parent: Option<String>,
    folder_name: Option<String>,
    include_meta: Option<bool>,
    include_preview: Option<bool>,
}

fn tool_extract_package(args: &Value) -> Result<Value, Value> {
    let parsed: ExtractArgs = serde_json::from_value(args.clone()).map_err(|e| {
        json!({"code": -32602, "message": format!("Arguments invalides: {e}")})
    })?;

    let item = build_queue_item(
        &parsed.package_path,
        parsed.output_parent.as_deref(),
        parsed.folder_name.as_deref(),
    )?;

    let options = ExtractOptions {
        include_meta: parsed.include_meta.unwrap_or(true),
        include_preview: parsed.include_preview.unwrap_or(false),
    };

    run_one(&item, &options)
}

#[derive(Debug, Deserialize)]
struct QueueArgs {
    items: Vec<QueueItemArgs>,
    include_meta: Option<bool>,
    include_preview: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct QueueItemArgs {
    package_path: String,
    output_parent: Option<String>,
    folder_name: Option<String>,
}

fn tool_extract_queue(args: &Value) -> Result<Value, Value> {
    let parsed: QueueArgs = serde_json::from_value(args.clone()).map_err(|e| {
        json!({"code": -32602, "message": format!("Arguments invalides: {e}")})
    })?;

    if parsed.items.is_empty() {
        return tool_err("La file d'attente est vide.".into());
    }

    let options = ExtractOptions {
        include_meta: parsed.include_meta.unwrap_or(true),
        include_preview: parsed.include_preview.unwrap_or(false),
    };

    let mut results = Vec::new();
    let mut failures = 0usize;

    for (i, raw) in parsed.items.iter().enumerate() {
        let item = match build_queue_item(
            &raw.package_path,
            raw.output_parent.as_deref(),
            raw.folder_name.as_deref(),
        ) {
            Ok(item) => item,
            Err(err) => {
                failures += 1;
                results.push(json!({
                    "index": i,
                    "ok": false,
                    "error": err.get("content").and_then(|c| c[0]["text"].as_str()).unwrap_or("erreur")
                }));
                continue;
            }
        };

        match run_extract_report(&item, &options) {
            Ok(report) => {
                results.push(json!({
                    "index": i,
                    "ok": true,
                    "package": item.package_path.to_string_lossy(),
                    "output_dir": report.output_dir.to_string_lossy(),
                    "folder_name": item.folder_name,
                    "extracted": report.extracted,
                    "skipped": report.skipped,
                    "warnings": report.errors.len()
                }));
            }
            Err(err) => {
                failures += 1;
                results.push(json!({
                    "index": i,
                    "ok": false,
                    "package": item.package_path.to_string_lossy(),
                    "folder_name": item.folder_name,
                    "error": err
                }));
            }
        }
    }

    let summary = json!({
        "total": parsed.items.len(),
        "success": parsed.items.len() - failures,
        "failed": failures,
        "results": results
    });

    if failures > 0 {
        tool_err(serde_json::to_string_pretty(&summary).unwrap_or_default())
    } else {
        tool_ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }
}

fn build_queue_item(
    package_path: &str,
    output_parent: Option<&str>,
    folder_name: Option<&str>,
) -> Result<QueueItem, Value> {
    let package = PathBuf::from(package_path);
    if !package.exists() {
        return Err(json!({
            "isErrorContent": true,
            "content": [{ "type": "text", "text": format!("Package introuvable: {package_path}") }]
        }));
    }

    let mut item = QueueItem::from_package(package, output_parent.map(PathBuf::from));
    if let Some(name) = folder_name {
        if !name.trim().is_empty() {
            item.folder_name = name.trim().to_string();
        }
    }
    // Assure que le parent existe conceptuellement
    let _ = resolve_output_dir(&item.output_parent, &item.folder_name);
    Ok(item)
}

fn run_one(item: &QueueItem, options: &ExtractOptions) -> Result<Value, Value> {
    match run_extract_report(item, options) {
        Ok(report) => tool_ok(
            serde_json::to_string_pretty(&json!({
                "ok": true,
                "package": item.package_path.to_string_lossy(),
                "folder_name": item.folder_name,
                "output_dir": report.output_dir.to_string_lossy(),
                "extracted": report.extracted,
                "skipped": report.skipped,
                "warnings": report.errors.len(),
                "sample_warnings": report.errors.iter().take(10).cloned().collect::<Vec<_>>()
            }))
            .unwrap_or_default(),
        ),
        Err(err) => tool_err(err),
    }
}

fn run_extract_report(
    item: &QueueItem,
    options: &ExtractOptions,
) -> Result<unity_extractor::ExtractReport, String> {
    let output = item.output_dir();
    extract_unitypackage(
        Path::new(&item.package_path),
        &output,
        options,
        |_p, _m| {},
    )
    .map_err(|e| e.to_string())
}
