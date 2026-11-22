use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Blob, BlobPropertyBag, window, Url, FileSystemFileHandle};
use js_sys::{Array, Object, Reflect, Promise, Uint8Array};
use gloo_timers::callback::Timeout;
use gloo_net::http::Request as GlooRequest;
use oxichrome::{log, storage, tabs};
use regex::Regex;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, Hash)]
struct CrateInfo {
    name: String,
    version: String,
    description: String,
    dependencies: String,
    updated: String,
    url: String,
    timestamp: String,
    // Enhanced metadata from API
    repository: Option<String>,
    documentation: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    downloads: Option<u64>,
    recent_downloads: Option<u64>,
    categories: Vec<String>,
    keywords: Vec<String>,
    rust_version: Option<String>,
    edition: Option<String>,
    crate_size: Option<u64>,
    // Dependency graph
    dependency_graph: Option<DependencyGraph>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, Hash)]
struct DependencyGraph {
    nodes: Vec<DependencyNode>,
    edges: Vec<DependencyEdge>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, Hash)]
struct DependencyNode {
    id: String,
    name: String,
    version: String,
    kind: String, // "normal", "dev", "build"
    optional: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, Hash)]
struct DependencyEdge {
    from: String,
    to: String,
}

impl CrateInfo {
    fn dedup_key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Settings {
    auto_save: bool,
    save_format: String,
    export_filename: String,
}

impl Settings {
    fn default() -> Self {
        Self {
            auto_save: true,
            save_format: "toml".to_string(),
            export_filename: "Cargo.toml".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct SavedCratesData {
    crates: Vec<CrateInfo>,
    settings: Settings,
}

const STORAGE_KEY: &str = "cargo_packer_data";
const CRATES_IO_API: &str = "https://crates.io/api/v1";
const USER_AGENT: &str = "Cargo-Packer/1.0 (https://github.com/BlancoBAM/Cargo-Packer)";

// API response types
#[derive(Deserialize, Debug)]
struct CrateResponse {
    #[serde(rename = "crate")]
    crate_data: ApiCrate,
    versions: Option<Vec<ApiVersion>>,
}

#[derive(Deserialize, Debug)]
struct VersionResponse {
    version: ApiVersion,
}

#[derive(Deserialize, Debug)]
struct DependenciesResponse {
    dependencies: Vec<ApiDependency>,
}

#[derive(Deserialize, Debug)]
struct ApiCrate {
    id: String,
    name: String,
    description: Option<String>,
    documentation: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    downloads: u64,
    recent_downloads: u64,
    max_stable_version: String,
    categories: Vec<ApiCategory>,
    keywords: Vec<ApiKeyword>,
}

#[derive(Deserialize, Debug, Clone)]
struct ApiVersion {
    id: String,
    crate_id: String,
    num: String,
    downloads: u64,
    yanked: bool,
    license: Option<String>,
    rust_version: Option<String>,
    edition: Option<String>,
    crate_size: Option<u64>,
    features: serde_json::Value,
    published_at: String,
    updated_at: String,
}

#[derive(Deserialize, Debug)]
struct ApiDependency {
    crate_id: String,
    name: String,
    req: String,
    kind: Option<String>,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
    target: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ApiCategory {
    category: String,
    slug: String,
}

#[derive(Deserialize, Debug)]
struct ApiKeyword {
    keyword: String,
}

// Fetch crate metadata from crates.io API
async fn fetch_crate_metadata(name: &str, version: Option<&str>) -> Result<CrateInfo, String> {
    let version_path = version.map(|v| format!("/{}", v)).unwrap_or_default();
    let url = format!("{}/crates/{}{}", CRATES_IO_API, name, version_path);
    
    let response = GlooRequest::get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    
    if !response.ok() {
        return Err(format!("API error: {}", response.status()));
    }
    
    let crate_resp: CrateResponse = response
        .json()
        .await
        .map_err(|e| format!("JSON parse failed: {}", e))?;
    
    let api_crate = crate_resp.crate_data;
    let versions = crate_resp.versions.unwrap_or_default();
    
    // Determine which version to use
    let target_version = if let Some(v) = version {
        versions.iter().find(|ver| ver.num == v).cloned()
    } else {
        versions.iter().find(|ver| ver.num == api_crate.max_stable_version).cloned()
    }.unwrap_or_else(|| versions.first().cloned().unwrap_or_else(|| ApiVersion {
        id: String::new(),
        crate_id: String::new(),
        num: "unknown".to_string(),
        downloads: 0,
        yanked: false,
        license: None,
        rust_version: None,
        edition: None,
        crate_size: None,
        features: serde_json::Value::Null,
        published_at: String::new(),
        updated_at: String::new(),
    }));
    
    // Fetch dependencies for this version
    let deps_url = format!("{}/crates/{}/{}/dependencies", CRATES_IO_API, name, target_version.num);
    let deps_response = GlooRequest::get(&deps_url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("Deps request failed: {}", e))?;
    
    let dependencies = if deps_response.ok() {
        let deps_resp: DependenciesResponse = deps_response
            .json()
            .await
            .map_err(|e| format!("Deps JSON parse failed: {}", e))?;
        deps_resp.dependencies
    } else {
        vec![]
    };
    
    // Build dependency graph
    let (dep_graph, dep_count) = build_dependency_graph(&dependencies);
    
    let categories = api_crate.categories.iter().map(|c| c.slug.clone()).collect();
    let keywords = api_crate.keywords.iter().map(|k| k.keyword.clone()).collect();
    
    Ok(CrateInfo {
        name: api_crate.name.clone(),
        version: target_version.num.clone(),
        description: api_crate.description.unwrap_or_default(),
        dependencies: dep_count.to_string(),
        updated: target_version.updated_at.split('T').next().unwrap_or("").to_string(),
        url: format!("https://crates.io/crates/{}/{}", api_crate.name, target_version.num),
        timestamp: chrono::Utc::now().to_rfc3339(),
        repository: api_crate.repository,
        documentation: api_crate.documentation,
        homepage: api_crate.homepage,
        license: target_version.license,
        downloads: Some(api_crate.downloads),
        recent_downloads: Some(api_crate.recent_downloads),
        categories,
        keywords,
        rust_version: target_version.rust_version,
        edition: target_version.edition,
        crate_size: target_version.crate_size,
        dependency_graph: Some(dep_graph),
    })
}

fn build_dependency_graph(dependencies: &[ApiDependency]) -> (DependencyGraph, usize) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut seen = std::collections::HashSet::new();
    
    // Add root node (will be filled by caller)
    nodes.push(DependencyNode {
        id: "root".to_string(),
        name: String::new(),
        version: String::new(),
        kind: "root".to_string(),
        optional: false,
    });
    
    for dep in dependencies {
        let node_id = format!("{}@{}", dep.name, dep.req);
        if !seen.contains(&node_id) {
            seen.insert(node_id.clone());
            nodes.push(DependencyNode {
                id: node_id.clone(),
                name: dep.name.clone(),
                version: dep.req.clone(),
                kind: dep.kind.clone().unwrap_or_else(|| "normal".to_string()),
                optional: dep.optional,
            });
        }
        edges.push(DependencyEdge {
            from: "root".to_string(),
            to: node_id,
        });
    }
    
    (DependencyGraph { nodes, edges }, dependencies.len())
}

// Extract version from URL if present (e.g., /crates/serde/1.0.0)
fn extract_version_from_url(url: &str) -> Option<String> {
    // Pattern: crates.io/crates/name/version
    let re = Regex::new(r"crates\.io/crates/[^/]+/([^/]+)(?:/|$)").ok()?;
    re.captures(url).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
}

#[oxichrome::extension(
    name = "Crates Packer",
    version = "1.0.0",
    description = "Save crates.io pages to local Cargo.toml files",
    permissions = ["activeTab", "downloads", "storage", "scripting"],
)]
struct CargoPackerExtension;

#[oxichrome::popup]
fn Popup() -> impl IntoView {
    let (current_page, set_current_page) = signal(0);
    let (current_crate, set_current_crate) = signal(None::<CrateInfo>);
    let (saved_crates, set_saved_crates) = signal(Vec::<CrateInfo>::new());
    let (settings, set_settings) = signal(Settings::default());
    let (status, set_status) = signal(String::from("Ready"));
    let (status_type, set_status_type) = signal(String::from(""));
    let (save_btn_text, set_save_btn_text) = signal(String::from("Save Current Crate"));
    let (save_disabled, set_save_disabled) = signal(true);
    let (drag_start, set_drag_start) = signal(None::<f64>);
    let (is_dragging, set_is_dragging) = signal(false);
    let (toml_content, set_toml_content) = signal(String::new());
    let (export_type, set_export_type) = signal(String::from("crates")); // "crates" or "dependency-graph"

    #[derive(Clone, Default)]
    struct PermissionsState {
        downloads: bool,
        scripting: bool,
        downloads_pending: bool,
        scripting_pending: bool,
    }
    let (permissions_state, set_permissions_state) = signal(PermissionsState::default());

    let check_permissions = move || {
        spawn_local(async move {
            let window = window().unwrap();
            let chrome = window.unchecked_ref::<js_sys::Object>();
            if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
                if let Ok(permissions) = Reflect::get(&chrome_obj, &"permissions".into()) {
                    if let Ok(contains_fn) = Reflect::get(&permissions, &"contains".into()) {
                        if let Some(contains_fn) = contains_fn.dyn_ref::<js_sys::Function>() {
                            let check = async move |perm: &str| -> bool {
                                let perm_obj = js_sys::Object::new();
                                Reflect::set(&perm_obj, &"permissions".into(), &js_sys::Array::of1(&perm.into())).unwrap();
                                if let Ok(promise) = contains_fn.call1(&permissions, &perm_obj) {
                                    if let Some(promise) = promise.dyn_ref::<Promise>() {
                                        if let Ok(result) = wasm_bindgen_futures::JsFuture::from(promise.clone()).await {
                                            return result.as_bool().unwrap_or(false);
                                        }
                                    }
                                }
                                false
                            };
                            let downloads = check("downloads").await;
                            let scripting = check("scripting").await;
                            set_permissions_state.set(PermissionsState {
                                downloads,
                                scripting,
                                downloads_pending: false,
                                scripting_pending: false,
                            });
                        }
                    }
                }
            }
        });
    };

    let toggle_permission = move |perm: String, granted: bool| {
        set_permissions_state.update(|state| {
            if perm == "downloads" {
                state.downloads_pending = true;
            } else if perm == "scripting" {
                state.scripting_pending = true;
            }
        });

        spawn_local(async move {
            let window = window().unwrap();
            let chrome = window.unchecked_ref::<js_sys::Object>();
            if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
                if let Ok(permissions) = Reflect::get(&chrome_obj, &"chrome".into()) {
                    let method = if granted { "request" } else { "remove" };
                    if let Ok(fn_obj) = Reflect::get(&permissions, &method.into()) {
                        if let Some(fn_ref) = fn_obj.dyn_ref::<js_sys::Function>() {
                            let perm_obj = js_sys::Object::new();
                            Reflect::set(&perm_obj, &"permissions".into(), &js_sys::Array::of1(&perm.into())).unwrap();
                            let _ = fn_ref.call1(&permissions, &perm_obj);
                        }
                    }
                }
            }
            // Re-check after a short delay
            let _ = Timeout::new(500, move || {
                check_permissions();
            });
        });
    };

    let update_toml_preview = move |crates: Vec<CrateInfo>, filename: String| {
        let content = generate_toml(&crates, &filename);
        set_toml_content.set(content);
    };

    let load_saved_crates = move || {
        let update_preview = update_toml_preview.clone();
        spawn_local(async move {
            match storage::get::<SavedCratesData>(STORAGE_KEY).await {
                Ok(Some(data)) => {
                    set_saved_crates.set(data.crates.clone());
                    set_settings.set(data.settings.clone());
                    update_preview(data.crates, data.settings.export_filename.clone());
                }
                Ok(None) => {
                    let default_data = SavedCratesData {
                        crates: vec![],
                        settings: Settings::default(),
                    };
                    let _ = storage::set(STORAGE_KEY, &default_data).await;
                    update_preview(vec![], "Cargo.toml".to_string());
                }
                Err(e) => {
                    log!("Error loading saved crates: {:?}", e);
                    update_preview(vec![], "Cargo.toml".to_string());
                }
            }
        });
    };

    let get_current_crate_info = move || {
        spawn_local(async move {
            let query_info = serde_json::json!({
                "active": true,
                "currentWindow": true
            });
            match tabs::query::<serde_json::Value, serde_json::Value>(&query_info).await {
                Ok(tabs) => {
                    if let Some(tab) = tabs.first() {
                        if let Some(url) = tab.get("url").and_then(|v| v.as_str()) {
                            if url.contains("crates.io") {
                                if let Some(tab_id) = tab.get("id").and_then(|v| v.as_i64()) {
                                    match tabs::send_message(
                                        tab_id as i32,
                                        &serde_json::json!({ "action": "getCurrentCrate" }),
                                    ).await {
                                        Ok(response) => {
                                            if let Ok(crate_info) = serde_wasm_bindgen::from_value::<CrateInfo>(response) {
                                                let is_dup = saved_crates.get().iter().any(|c| c.dedup_key() == crate_info.dedup_key());
                                                set_current_crate.set(Some(crate_info.clone()));
                                                set_save_disabled.set(is_dup);
                                                set_save_btn_text.set(if is_dup {
                                                    format!("Already saved: {}", crate_info.name)
                                                } else {
                                                    format!("Save {}", crate_info.name)
                                                });
                                            } else {
                                                set_save_disabled.set(true);
                                                set_save_btn_text.set("Not on a crate page".to_string());
                                            }
                                        }
                                        Err(_) => {
                                            set_save_disabled.set(true);
                                            set_save_btn_text.set("Content script not loaded".to_string());
                                        }
                                    }
                                }
                            } else {
                                set_save_disabled.set(true);
                                set_save_btn_text.set("Visit crates.io to save".to_string());
                            }
                        }
                    }
                }
                Err(e) => {
                    log!("Error querying tabs: {:?}", e);
                }
            }
        });
    };

    let save_current_crate = move || {
        let crate_info = current_crate.get();
        if crate_info.is_none() {
            return;
        }
        let crate_info = crate_info.unwrap();
        let is_dup = saved_crates.get().iter().any(|c| c.dedup_key() == crate_info.dedup_key());
        if is_dup {
            set_status.set(format!("{}@{} already saved", crate_info.name, crate_info.version));
            set_status_type.set("error".to_string());
            Timeout::new(3000, move || {
                set_status.set("Ready".to_string());
                set_status_type.set("".to_string());
            }).forget();
            return;
        }
        set_status.set("Saving crate...".to_string());
        set_status_type.set("loading".to_string());
        set_save_disabled.set(true);

        spawn_local(async move {
            let query_info = serde_json::json!({
                "active": true,
                "currentWindow": true
            });
            match tabs::query::<serde_json::Value, serde_json::Value>(&query_info).await {
                Ok(tabs) => {
                    if let Some(tab) = tabs.first() {
                        if let Some(tab_id) = tab.get("id").and_then(|v| v.as_i64()) {
                            match tabs::send_message(
                                tab_id as i32,
                                &serde_json::json!({ "action": "saveCurrentCrate" }),
                            ).await {
                                Ok(_) => {
                                    set_status.set(format!("Saved {}@{}", crate_info.name, crate_info.version));
                                    set_status_type.set("success".to_string());
                                    load_saved_crates();
                                    set_save_btn_text.set(format!("Saved: {}", crate_info.name));
                                    set_save_disabled.set(true);
                                }
                                Err(e) => {
                                    log!("Save error: {:?}", e);
                                    set_status.set("Failed to save crate".to_string());
                                    set_status_type.set("error".to_string());
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log!("Error querying tabs: {:?}", e);
                    set_status.set("Error saving crate".to_string());
                    set_status_type.set("error".to_string());
                }
            }
            set_save_disabled.set(false);
        });
    };

    let try_save_with_picker = async |content: &str, filename: &str, format: &str| -> bool {
        let window = window().unwrap();
        let show_save_picker = Reflect::get(&window, &"showSaveFilePicker".into());
        
        if let Ok(show_save_picker_fn) = show_save_picker {
            if let Some(show_save_picker_fn) = show_save_picker_fn.dyn_ref::<js_sys::Function>() {
                let options = Object::new();
                let types = Array::new();
                let type_obj = Object::new();
                let accept = Object::new();
                
                let mime_type = match format {
                    "toml" => "text/plain",
                    "json" => "application/json",
                    _ => "text/plain",
                };
                
                let extensions = Array::new();
                extensions.push(&format.into());
                Reflect::set(&accept, &mime_type.into(), &extensions).unwrap();
                Reflect::set(&type_obj, &"description".into(), &format!("{} file", format.to_uppercase()).into()).unwrap();
                Reflect::set(&type_obj, &"accept".into(), &accept).unwrap();
                types.push(&type_obj);
                Reflect::set(&options, &"types".into(), &types).unwrap();
                Reflect::set(&options, &"suggestedName".into(), &filename.into()).unwrap();
                
                if let Ok(promise) = show_save_picker_fn.call1(&window, &options) {
                    if let Some(promise) = promise.dyn_ref::<Promise>() {
                        if let Ok(handle_val) = wasm_bindgen_futures::JsFuture::from(promise.clone()).await {
                            if let Some(handle) = handle_val.dyn_ref::<FileSystemFileHandle>() {
                                if let Ok(writable_promise) = Reflect::get(handle, &"createWritable".into()) {
                                    if let Some(writable_fn) = writable_promise.dyn_ref::<js_sys::Function>() {
                                        if let Ok(writable_val) = writable_fn.call0(handle) {
                                            if let Some(writable_promise) = writable_val.dyn_ref::<Promise>() {
                                                if let Ok(writable_val) = wasm_bindgen_futures::JsFuture::from(writable_promise.clone()).await {
                                                    if let Some(writable) = writable_val.dyn_ref::<web_sys::FileSystemWritableFileStream>() {
                                                        let content_bytes = Uint8Array::new_from_slice(content.as_bytes());
                                                        if let Ok(write_promise) = Reflect::get(writable, &"write".into()) {
                                                            if let Some(write_fn) = write_promise.dyn_ref::<js_sys::Function>() {
                                                                if let Ok(_) = write_fn.call1(writable, &content_bytes) {
                                                                    if let Ok(close_promise) = Reflect::get(writable, &"close".into()) {
                                                                        if let Some(close_fn) = close_promise.dyn_ref::<js_sys::Function>() {
                                                                            let _ = close_fn.call0(writable);
                                                                            return true;
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    };

    let export_all_crates = move || {
        let format = settings.get().save_format.clone();
        let filename = settings.get().export_filename.clone();
        let exp_type = export_type.get();
        
        spawn_local(async move {
            match storage::get::<SavedCratesData>(STORAGE_KEY).await {
                Ok(Some(data)) => {
                    if data.crates.is_empty() {
                        set_status.set("No crates to export".to_string());
                        set_status_type.set("error".to_string());
                        return;
                    }
                    
                    let (content, fname) = match (exp_type.as_str(), format.as_str()) {
                        ("dependency-graph", "toml") => (generate_dependency_graph_toml(&data.crates), "dependency-graph.toml".to_string()),
                        ("dependency-graph", "json") => (generate_dependency_graph_json(&data.crates), "dependency-graph.json".to_string()),
                        ("dependency-graph", "dot") => (generate_dependency_graph_dot(&data.crates), "dependency-graph.dot".to_string()),
                        ("crates", "toml") => (generate_toml(&data.crates, &filename), filename),
                        ("crates", "json") => (generate_json(&data.crates), "crates.json".to_string()),
                        ("crates", _) => (generate_text(&data.crates), "crates.txt".to_string()),
                        _ => (generate_toml(&data.crates, &filename), filename),
                    };
                    
                    let saved = try_save_with_picker(&content, &fname, &format).await;
                    
                    if !saved {
                        download_file(&content, &fname).await;
                        set_status.set(format!("Exported {} crates to Downloads/{}", data.crates.len(), fname));
                    } else {
                        set_status.set(format!("Exported {} crates to {}", data.crates.len(), fname));
                    }
                    set_status_type.set("success".to_string());
                }
                Ok(None) => {
                    set_status.set("No crates to export".to_string());
                    set_status_type.set("error".to_string());
                }
                Err(e) => {
                    log!("Export error: {:?}", e);
                    set_status.set("Export failed".to_string());
                    set_status_type.set("error".to_string());
                }
            }
        });
    };

    let clear_all_crates = move || {
        if !window().unwrap().confirm_with_message("Are you sure you want to clear all saved crates?").unwrap_or(false) {
            return;
        }
        set_status.set("Clearing...".to_string());
        set_status_type.set("loading".to_string());

        spawn_local(async move {
            match storage::remove(STORAGE_KEY).await {
                Ok(_) => {
                    set_saved_crates.set(vec![]);
                    set_toml_content.set(String::new());
                    set_status.set("Cleared all saved crates".to_string());
                    set_status_type.set("success".to_string());
                    get_current_crate_info();
                }
                Err(e) => {
                    log!("Clear error: {:?}", e);
                    set_status.set("Failed to clear crates".to_string());
                    set_status_type.set("error".to_string());
                }
            }
        });
    };

    let save_settings = move |new_settings: Settings| {
        set_settings.set(new_settings.clone());
        spawn_local(async move {
            let data = SavedCratesData {
                crates: saved_crates.get(),
                settings: new_settings,
            };
            let _ = storage::set(STORAGE_KEY, &data).await;
            set_status.set("Settings saved".to_string());
            set_status_type.set("success".to_string());
            Timeout::new(1500, move || {
                set_status.set("Ready".to_string());
                set_status_type.set("".to_string());
            }).forget();
        });
    };

    let toggle_auto_save = move |checked: bool| {
        let mut s = settings.get();
        s.auto_save = checked;
        save_settings(s);
    };

    let change_format = move |ev: web_sys::Event| {
        let format = event_target_value(&ev);
        let mut s = settings.get();
        s.save_format = format;
        save_settings(s);
    };

    let change_export_filename = move |ev: web_sys::Event| {
        let filename = event_target_value(&ev);
        let mut s = settings.get();
        s.export_filename = if filename.ends_with(".toml") || filename.ends_with(".json") || filename.ends_with(".txt") {
            filename
        } else {
            format!("{}.{}", filename, settings.get().save_format)
        };
        save_settings(s);
    };

    let last_saved = move || {
        let crates = saved_crates.get();
        if crates.is_empty() {
            "Never".to_string()
        } else {
            let last = crates.last().unwrap();
            chrono::DateTime::parse_from_rfc3339(&last.timestamp)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|_| "Unknown".to_string())
        }
    };

    let handle_drag_start = move |ev: web_sys::DragEvent| {
        if let Some(client_x) = ev.client_x().into() {
            set_drag_start.set(Some(client_x as f64));
            set_is_dragging.set(true);
        }
    };

    let handle_drag_move = move |ev: web_sys::DragEvent| {
        if is_dragging.get() {
            if let Some(client_x) = ev.client_x().into() {
                let start = drag_start.get().unwrap_or(0.0);
                let delta = client_x as f64 - start;
                if delta.abs() > 50.0 {
                    set_current_page.set(if delta > 0.0 { 1 } else { 0 });
                    set_drag_start.set(None);
                    set_is_dragging.set(false);
                }
            }
        }
    };

    let handle_drag_end = move |_ev: web_sys::DragEvent| {
        set_drag_start.set(None);
        set_is_dragging.set(false);
    };

    let prev_page = move |_ev: web_sys::MouseEvent| {
        set_current_page.set(0);
    };

    let next_page = move |_ev: web_sys::MouseEvent| {
        set_current_page.set(1);
    };

    Effect::new(move |_| {
        load_saved_crates();
        get_current_crate_info();
        check_permissions();
        
        let _interval_handle = Timeout::new(2000, move || {
            if let Some(doc) = window().and_then(|w| w.document()) {
                if doc.hidden() == false {
                    get_current_crate_info();
                }
            }
        });
    });

    let page_class = move || {
        if current_page.get() == 0 {
            "page active".to_string()
        } else {
            "page".to_string()
        }
    };

    let toml_page_class = move || {
        if current_page.get() == 1 {
            "page active".to_string()
        } else {
            "page".to_string()
        }
    };

    view! {
        <div class="popup-container">
            <style>{include_str!("popup.css")}</style>
            <div class="pages-container" 
                on:dragstart=handle_drag_start
                on:drag=handle_drag_move
                on:dragend=handle_drag_end
                draggable=true>
                
                <div class={page_class}>
                    <div class="page-content">
                        <div class="header">
                            <h1>"📦 Cargo Packer"</h1>
                            <div class="subtitle">"Save crates.io crates to Cargo.toml"</div>
                        </div>

<div class="crate-info">
    <h3>"Current Crate"</h3>
    <div class="crate-details">
        <div class=move || {
            if let Some(c) = current_crate.get() {
                let is_dup = saved_crates.get().iter().any(|sc| sc.dedup_key() == c.dedup_key());
                if is_dup { "crate-saved" } else { "" }
            } else {
                ""
            }
        }>
<div>
    {move || {
        if let Some(c) = current_crate.get() {
            view! { <><strong>{"Name: ".to_string()}</strong>{c.name.clone()}</> }
        } else {
            view! { <><strong>{"Not on a crate page. Visit crates.io/crates/* to save. ".to_string()}</strong>{String::new()}</> }
        }
    }}
</div>
<div>
    {move || {
        if let Some(c) = current_crate.get() {
            view! { <><strong>{"Version: ".to_string()}</strong>{c.version.clone()}</> }
        } else {
            view! { <><strong>{String::new()}</strong>{String::new()}</> }
        }
    }}
</div>
<div>
    {move || {
        if let Some(c) = current_crate.get() {
            view! { <><strong>{"Description: ".to_string()}</strong>{c.description.clone()}</> }
        } else {
            view! { <><strong>{String::new()}</strong>{String::new()}</> }
        }
    }}
</div>
<div>
    {move || {
        if let Some(c) = current_crate.get() {
            view! { <><strong>{"URL: ".to_string()}</strong>{c.url.clone()}</> }
        } else {
            view! { <><strong>{String::new()}</strong>{String::new()}</> }
        }
    }}
</div>
            <div class="saved-badge" style=move || {
                if let Some(c) = current_crate.get() {
                    let is_dup = saved_crates.get().iter().any(|sc| sc.dedup_key() == c.dedup_key());
                    if is_dup { "visibility: visible;" } else { "visibility: hidden;" }
                } else {
                    "visibility: hidden;"
                }
            }>
                {move || {
                    if let Some(c) = current_crate.get() {
                        let is_dup = saved_crates.get().iter().any(|sc| sc.dedup_key() == c.dedup_key());
                        if is_dup { "✓ Already saved".to_string() } else { String::new() }
                    } else {
                        String::new()
                    }
                }}
            </div>
        </div>
    </div>
</div>

                        <button
                            class="btn primary"
                            disabled=move || save_disabled.get()
                            on:click=move |_| save_current_crate()
                        >
                            {move || save_btn_text.get()}
                        </button>

<div class="settings">
    <h3>"Settings"</h3>

    <div class="setting-item">
        <div class="setting-label">
            <span>"Auto-save new crates"</span>
            <input
                type="checkbox"
                checked=move || settings.get().auto_save
                on:change=move |ev| toggle_auto_save(event_target_checked(&ev))
            />
        </div>
    </div>

    <div class="setting-item">
        <div class="setting-label">
            <span>"Export format"</span>
        </div>
        <select
            prop:value=move || settings.get().save_format.clone()
            on:change=change_format
        >
            <option value="toml">"TOML (Cargo.toml)"</option>
            <option value="json">"JSON"</option>
            <option value="text">"Plain Text"</option>
        </select>
    </div>

    <div class="setting-item">
        <div class="setting-label">
            <span>"Export filename"</span>
        </div>
        <input
            type="text"
            class="filename-input"
            prop:value=move || settings.get().export_filename.clone()
            on:change=change_export_filename
            placeholder="Cargo.toml"
        />
    </div>

    <div class="settings-divider"></div>

    <h4>"Permissions"</h4>

    <div class="setting-item">
        <div class="setting-label">
            <span>"Downloads (optional)"</span>
            <input
                type="checkbox"
                checked=move || permissions_state.get().downloads
                on:change=move |ev| toggle_permission("downloads".to_string(), event_target_checked(&ev))
                disabled=move || permissions_state.get().downloads_pending
            />
        </div>
        <span class="permission-status" 
            style=move || if permissions_state.get().downloads { "color: #4caf50;" } else { "color: #ff9800;" }>
            {move || if permissions_state.get().downloads { "Granted" } else { "Not granted" }}
        </span>
    </div>

    <div class="setting-item">
        <div class="setting-label">
            <span>"Scripting (optional)"</span>
            <input
                type="checkbox"
                checked=move || permissions_state.get().scripting
                on:change=move |ev| toggle_permission("scripting".to_string(), event_target_checked(&ev))
                disabled=move || permissions_state.get().scripting_pending
            />
        </div>
        <span class="permission-status" 
            style=move || if permissions_state.get().scripting { "color: #4caf50;" } else { "color: #ff9800;" }>
            {move || if permissions_state.get().scripting { "Granted" } else { "Not granted" }}
        </span>
    </div>

    <div class="setting-item">
        <div class="setting-label">
            <span>"Host access: crates.io"</span>
        </div>
        <span class="permission-status" style="color: #4caf50;">"Granted (host_permissions)"</span>
    </div>
</div>

                        <div class="button-group">
                            <button class="btn secondary" on:click=move |_| export_all_crates()>
                                "Export All"
                            </button>
                            <button class="btn secondary danger" on:click=move |_| clear_all_crates()>
                                "Clear All"
                            </button>
                        </div>

                        <div class="stats">
                            <span>"Saved: "{move || saved_crates.get().len()}" crates"</span>
                            <span>"Last: "{last_saved}</span>
                        </div>

                        <div class="pagination-indicator">
                            <button class="nav-arrow prev" on:click=prev_page prop:disabled=move || current_page.get() == 0>"‹"</button>
                            <span class="page-dots">
                                <span class={move || if current_page.get() == 0 { "dot active" } else { "dot" }}></span>
                                <span class={move || if current_page.get() == 1 { "dot active" } else { "dot" }}></span>
                            </span>
                            <button class="nav-arrow next" on:click=next_page prop:disabled=move || current_page.get() == 1>"›"</button>
                        </div>

                        <div class={move || format!("status {}", status_type.get())}>
                            {move || status.get()}
                        </div>
                    </div>
                </div>

                <div class={toml_page_class}>
                    <div class="page-content toml-page">
                        <div class="header">
                            <h1>"📄 Cargo.toml Preview"</h1>
                            <div class="subtitle">"Live preview of your dependencies"</div>
                        </div>

<div class="toml-toolbar">
    <div class="filename-input-group">
        <label>"Filename: "</label>
        <input
            type="text"
            class="filename-input"
            prop:value=move || settings.get().export_filename.clone()
            on:change=change_export_filename
            placeholder="Cargo.toml"
        />
    </div>
    <div class="export-type-selector">
        <label>"Export: "</label>
        <select
            prop:value=move || export_type.get()
            on:change=move |ev| set_export_type.set(event_target_value(&ev))
        >
            <option value="crates">"Crates (Cargo.toml)"</option>
            <option value="dependency-graph">"Dependency Graph"</option>
        </select>
    </div>
    <button class="btn secondary" on:click=move |_| export_all_crates()>
        "Export to File..."
    </button>
</div>

                        <div class="toml-preview-container">
                            <div class="carbon-mock">
                                <div class="carbon-header">
                                    <div class="carbon-dots">
                                        <span></span><span></span><span></span>
                                    </div>
                                    <div class="carbon-title">{move || settings.get().export_filename.clone()}</div>
                                </div>
                                <div class="carbon-body">
                                    <pre class="toml-content" spellcheck="false">{move || toml_content.get()}</pre>
                                </div>
                            </div>
                        </div>

                        <div class="toml-stats">
                            <span>"Crates: "{move || saved_crates.get().len()}</span>
                            <span>"Format: "{move || settings.get().save_format.to_uppercase()}</span>
                            <span>"Lines: "{move || toml_content.get().lines().count()}</span>
                        </div>

                        <div class="pagination-indicator">
                            <button class="nav-arrow prev" on:click=prev_page>"‹"</button>
                            <span class="page-dots">
                                <span class={move || if current_page.get() == 0 { "dot active" } else { "dot" }}></span>
                                <span class={move || if current_page.get() == 1 { "dot active" } else { "dot" }}></span>
                            </span>
                            <button class="nav-arrow next" on:click=next_page prop:disabled=move || current_page.get() == 1>"›"</button>
                        </div>
                    </div>
                </div>

            </div>
        </div>
    }
}

fn generate_toml(crates: &[CrateInfo], filename: &str) -> String {
    let mut toml = String::new();
    
    toml.push_str("# Cargo.toml - Generated by Cargo Packer\n");
    toml.push_str(&format!("# Source: crates.io\n"));
    toml.push_str(&format!("# Generated: {}\n", chrono::Utc::now().to_rfc3339()));
    toml.push_str(&format!("# Crates: {}\n\n", crates.len()));
    
    toml.push_str("[package]\n");
    let pkg_name = filename.strip_suffix(".toml").unwrap_or("my-project");
    toml.push_str(&format!("name = \"{}\"\n", pkg_name));
    toml.push_str("version = \"0.1.0\"\n");
    toml.push_str("edition = \"2021\"\n\n");
    
    // Add metadata comments for each crate
    for crate_info in crates {
        toml.push_str(&format!("# === {} v{} ===\n", crate_info.name, crate_info.version));
        if !crate_info.description.is_empty() {
            for line in crate_info.description.lines() {
                toml.push_str(&format!("# {}\n", line));
            }
        }
        if let Some(repo) = &crate_info.repository {
            toml.push_str(&format!("# Repository: {}\n", repo));
        }
        if let Some(doc) = &crate_info.documentation {
            toml.push_str(&format!("# Documentation: {}\n", doc));
        }
        if let Some(license) = &crate_info.license {
            toml.push_str(&format!("# License: {}\n", license));
        }
        if let Some(dl) = crate_info.downloads {
            toml.push_str(&format!("# Downloads: {}\n", dl));
        }
        if !crate_info.categories.is_empty() {
            toml.push_str(&format!("# Categories: {}\n", crate_info.categories.join(", ")));
        }
        if !crate_info.keywords.is_empty() {
            toml.push_str(&format!("# Keywords: {}\n", crate_info.keywords.join(", ")));
        }
        if let Some(rust_ver) = &crate_info.rust_version {
            toml.push_str(&format!("# Rust version: {}\n", rust_ver));
        }
        if let Some(edition) = &crate_info.edition {
            toml.push_str(&format!("# Edition: {}\n", edition));
        }
        toml.push_str(&format!("{} = \"{}\"\n\n", crate_info.name, crate_info.version));
    }
    
    format_toml(&toml)
}

fn generate_dependency_graph_toml(crates: &[CrateInfo]) -> String {
    let mut toml = String::new();
    
    toml.push_str("# Dependency Graph - Generated by Cargo Packer\n");
    toml.push_str(&format!("# Generated: {}\n\n", chrono::Utc::now().to_rfc3339()));
    
    toml.push_str("[workspace]\n");
    toml.push_str("members = [\n");
    for crate_info in crates {
        toml.push_str(&format!("    \"{}\",\n", crate_info.name));
    }
    toml.push_str("]\n\n");
    
    toml.push_str("[workspace.dependencies]\n");
    for crate_info in crates {
        let mut dep_spec = format!("\"{}\"", crate_info.version);
        if let Some(rust_ver) = &crate_info.rust_version {
            dep_spec.push_str(&format!(", rust-version = \"{}\"", rust_ver));
        }
        toml.push_str(&format!("{} = {}\n", crate_info.name, dep_spec));
    }
    toml.push_str("\n");
    
    // Add dependency graph edges as comments
    for crate_info in crates {
        if let Some(graph) = &crate_info.dependency_graph {
            if !graph.edges.is_empty() {
                toml.push_str(&format!("# {} dependencies:\n", crate_info.name));
                for edge in &graph.edges {
                    if edge.from != "root" {
                        toml.push_str(&format!("#   {} -> {}\n", edge.from, edge.to));
                    }
                }
                toml.push_str("\n");
            }
        }
    }
    
    format_toml(&toml)
}

fn generate_dependency_graph_json(crates: &[CrateInfo]) -> String {
    use serde_json::json;
    
    let mut all_nodes = Vec::new();
    let mut all_edges = Vec::new();
    
    for crate_info in crates {
        if let Some(graph) = &crate_info.dependency_graph {
            for node in &graph.nodes {
                if node.id != "root" {
                    all_nodes.push(json!({
                        "id": node.id,
                        "name": node.name,
                        "version": node.version,
                        "kind": node.kind,
                        "optional": node.optional,
                        "crate": crate_info.name,
                    }));
                }
            }
            for edge in &graph.edges {
                if edge.from != "root" {
                    all_edges.push(json!({
                        "from": edge.from,
                        "to": edge.to,
                    }));
                }
            }
        }
    }
    
    json!({
        "metadata": {
            "generated": chrono::Utc::now().to_rfc3339(),
            "crates_count": crates.len(),
            "source": "Cargo Packer"
        },
        "nodes": all_nodes,
        "edges": all_edges,
    }).to_string()
}

fn generate_dependency_graph_dot(crates: &[CrateInfo]) -> String {
    let mut dot = String::new();
    
    dot.push_str("digraph dependencies {\n");
    dot.push_str("  rankdir=LR;\n");
    dot.push_str("  node [shape=box, style=filled, fillcolor=lightblue];\n\n");
    
    for crate_info in crates {
        if let Some(graph) = &crate_info.dependency_graph {
            for node in &graph.nodes {
                if node.id != "root" {
                    let label = format!("{}@{}", node.name, node.version);
                    let color = if node.optional { "lightyellow" } else { "lightblue" };
                    dot.push_str(&format!("  \"{}\" [label=\"{}\", fillcolor=\"{}\"];\n", node.id, label, color));
                }
            }
            for edge in &graph.edges {
                if edge.from != "root" {
                    dot.push_str(&format!("  \"{}\" -> \"{}\";\n", edge.from, edge.to));
                }
            }
        }
    }
    
    dot.push_str("}\n");
    dot
}

fn format_toml(toml: &str) -> String {
    let mut in_deps = false;
    let mut deps_lines: Vec<String> = Vec::new();
    let mut other_lines = Vec::new();
    
    for line in toml.lines() {
        if line.trim().starts_with("[dependencies]") {
            in_deps = true;
            other_lines.push(line.to_string());
        } else if in_deps && line.trim().starts_with('[') {
            in_deps = false;
            deps_lines.sort();
            for dep in &deps_lines {
                other_lines.push(dep.clone());
            }
            other_lines.push(line.to_string());
        } else if in_deps && !line.trim().is_empty() && !line.trim().starts_with('#') {
            deps_lines.push(line.to_string());
        } else {
            if in_deps {
                deps_lines.sort();
                for dep in &deps_lines {
                    other_lines.push(dep.clone());
                }
                deps_lines.clear();
                in_deps = false;
            }
            other_lines.push(line.to_string());
        }
    }
    
    if in_deps {
        deps_lines.sort();
        for dep in &deps_lines {
            other_lines.push(dep.clone());
        }
    }
    
    other_lines.join("\n")
}

fn generate_json(crates: &[CrateInfo]) -> String {
    serde_json::json!({
        "meta": {
            "generated": chrono::Utc::now().to_rfc3339(),
            "count": crates.len(),
            "source": "Cargo Packer Extension"
        },
        "packages": crates
    }).to_string()
}

fn generate_text(crates: &[CrateInfo]) -> String {
    let mut text = format!("Cargo Packer Export\n");
    text.push_str(&format!("Generated: {}\n", chrono::Utc::now().to_rfc3339()));
    text.push_str(&format!("Total: {} packages\n\n", crates.len()));

    for (index, crate_info) in crates.iter().enumerate() {
        text.push_str(&format!("{}. {} @ {}\n", index + 1, crate_info.name, crate_info.version));
        text.push_str(&format!("   Description: {}\n", crate_info.description));
        text.push_str(&format!("   Dependencies: {}\n", crate_info.dependencies));
        text.push_str(&format!("   Updated: {}\n", crate_info.updated));
        text.push_str(&format!("   URL: {}\n", crate_info.url));
        text.push_str(&format!("   Saved: {}\n\n", crate_info.timestamp));
    }
    text
}

async fn download_file(content: &str, filename: &str) {
    let blob_parts = Array::new();
    blob_parts.push(&JsValue::from_str(content));
    
    let blob_props = BlobPropertyBag::new();
    blob_props.set_type("text/plain");
    
    let blob = Blob::new_with_u8_array_sequence_and_options(&blob_parts, &blob_props).unwrap();
    let url = Url::create_object_url_with_blob(&blob).unwrap();
    let url_clone = url.clone();
    
    let download_options = Object::new();
    Reflect::set(&download_options, &"url".into(), &url.into()).unwrap();
    Reflect::set(&download_options, &"filename".into(), &filename.into()).unwrap();
    Reflect::set(&download_options, &"saveAs".into(), &JsValue::from(false)).unwrap();
    Reflect::set(&download_options, &"conflictAction".into(), &"overwrite".into()).unwrap();
    
    let window = window().unwrap();
    let chrome = window.unchecked_ref::<js_sys::Object>();
    if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
        if let Ok(downloads) = Reflect::get(&chrome_obj, &"downloads".into()) {
            if let Ok(download_fn) = Reflect::get(&downloads, &"download".into()) {
                if let Some(download_fn) = download_fn.dyn_ref::<js_sys::Function>() {
                    let _ = download_fn.call1(&downloads, &download_options);
                }
            }
        }
    }
    
    let _ = Url::revoke_object_url(&url_clone);
}

#[oxichrome::background]
async fn background() {
    log!("Cargo Packer background service worker started");

    let result = storage::get::<SavedCratesData>(STORAGE_KEY).await;
    if let Ok(None) = result {
        let default_data = SavedCratesData {
            crates: vec![],
            settings: Settings::default(),
        };
        let _ = storage::set(STORAGE_KEY, &default_data).await;
        log!("Initialized default settings");
    }

    inject_content_scripts().await;

    let window = window().unwrap();
    let chrome = window.unchecked_ref::<js_sys::Object>();
    if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
        if let Ok(tabs) = Reflect::get(&chrome_obj, &"tabs".into()) {
            if let Ok(on_updated) = Reflect::get(&tabs, &"onUpdated".into()) {
                if let Ok(add_listener) = Reflect::get(&on_updated, &"addListener".into()) {
                    if let Some(add_listener_fn) = add_listener.dyn_ref::<js_sys::Function>() {
                        let handler = wasm_bindgen::prelude::Closure::wrap(Box::new(move |tab_id: i32, _change_info: JsValue, tab: JsValue| {
                            spawn_local(async move {
                                if let Some(url_val) = Reflect::get(&tab, &"url".into()).ok().and_then(|v| v.as_string()) {
                                    if url_val.contains("crates.io") && url_val.contains("/crates/") {
                                        inject_content_script_into_tab(tab_id).await;
                                    }
                                }
                            });
                        }) as Box<dyn FnMut(i32, JsValue, JsValue)>);
                        let _ = add_listener_fn.call1(&on_updated, &handler.as_ref().unchecked_ref());
                        handler.forget();
                    }
                }
            }
        }
    }

    if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
        if let Ok(runtime) = Reflect::get(&chrome_obj, &"runtime".into()) {
            if let Ok(on_message) = Reflect::get(&runtime, &"onMessage".into()) {
                if let Ok(add_listener) = Reflect::get(&on_message, &"addListener".into()) {
                    if let Some(add_listener_fn) = add_listener.dyn_ref::<js_sys::Function>() {
                        let handler = wasm_bindgen::prelude::Closure::wrap(Box::new(move |message: JsValue, _sender: JsValue, send_response: JsValue| {
                            spawn_local(async move {
                                let msg: serde_json::Value = match serde_json::from_str(&message.as_string().unwrap_or_default()) {
                                    Ok(v) => v,
                                    Err(_) => return,
                                };

                                let send_resp = |resp: serde_json::Value| {
                                    let _ = send_response.dyn_ref::<js_sys::Function>().map(|f| {
                                        f.call1(&JsValue::NULL, &JsValue::from_str(&resp.to_string()))
                                    });
                                };

                                match msg.get("action").and_then(|v| v.as_str()) {
Some("saveCrate") => {
    if let Some(crate_info) = msg.get("crate").and_then(|v| {
        serde_wasm_bindgen::to_value(v).ok()
            .and_then(|jv| serde_wasm_bindgen::from_value::<CrateInfo>(jv).ok())
    }) {
        let response = save_crate_to_storage(crate_info).await;
        send_resp(response);
    }
}
Some("getSavedCrates") => {
    match storage::get::<SavedCratesData>(STORAGE_KEY).await {
        Ok(Some(data)) => send_resp(serde_json::json!({ "success": true, "crates": data.crates })),
        Ok(None) => send_resp(serde_json::json!({ "success": true, "crates": [] })),
        Err(e) => send_resp(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}
Some("exportCrates") => {
    let format = msg.get("format").and_then(|v| v.as_str()).unwrap_or("toml");
    let export_type = msg.get("exportType").and_then(|v| v.as_str()).unwrap_or("crates");
    match storage::get::<SavedCratesData>(STORAGE_KEY).await {
        Ok(Some(data)) => {
            let (content, filename) = match (export_type, format) {
                ("dependency-graph", "toml") => (generate_dependency_graph_toml(&data.crates), "dependency-graph.toml".to_string()),
                ("dependency-graph", "json") => (generate_dependency_graph_json(&data.crates), "dependency-graph.json".to_string()),
                ("dependency-graph", "dot") => (generate_dependency_graph_dot(&data.crates), "dependency-graph.dot".to_string()),
                ("crates", "toml") => (generate_toml(&data.crates, &data.settings.export_filename), data.settings.export_filename.clone()),
                ("crates", "json") => (generate_json(&data.crates), "crates.json".to_string()),
                ("crates", _) => (generate_text(&data.crates), "crates.txt".to_string()),
                _ => (generate_toml(&data.crates, &data.settings.export_filename), data.settings.export_filename.clone()),
            };
            download_file(&content, &filename).await;
            send_resp(serde_json::json!({ "success": true, "filename": filename, "count": data.crates.len(), "exportType": export_type }));
        }
        Ok(None) => send_resp(serde_json::json!({ "success": false, "error": "No crates to export" })),
        Err(e) => send_resp(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}
Some("clearCrates") => {
    match storage::remove(STORAGE_KEY).await {
        Ok(_) => send_resp(serde_json::json!({ "success": true })),
        Err(e) => send_resp(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}
Some("getSettings") => {
    match storage::get::<SavedCratesData>(STORAGE_KEY).await {
        Ok(Some(data)) => send_resp(serde_json::json!({ "success": true, "settings": data.settings })),
        Ok(None) => send_resp(serde_json::json!({ "success": true, "settings": Settings::default() })),
        Err(e) => send_resp(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}
Some("saveSettings") => {
    if let Some(settings) = msg.get("settings").and_then(|v| {
        serde_wasm_bindgen::to_value(v).ok()
            .and_then(|jv| serde_wasm_bindgen::from_value::<Settings>(jv).ok())
    }) {
        match storage::get::<SavedCratesData>(STORAGE_KEY).await {
            Ok(Some(mut data)) => {
                data.settings = settings;
                let _ = storage::set(STORAGE_KEY, &data).await;
                send_resp(serde_json::json!({ "success": true }));
            }
            Ok(None) => {
                let data = SavedCratesData {
                    crates: vec![],
                    settings,
                };
                let _ = storage::set(STORAGE_KEY, &data).await;
                send_resp(serde_json::json!({ "success": true }));
            }
            Err(e) => send_resp(serde_json::json!({ "success": false, "error": e.to_string() })),
        }
    }
}
                                    _ => {}
                                }
                            });
                        }) as Box<dyn FnMut(JsValue, JsValue, JsValue)>);
                        
                        let _ = add_listener_fn.call1(&on_message, &handler.as_ref().unchecked_ref());
                        handler.forget();
                    }
                }
            }
        }
    }
}

async fn inject_content_scripts() {
    let window = window().unwrap();
    let chrome = window.unchecked_ref::<js_sys::Object>();
    if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
        if let Ok(tabs) = Reflect::get(&chrome_obj, &"tabs".into()) {
            if let Ok(query) = Reflect::get(&tabs, &"query".into()) {
                if let Some(query_fn) = query.dyn_ref::<js_sys::Function>() {
                    let query_info = js_sys::Object::new();
                    js_sys::Reflect::set(&query_info, &"url".into(), &"*://crates.io/*".into()).unwrap();
                    
                    if let Ok(promise_val) = query_fn.call1(&tabs, &query_info) {
                        if let Some(promise) = promise_val.dyn_ref::<js_sys::Promise>() {
                            let _ = wasm_bindgen_futures::JsFuture::from(promise.clone()).await;
                        }
                    }
                }
            }
        }
    }
}

async fn inject_content_script_into_tab(tab_id: i32) {
    let window = window().unwrap();
    let chrome = window.unchecked_ref::<js_sys::Object>();
    if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
        if let Ok(scripting) = Reflect::get(&chrome_obj, &"scripting".into()) {
            if let Ok(execute_script) = Reflect::get(&scripting, &"executeScript".into()) {
                if let Some(execute_script_fn) = execute_script.dyn_ref::<js_sys::Function>() {
                    let injection = js_sys::Object::new();
                    let target = js_sys::Object::new();
                    js_sys::Reflect::set(&target, &"tabId".into(), &tab_id.into()).unwrap();
                    js_sys::Reflect::set(&injection, &"target".into(), &target).unwrap();
                    let closure = wasm_bindgen::prelude::Closure::once(|| {
                        init_content_script();
                    });
                    js_sys::Reflect::set(&injection, &"func".into(), closure.as_ref().unchecked_ref()).unwrap();
                    
                    let _ = execute_script_fn.call1(&scripting, &injection);
                    closure.forget();
                }
            }
        }
    }
}

async fn save_crate_to_storage(crate_info: CrateInfo) -> serde_json::Value {
    // Fetch enhanced metadata from crates.io API
    let enhanced_crate = match fetch_crate_metadata(&crate_info.name, Some(&crate_info.version)).await {
        Ok(mut enhanced) => {
            // Preserve the original timestamp and URL from content script
            enhanced.timestamp = crate_info.timestamp.clone();
            enhanced.url = crate_info.url.clone();
            enhanced
        }
        Err(e) => {
            log!("API fetch failed, using basic info: {}", e);
            crate_info
        }
    };
    
    match storage::get::<SavedCratesData>(STORAGE_KEY).await {
        Ok(Some(mut data)) => {
            let existing_index = data.crates.iter().position(|c| c.dedup_key() == enhanced_crate.dedup_key());
            
            if let Some(idx) = existing_index {
                data.crates[idx] = CrateInfo {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    ..enhanced_crate.clone()
                };
            } else {
                data.crates.push(CrateInfo {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    ..enhanced_crate.clone()
                });
            }

            let settings = data.settings.clone();
            
            if let Err(e) = storage::set(STORAGE_KEY, &data).await {
                return serde_json::json!({ "success": false, "error": e.to_string() });
            }

            if settings.auto_save {
                let format = &settings.save_format;
                let content = match format.as_str() {
                    "toml" => generate_toml(&data.crates, &settings.export_filename),
                    "json" => generate_json(&data.crates),
                    _ => generate_text(&data.crates),
                };
                let filename = settings.export_filename.clone();
                download_file(&content, &filename).await;
            }

            serde_json::json!({
                "success": true,
                "action": if existing_index.is_some() { "updated" } else { "added" },
                "count": data.crates.len(),
                "crate": enhanced_crate
            })
        }
        Ok(None) => {
            let data = SavedCratesData {
                crates: vec![CrateInfo {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    ..enhanced_crate.clone()
                }],
                settings: Settings::default(),
            };
            
            if let Err(e) = storage::set(STORAGE_KEY, &data).await {
                return serde_json::json!({ "success": false, "error": e.to_string() });
            }

            if data.settings.auto_save {
                let format = &data.settings.save_format;
                let content = match format.as_str() {
                    "toml" => generate_toml(&data.crates, &data.settings.export_filename),
                    "json" => generate_json(&data.crates),
                    _ => generate_text(&data.crates),
                };
                let filename = data.settings.export_filename.clone();
                download_file(&content, &filename).await;
            }

            serde_json::json!({
                "success": true,
                "action": "added",
                "count": 1,
                "crate": enhanced_crate
            })
        }
        Err(e) => {
            serde_json::json!({ "success": false, "error": e.to_string() })
        }
    }
}

#[wasm_bindgen]
pub fn init_content_script() {
    log!("Cargo Packer content script loaded");
    
    let window = web_sys::window().unwrap();
    let chrome = window.unchecked_ref::<js_sys::Object>();
    
    if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
        if let Ok(runtime) = Reflect::get(&chrome_obj, &"runtime".into()) {
            if let Ok(on_message) = Reflect::get(&runtime, &"onMessage".into()) {
                if let Ok(add_listener) = Reflect::get(&on_message, &"addListener".into()) {
                    if let Some(add_listener_fn) = add_listener.dyn_ref::<js_sys::Function>() {
                        let handler = wasm_bindgen::prelude::Closure::wrap(Box::new(move |message: JsValue, _sender: JsValue, send_response: JsValue| {
                            let msg: serde_json::Value = match serde_json::from_str(&message.as_string().unwrap_or_default()) {
                                Ok(v) => v,
                                Err(_) => return,
                            };
                            
                            let send_resp = |resp: serde_json::Value| {
                                let _ = send_response.dyn_ref::<js_sys::Function>().map(|f| {
                                    f.call1(&JsValue::NULL, &JsValue::from_str(&resp.to_string()))
                                });
                            };
                            
                            match msg.get("action").and_then(|v| v.as_str()) {
                                Some("getCurrentCrate") => {
                                    let crate_info = extract_crate_info();
                                    let response = if let Some(info) = crate_info {
                                        serde_json::json!({ "success": true, "crate": info })
                                    } else {
                                        serde_json::json!({ "success": false })
                                    };
                                    send_resp(response);
                                }
                                Some("saveCurrentCrate") => {
                                    let crate_info = extract_crate_info();
                                    let response = if let Some(info) = crate_info {
                                        serde_json::json!({ "success": true, "crate": info })
                                    } else {
                                        serde_json::json!({ "success": false })
                                    };
                                    send_resp(response);
                                }
                                Some("ping") => {
                                    send_resp(serde_json::json!({ "success": true, "message": "Content script alive" }));
                                }
                                _ => {}
                            }
                        }) as Box<dyn FnMut(JsValue, JsValue, JsValue)>);
                        
                        let _ = add_listener_fn.call1(&on_message, &handler.as_ref().unchecked_ref());
                        handler.forget();
                    }
                }
            }
        }
    }
    
    log!("Cargo Packer ready!");
}

fn extract_crate_info() -> Option<CrateInfo> {
    let window = window()?;
    let url = window.location().href().ok()?;
    
    // Extract crate name from URL
    let crate_name = url.split("crates.io/crates/").nth(1)?.split('/').next()?;
    if crate_name.is_empty() {
        return None;
    }
    
    // Smart version detection from URL
    let version_from_url = extract_version_from_url(&url);
    
    // For now, return basic info and let background fetch full metadata via API
    // This avoids CORS issues in content script
    let document = window.document()?;
    
    let version = version_from_url.unwrap_or_else(|| {
        // Extract version from DOM without using ? operator in closure
        let version_opt = document.query_selector(".version").ok().flatten()
            .and_then(|el| el.text_content())
            .or_else(|| document.query_selector("[data-testid=\"version\"]").ok().flatten().and_then(|el| el.text_content()))
            .or_else(|| document.query_selector(".details .version").ok().flatten().and_then(|el| el.text_content()));
        
        version_opt.unwrap_or_else(|| "unknown".to_string())
            .trim()
            .trim_start_matches('v')
            .to_string()
    });
    
    let description = document.query_selector(".description").ok().flatten()
        .and_then(|el| el.text_content())
        .or_else(|| document.query_selector("[data-testid=\"description\"]").ok().flatten().and_then(|el| el.text_content()))
        .or_else(|| document.query_selector(".crate-info p").ok().flatten().and_then(|el| el.text_content()))
        .unwrap_or_default()
        .trim()
        .to_string();
    
    let dependencies = document.query_selector("[href=\"#dependencies\"]").ok().flatten()
        .and_then(|el| el.text_content())
        .or_else(|| document.query_selector(".dependencies").ok().flatten().and_then(|el| el.text_content()))
        .and_then(|t| {
            Regex::new(r"(\d+)").ok().and_then(|re| {
                re.captures(&t).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
            })
        })
        .unwrap_or_else(|| "0".to_string());
    
    let updated = document.query_selector(".last-updated").ok().flatten()
        .and_then(|el| el.text_content())
        .or_else(|| document.query_selector("[title*=\"Updated\"]").ok().flatten().and_then(|el| el.get_attribute("title")))
        .and_then(|t| {
            Regex::new(r"\d{4}-\d{2}-\d{2}").ok().and_then(|re| {
                re.find(&t).map(|m| m.as_str().to_string())
            })
        })
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    
    Some(CrateInfo {
        name: crate_name.to_string(),
        version,
        description,
        dependencies,
        updated,
        url,
        timestamp: chrono::Utc::now().to_rfc3339(),
        repository: None,
        documentation: None,
        homepage: None,
        license: None,
        downloads: None,
        recent_downloads: None,
        categories: vec![],
        keywords: vec![],
        rust_version: None,
        edition: None,
        crate_size: None,
        dependency_graph: None,
    })
}

#[wasm_bindgen]
pub fn save_current_crate_from_content() {
    if let Some(crate_info) = extract_crate_info() {
        let window = web_sys::window().unwrap();
        let chrome = window.unchecked_ref::<js_sys::Object>();
        
        if let Ok(chrome_obj) = Reflect::get(&chrome, &"chrome".into()) {
            if let Ok(runtime) = Reflect::get(&chrome_obj, &"runtime".into()) {
                if let Ok(send_message) = Reflect::get(&runtime, &"sendMessage".into()) {
                    if let Some(send_message_fn) = send_message.dyn_ref::<js_sys::Function>() {
                        let msg = serde_json::json!({
                            "action": "saveCrate",
                            "crate": crate_info
                        });
                        let _ = send_message_fn.call1(&runtime, &JsValue::from_str(&msg.to_string()));
                    }
                }
            }
        }
    }
}