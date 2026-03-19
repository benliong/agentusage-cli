use std::path::PathBuf;

use rquickjs::{prelude::Rest, Ctx, Function, Object, Value};
use serde_json::Value as JsonValue;

/// Inject the full `__openusage_ctx` host API object into the QuickJS context.
///
/// This implements the same contract as robinebers/openusage's plugin_engine/host_api.rs,
/// minus the Tauri-specific imports. The keychain uses the `keyring` crate directly.
pub fn inject<'js>(
    ctx: &Ctx<'js>,
    plugin_dir: &PathBuf,
    app_version: &str,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    // Build the top-level ctx object
    let ctx_obj = Object::new(ctx.clone())?;

    // ctx.nowIso — current time as ISO 8601 string
    {
        let now = chrono::Utc::now().to_rfc3339();
        ctx_obj.set("nowIso", now)?;
    }

    // ctx.app — app metadata
    {
        let app_obj = Object::new(ctx.clone())?;
        app_obj.set("version", app_version)?;
        app_obj.set("name", "agentusage")?;
        // Plugin data dir: {plugin_dir}/data
        let data_dir = plugin_dir.join("data");
        std::fs::create_dir_all(&data_dir).ok();
        app_obj.set("pluginDataDir", data_dir.to_string_lossy().to_string())?;
        ctx_obj.set("app", app_obj)?;
    }

    // ctx.host
    let host_obj = Object::new(ctx.clone())?;
    inject_http(ctx, &host_obj)?;
    inject_keychain(ctx, &host_obj)?;
    inject_fs(ctx, &host_obj, plugin_dir)?;
    inject_sqlite(ctx, &host_obj, plugin_dir)?;
    inject_env(ctx, &host_obj)?;
    inject_log(ctx, &host_obj)?;
    inject_ccusage(ctx, &host_obj)?;
    ctx_obj.set("host", host_obj)?;

    globals.set("__openusage_ctx", ctx_obj)?;
    Ok(())
}

/// Inject ctx.host.http
///
/// Exposes a low-level `_requestRaw(method, url, headers, body)` function.
/// The plugin JS runtime wraps this into the cleaner `request()` API via a patch script.
fn inject_http<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> rquickjs::Result<()> {
    let http_obj = Object::new(ctx.clone())?;

    let request_fn = Function::new(
        ctx.clone(),
        move |method: String, url: String, headers_json: String, body: Option<String>| -> String {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default();

            let headers: serde_json::Map<String, JsonValue> =
                serde_json::from_str(&headers_json).unwrap_or_default();

            let mut req = match method.to_uppercase().as_str() {
                "GET" => client.get(&url),
                "POST" => client.post(&url),
                "PUT" => client.put(&url),
                "DELETE" => client.delete(&url),
                "PATCH" => client.patch(&url),
                _ => client.get(&url),
            };

            for (key, val) in &headers {
                if let Some(v) = val.as_str() {
                    if let (Ok(name), Ok(value)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(v),
                    ) {
                        req = req.header(name, value);
                    }
                }
            }

            if let Some(b) = body {
                req = req.body(b);
            }

            match req.send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let resp_headers: serde_json::Map<String, JsonValue> = resp
                        .headers()
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.as_str().to_string(),
                                JsonValue::String(v.to_str().unwrap_or("").to_string()),
                            )
                        })
                        .collect();
                    let body = resp.text().unwrap_or_default();
                    serde_json::json!({
                        "ok": status >= 200 && status < 300,
                        "status": status,
                        "headers": resp_headers,
                        "body": body,
                    })
                    .to_string()
                }
                Err(e) => {
                    tracing::debug!("http request failed: {e}");
                    serde_json::json!({
                        "ok": false,
                        "status": 0,
                        "headers": {},
                        "body": "",
                        "error": e.to_string(),
                    })
                    .to_string()
                }
            }
        },
    )?;

    http_obj.set("_requestRaw", request_fn)?;
    host.set("http", http_obj)?;
    Ok(())
}

/// Inject ctx.host.keychain
///
/// readGenericPassword(service): on macOS tries `security find-generic-password -s <service> -w`
/// first (picks up native app credentials), then falls back to keyring ("agentusage", service).
/// writeGenericPassword(service, value): stores via keyring ("agentusage", service).
fn inject_keychain<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> rquickjs::Result<()> {
    let kc_obj = Object::new(ctx.clone())?;

    let read_fn = Function::new(ctx.clone(), move |service: String| -> Option<String> {
        // On macOS: try the system keychain by service name (how the native app stored it)
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("security")
                .args(["find-generic-password", "-s", &service, "-w"])
                .output()
                .ok()?;
            if out.status.success() {
                let password = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !password.is_empty() {
                    return Some(password);
                }
            }
        }
        // Fallback: credentials stored by `au configure`
        keyring::Entry::new("agentusage", &service)
            .ok()
            .and_then(|e| e.get_password().ok())
    })?;

    let write_fn = Function::new(ctx.clone(), move |service: String, value: String| -> bool {
        keyring::Entry::new("agentusage", &service)
            .ok()
            .and_then(|e| e.set_password(&value).ok())
            .is_some()
    })?;

    kc_obj.set("readGenericPassword", read_fn)?;
    kc_obj.set("writeGenericPassword", write_fn)?;
    host.set("keychain", kc_obj)?;
    Ok(())
}

/// Inject ctx.host.fs
///
/// Sandboxed to the plugin's data directory.
fn inject_fs<'js>(
    ctx: &Ctx<'js>,
    host: &Object<'js>,
    plugin_dir: &PathBuf,
) -> rquickjs::Result<()> {
    let fs_obj = Object::new(ctx.clone())?;
    let data_dir = plugin_dir.join("data");
    std::fs::create_dir_all(&data_dir).ok();

    // readFile(relativePath) -> string | null
    {
        let dir = data_dir.clone();
        let read_fn = Function::new(ctx.clone(), move |path: String| -> Option<String> {
            let full = dir.join(sanitize_path(&path)?);
            std::fs::read_to_string(full).ok()
        })?;
        fs_obj.set("readFile", read_fn)?;
    }

    // writeFile(relativePath, content) -> bool
    {
        let dir = data_dir.clone();
        let write_fn = Function::new(ctx.clone(), move |path: String, content: String| -> bool {
            let Some(rel) = sanitize_path(&path) else {
                return false;
            };
            let full = dir.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(full, content).is_ok()
        })?;
        fs_obj.set("writeFile", write_fn)?;
    }

    // deleteFile(relativePath) -> bool
    {
        let dir = data_dir.clone();
        let del_fn = Function::new(ctx.clone(), move |path: String| -> bool {
            let Some(rel) = sanitize_path(&path) else {
                return false;
            };
            std::fs::remove_file(dir.join(rel)).is_ok()
        })?;
        fs_obj.set("deleteFile", del_fn)?;
    }

    // exists(path) -> bool — supports ~/ and absolute paths
    {
        let dir = data_dir.clone();
        let exists_fn = Function::new(ctx.clone(), move |path: String| -> bool {
            resolve_path(&path, &dir)
                .map(|p| p.exists())
                .unwrap_or(false)
        })?;
        fs_obj.set("exists", exists_fn)?;
    }

    // readText(path) -> string | null — supports ~/ and absolute paths
    {
        let dir = data_dir.clone();
        let read_text_fn = Function::new(ctx.clone(), move |path: String| -> Option<String> {
            std::fs::read_to_string(resolve_path(&path, &dir)?).ok()
        })?;
        fs_obj.set("readText", read_text_fn)?;
    }

    // writeText(relativePath, content) -> bool — sandboxed (relative only)
    {
        let dir = data_dir.clone();
        let write_text_fn =
            Function::new(ctx.clone(), move |path: String, content: String| -> bool {
                let Some(rel) = sanitize_path(&path) else {
                    return false;
                };
                let full = dir.join(rel);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(full, content).is_ok()
            })?;
        fs_obj.set("writeText", write_text_fn)?;
    }

    // ls(path) — runs platform ls/dir, matches robinebers/openusage behavior
    {
        let dir = data_dir.clone();
        let ls_fn = Function::new(ctx.clone(), move |path: Option<String>| -> String {
            let target = if let Some(p) = path {
                if p.starts_with('/') || (cfg!(windows) && p.contains(':')) {
                    PathBuf::from(p)
                } else {
                    dir.join(p)
                }
            } else {
                dir.clone()
            };
            #[cfg(windows)]
            let output = std::process::Command::new("cmd")
                .args(["/C", "dir", &target.to_string_lossy()])
                .output();
            #[cfg(not(windows))]
            let output = std::process::Command::new("ls")
                .arg("-la")
                .arg(&target)
                .output();
            match output {
                Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
                Err(_) => String::new(),
            }
        })?;
        fs_obj.set("ls", ls_fn)?;
    }

    host.set("fs", fs_obj)?;
    Ok(())
}

/// Resolve a path for readText/exists: expands ~/, allows absolute /, or sandboxes to data_dir.
fn resolve_path(path: &str, data_dir: &PathBuf) -> Option<PathBuf> {
    if let Some(rel) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(rel))
    } else if path.starts_with('/') {
        Some(PathBuf::from(path))
    } else {
        let rel = sanitize_path(path)?;
        Some(data_dir.join(rel))
    }
}

/// Prevent path traversal: only allow relative paths without `..` components.
fn sanitize_path(path: &str) -> Option<String> {
    let p = PathBuf::from(path);
    for component in p.components() {
        match component {
            std::path::Component::ParentDir => return None,
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
            _ => {}
        }
    }
    Some(path.to_string())
}

/// Inject ctx.host.sqlite
///
/// Plugins call `sqlite.query(dbPath, sql, params?)` where `dbPath` is "" for the plugin's own
/// db, "~/.../foo.db" for a home-relative path, or an absolute path for external databases
/// (e.g. Cursor/Windsurf state.vscdb).
fn inject_sqlite<'js>(
    ctx: &Ctx<'js>,
    host: &Object<'js>,
    plugin_dir: &PathBuf,
) -> rquickjs::Result<()> {
    let sqlite_obj = Object::new(ctx.clone())?;
    let default_db_path = plugin_dir.join("data").join("plugin.db");

    // execute(dbPath, sql, params?) -> { changes: number }
    {
        let default_db = default_db_path.clone();
        let exec_fn = Function::new(
            ctx.clone(),
            move |db_path_str: String, sql: String, params_json: Option<String>| -> String {
                let path = resolve_db_path(&db_path_str, &default_db);
                let result = sqlite_execute(&path, &sql, params_json.as_deref());
                result.unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string())
            },
        )?;
        sqlite_obj.set("execute", exec_fn)?;
    }

    // query(dbPath, sql, params?) -> row[]
    {
        let default_db = default_db_path.clone();
        let query_fn = Function::new(
            ctx.clone(),
            move |db_path_str: String, sql: String, params_json: Option<String>| -> String {
                let path = resolve_db_path(&db_path_str, &default_db);
                let result = sqlite_query(&path, &sql, params_json.as_deref());
                result.unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string())
            },
        )?;
        sqlite_obj.set("query", query_fn)?;
    }

    host.set("sqlite", sqlite_obj)?;
    Ok(())
}

fn resolve_db_path(db_path_str: &str, default_db: &PathBuf) -> PathBuf {
    if db_path_str.is_empty() {
        default_db.clone()
    } else if let Some(rel) = db_path_str.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(rel)
    } else {
        PathBuf::from(db_path_str)
    }
}

/// A rusqlite-compatible parameter type derived from a JSON value.
enum SqlParam {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl rusqlite::ToSql for SqlParam {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value};
        Ok(match self {
            SqlParam::Null => ToSqlOutput::Owned(Value::Null),
            SqlParam::Int(n) => ToSqlOutput::Owned(Value::Integer(*n)),
            SqlParam::Real(f) => ToSqlOutput::Owned(Value::Real(*f)),
            SqlParam::Text(s) => ToSqlOutput::Owned(Value::Text(s.clone())),
            SqlParam::Blob(b) => ToSqlOutput::Owned(Value::Blob(b.clone())),
        })
    }
}

fn json_to_sql_param(v: &JsonValue) -> SqlParam {
    match v {
        JsonValue::Null => SqlParam::Null,
        JsonValue::Bool(b) => SqlParam::Int(if *b { 1 } else { 0 }),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlParam::Int(i)
            } else if let Some(f) = n.as_f64() {
                SqlParam::Real(f)
            } else {
                SqlParam::Text(n.to_string())
            }
        }
        JsonValue::String(s) => SqlParam::Text(s.clone()),
        other => SqlParam::Text(other.to_string()),
    }
}

fn sqlite_execute(
    db_path: &PathBuf,
    sql: &str,
    params_json: Option<&str>,
) -> anyhow::Result<String> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path)?;
    let params = parse_params(params_json)?;
    let sql_params: Vec<SqlParam> = params.iter().map(json_to_sql_param).collect();
    let changes = conn.execute(sql, rusqlite::params_from_iter(sql_params.iter()))?;
    Ok(serde_json::json!({"changes": changes}).to_string())
}

fn sqlite_query(db_path: &PathBuf, sql: &str, params_json: Option<&str>) -> anyhow::Result<String> {
    use rusqlite::{types::ValueRef, Connection};
    let conn = Connection::open(db_path)?;
    let params = parse_params(params_json)?;
    let sql_params: Vec<SqlParam> = params.iter().map(json_to_sql_param).collect();
    let mut stmt = conn.prepare(sql)?;
    let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let rows: Vec<JsonValue> = stmt
        .query_map(rusqlite::params_from_iter(sql_params.iter()), |row| {
            let mut obj = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                let val = match row.get_ref(i)? {
                    ValueRef::Null => JsonValue::Null,
                    ValueRef::Integer(n) => JsonValue::Number(n.into()),
                    ValueRef::Real(f) => serde_json::Number::from_f64(f)
                        .map(JsonValue::Number)
                        .unwrap_or(JsonValue::Null),
                    ValueRef::Text(s) => JsonValue::String(String::from_utf8_lossy(s).into_owned()),
                    ValueRef::Blob(b) => JsonValue::String(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        b,
                    )),
                };
                obj.insert(name.clone(), val);
            }
            Ok(JsonValue::Object(obj))
        })?
        .flatten()
        .collect();
    Ok(serde_json::to_string(&rows)?)
}

fn parse_params(params_json: Option<&str>) -> anyhow::Result<Vec<JsonValue>> {
    match params_json {
        None | Some("") => Ok(vec![]),
        Some(s) => {
            let v: Vec<JsonValue> = serde_json::from_str(s)?;
            Ok(v)
        }
    }
}

/// Inject ctx.host.env
///
/// Only whitelisted environment variables are accessible.
fn inject_env<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> rquickjs::Result<()> {
    let env_obj = Object::new(ctx.clone())?;

    const ALLOWED_VARS: &[&str] = &[
        "HOME",
        "USER",
        "USERNAME",
        "USERPROFILE",
        "PATH",
        "SHELL",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GROQ_API_KEY",
        "MINIMAX_API_KEY",
        "MINIMAX_CN_API_KEY",
        "MINIMAX_CN_SECRET",
        "CURSOR_API_KEY",
    ];

    let get_fn = Function::new(ctx.clone(), move |name: String| -> Option<String> {
        if ALLOWED_VARS.contains(&name.as_str()) {
            std::env::var(&name).ok()
        } else {
            tracing::debug!("plugin requested non-whitelisted env var: {name}");
            None
        }
    })?;

    env_obj.set("get", get_fn)?;
    host.set("env", env_obj)?;
    Ok(())
}

/// Inject ctx.host.log
fn inject_log<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> rquickjs::Result<()> {
    let log_obj = Object::new(ctx.clone())?;

    macro_rules! log_fn {
        ($level:ident) => {{
            Function::new(ctx.clone(), move |args: Rest<String>| {
                let msg = args.join(" ");
                tracing::$level!(target: "plugin", "{}", msg);
            })?
        }};
    }

    log_obj.set("debug", log_fn!(debug))?;
    log_obj.set("info", log_fn!(info))?;
    log_obj.set("warn", log_fn!(warn))?;
    log_obj.set("error", log_fn!(warn))?;
    host.set("log", log_obj)?;
    Ok(())
}

/// Inject ctx.host.ccusage
///
/// Calls the `ccusage` CLI tool as an external process for Claude Code usage data.
/// Degrades gracefully when ccusage is not installed.
fn inject_ccusage<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> rquickjs::Result<()> {
    let ccusage_obj = Object::new(ctx.clone())?;

    let query_fn = Function::new(
        ctx.clone(),
        move |opts_json: Option<String>| -> Option<String> {
            let mut cmd = std::process::Command::new("ccusage");
            cmd.arg("--json");
            if let Some(opts) = opts_json {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&opts) {
                    if let Some(since) = v.get("since").and_then(|v| v.as_str()) {
                        cmd.args(["--since", since]);
                    }
                    if let Some(until) = v.get("until").and_then(|v| v.as_str()) {
                        cmd.args(["--until", until]);
                    }
                }
            }
            match cmd.output() {
                Ok(output) if output.status.success() => String::from_utf8(output.stdout).ok(),
                _ => None,
            }
        },
    )?;

    ccusage_obj.set("query", query_fn)?;
    host.set("ccusage", ccusage_obj)?;
    Ok(())
}

/// JS patch script injected after the plugin JS is evaluated.
/// Wraps `ctx.host.http._requestRaw` into the cleaner `ctx.host.http.request()` API
/// that plugins actually call.
pub const HTTP_PATCH_SCRIPT: &str = r#"
(function() {
    var ctx = globalThis.__openusage_ctx;
    if (!ctx || !ctx.host || !ctx.host.http) return;
    var raw = ctx.host.http._requestRaw;
    ctx.host.http.request = function(method, url, options) {
        // Support both calling conventions:
        //   (method, url, options)  — from ctx.util.request
        //   ({method, url, ...})    — direct object-style calls (antigravity, windsurf)
        if (method && typeof method === 'object') {
            options = method;
            method = options.method;
            url = options.url;
        }
        options = options || {};
        var headersJson = JSON.stringify(options.headers || {});
        // Plugins use either `body` or `bodyText` for the outgoing body
        var body = options.body != null ? String(options.body)
                 : options.bodyText != null ? String(options.bodyText)
                 : null;
        var resultJson = raw(method, url, headersJson, body);
        try {
            var result = JSON.parse(resultJson);
            // Expose both `body` and `bodyText` — plugins use either
            result.bodyText = result.body;
            result.json = function() {
                try { return JSON.parse(result.body); } catch(e) { return null; }
            };
            return result;
        } catch(e) {
            return { ok: false, status: 0, headers: {}, body: '', bodyText: '', json: function(){ return null; } };
        }
    };
})();
"#;

/// JS patch injected to expose ctx.host.sqlite.* with a nicer API.
/// Plugins call query(dbPath, sql, params?) — dbPath="" uses the plugin's own db.
/// JS patch for sqlite — forwards the 3-arg (dbPath, sql, params) signature.
/// Returns raw JSON strings so plugins can call ctx.util.tryParseJson() on the result.
pub const SQLITE_PATCH_SCRIPT: &str = r#"
(function() {
    var ctx = globalThis.__openusage_ctx;
    if (!ctx || !ctx.host || !ctx.host.sqlite) return;
    var rawExec = ctx.host.sqlite.execute;
    var rawQuery = ctx.host.sqlite.query;
    // query returns a raw JSON string (row array); plugins parse it with tryParseJson
    ctx.host.sqlite.query = function(dbPath, sql, params) {
        return rawQuery(dbPath || '', sql, params ? JSON.stringify(params) : null);
    };
    // execute returns a raw JSON string ({ changes: n } or { error: ... })
    ctx.host.sqlite.execute = function(dbPath, sql, params) {
        return rawExec(dbPath || '', sql, params ? JSON.stringify(params) : null);
    };
    ctx.host.sqlite.exec = ctx.host.sqlite.execute;
})();
"#;

/// Pure JS utilities injected as ctx.line.*, ctx.fmt.*, ctx.util.*, ctx.base64, ctx.jwt
/// These match the helpers in bundled_plugins/test-helpers.js (the authoritative reference).
pub const UTILS_SCRIPT: &str = r#"
(function() {
    var ctx = globalThis.__openusage_ctx;
    if (!ctx) return;

    // ctx.line helpers — object-based API
    ctx.line = {
        text: function(opts) {
            var line = { type: 'text', label: opts.label, value: opts.value };
            if (opts.color) line.color = opts.color;
            if (opts.subtitle) line.subtitle = opts.subtitle;
            return line;
        },
        progress: function(opts) {
            var line = { type: 'progress', label: opts.label, used: opts.used, limit: opts.limit, format: opts.format };
            if (opts.resetsAt) line.resetsAt = opts.resetsAt;
            if (opts.periodDurationMs) line.periodDurationMs = opts.periodDurationMs;
            if (opts.color) line.color = opts.color;
            return line;
        },
        badge: function(opts) {
            var line = { type: 'badge', label: opts.label, text: opts.text };
            if (opts.color) line.color = opts.color;
            if (opts.subtitle) line.subtitle = opts.subtitle;
            return line;
        },
    };

    // ctx.fmt helpers
    ctx.fmt = {
        planLabel: function(value) {
            var text = String(value || '').trim();
            if (!text) return '';
            return text.replace(/(^|\s)([a-z])/g, function(match, space, letter) { return space + letter.toUpperCase(); });
        },
        resetIn: function(secondsUntil) {
            if (!isFinite(secondsUntil) || secondsUntil < 0) return null;
            var totalMinutes = Math.floor(secondsUntil / 60);
            var totalHours = Math.floor(totalMinutes / 60);
            var days = Math.floor(totalHours / 24);
            var hours = totalHours % 24;
            var minutes = totalMinutes % 60;
            if (days > 0) return days + 'd ' + hours + 'h';
            if (totalHours > 0) return totalHours + 'h ' + minutes + 'm';
            if (totalMinutes > 0) return totalMinutes + 'm';
            return '<1m';
        },
        dollars: function(cents) { return Math.round((cents / 100) * 100) / 100; },
        date: function(unixMs) {
            var d = new Date(Number(unixMs));
            var months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
            return months[d.getMonth()] + ' ' + String(d.getDate());
        },
        percent: function() { return { kind: 'percent' }; },
        count: function(suffix) { return { kind: 'count', suffix: suffix || '' }; },
    };

    // ctx.base64 — pure-JS, handles URL-safe chars and padding
    var b64chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    ctx.base64 = {
        decode: function(str) {
            str = str.replace(/-/g, '+').replace(/_/g, '/');
            while (str.length % 4) str += '=';
            str = str.replace(/=+$/, '');
            var result = '';
            var len = str.length;
            var i = 0;
            while (i < len) {
                var remaining = len - i;
                var a = b64chars.indexOf(str.charAt(i++));
                var b = b64chars.indexOf(str.charAt(i++));
                var c = remaining > 2 ? b64chars.indexOf(str.charAt(i++)) : 0;
                var d = remaining > 3 ? b64chars.indexOf(str.charAt(i++)) : 0;
                var n = (a << 18) | (b << 12) | (c << 6) | d;
                result += String.fromCharCode((n >> 16) & 0xff);
                if (remaining > 2) result += String.fromCharCode((n >> 8) & 0xff);
                if (remaining > 3) result += String.fromCharCode(n & 0xff);
            }
            return result;
        },
        encode: function(str) {
            var result = '';
            var len = str.length;
            var i = 0;
            while (i < len) {
                var chunkStart = i;
                var a = str.charCodeAt(i++);
                var b = i < len ? str.charCodeAt(i++) : 0;
                var c = i < len ? str.charCodeAt(i++) : 0;
                var bytesInChunk = i - chunkStart;
                var n = (a << 16) | (b << 8) | c;
                result += b64chars.charAt((n >> 18) & 63);
                result += b64chars.charAt((n >> 12) & 63);
                result += bytesInChunk < 2 ? '=' : b64chars.charAt((n >> 6) & 63);
                result += bytesInChunk < 3 ? '=' : b64chars.charAt(n & 63);
            }
            return result;
        },
    };

    // ctx.jwt
    ctx.jwt = {
        decodePayload: function(token) {
            try {
                var parts = token.split('.');
                if (parts.length !== 3) return null;
                var decoded = ctx.base64.decode(parts[1]);
                return JSON.parse(decoded);
            } catch(e) { return null; }
        },
    };

    // ctx.util helpers
    ctx.util = {
        parseDate: function(s) { return new Date(s).toISOString(); },
        addDays: function(iso, n) {
            var d = new Date(iso); d.setDate(d.getDate() + n); return d.toISOString();
        },
        tryParseJson: function(text) {
            if (text === null || text === undefined) return null;
            var trimmed = String(text).trim();
            if (!trimmed) return null;
            try { return JSON.parse(trimmed); } catch(e) { return null; }
        },
        safeJsonParse: function(text) {
            if (text === null || text === undefined) return { ok: false };
            var trimmed = String(text).trim();
            if (!trimmed) return { ok: false };
            try { return { ok: true, value: JSON.parse(trimmed) }; } catch(e) { return { ok: false }; }
        },
        request: function(opts) { return ctx.host.http.request(opts.method, opts.url, opts); },
        requestJson: function(opts) {
            var resp = ctx.util.request(opts);
            var parsed = ctx.util.safeJsonParse(resp.body);
            return { resp: resp, json: parsed.ok ? parsed.value : null };
        },
        isAuthStatus: function(status) { return status === 401 || status === 403; },
        retryOnceOnAuth: function(opts) {
            var resp = opts.request();
            if (ctx.util.isAuthStatus(resp.status)) {
                var token = opts.refresh();
                if (token) resp = opts.request(token);
            }
            return resp;
        },
        parseDateMs: function(value) {
            if (value instanceof Date) {
                var dm = value.getTime();
                return isFinite(dm) ? dm : null;
            }
            if (typeof value === 'number') return isFinite(value) ? value : null;
            if (typeof value === 'string') {
                var parsed = Date.parse(value);
                if (isFinite(parsed)) return parsed;
                var n = Number(value);
                return isFinite(n) ? n : null;
            }
            return null;
        },
        toIso: function(value) {
            if (value === null || value === undefined) return null;
            if (typeof value === 'string') {
                var s = String(value).trim();
                if (!s) return null;
                if (s.indexOf(' ') !== -1 && /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/.test(s)) {
                    s = s.replace(' ', 'T');
                }
                if (s.endsWith(' UTC')) s = s.slice(0, -4) + 'Z';
                if (/^-?\d+(\.\d+)?$/.test(s)) {
                    var n = Number(s);
                    if (!isFinite(n)) return null;
                    var msNum = Math.abs(n) < 1e10 ? n * 1000 : n;
                    var dn = new Date(msNum);
                    if (!isFinite(dn.getTime())) return null;
                    return dn.toISOString();
                }
                if (/[+-]\d{4}$/.test(s)) s = s.replace(/([+-]\d{2})(\d{2})$/, '$1:$2');
                var m = s.match(/^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(\.\d+)?(Z|[+-]\d{2}:\d{2})$/);
                if (m) {
                    var head = m[1], frac = m[2] || '', tz = m[3];
                    if (frac) {
                        var digits = frac.slice(1);
                        if (digits.length > 3) digits = digits.slice(0, 3);
                        while (digits.length < 3) digits += '0';
                        frac = '.' + digits;
                    }
                    s = head + frac + tz;
                } else {
                    var mNoTz = s.match(/^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(\.\d+)?$/);
                    if (mNoTz) {
                        var head2 = mNoTz[1], frac2 = mNoTz[2] || '';
                        if (frac2) {
                            var digits2 = frac2.slice(1);
                            if (digits2.length > 3) digits2 = digits2.slice(0, 3);
                            while (digits2.length < 3) digits2 += '0';
                            frac2 = '.' + digits2;
                        }
                        s = head2 + frac2 + 'Z';
                    }
                }
                var p = Date.parse(s);
                if (!isFinite(p)) return null;
                return new Date(p).toISOString();
            }
            if (typeof value === 'number') {
                if (!isFinite(value)) return null;
                var ms = Math.abs(value) < 1e10 ? value * 1000 : value;
                var d = new Date(ms);
                if (!isFinite(d.getTime())) return null;
                return d.toISOString();
            }
            if (value instanceof Date) {
                var t = value.getTime();
                if (!isFinite(t)) return null;
                return value.toISOString();
            }
            return null;
        },
        needsRefreshByExpiry: function(opts) {
            if (!opts) return true;
            if (opts.expiresAtMs === null || opts.expiresAtMs === undefined) return true;
            var nowMs = Number(opts.nowMs);
            var expiresAtMs = Number(opts.expiresAtMs);
            var bufferMs = Number(opts.bufferMs);
            if (!isFinite(nowMs)) return true;
            if (!isFinite(expiresAtMs)) return true;
            if (!isFinite(bufferMs)) bufferMs = 0;
            return nowMs + bufferMs >= expiresAtMs;
        },
    };

    // Wrap ccusage.query: Rust expects a JSON string arg and returns a JSON string.
    // Plugins pass an options object and expect a parsed object back.
    if (ctx.host && ctx.host.ccusage && ctx.host.ccusage.query) {
        var _rawCcusage = ctx.host.ccusage.query;
        ctx.host.ccusage.query = function(opts) {
            var json = _rawCcusage(opts ? JSON.stringify(opts) : null);
            if (!json) return null;
            try { return JSON.parse(json); } catch(e) { return null; }
        };
    }

    // ctx.host.ls — stub for local service discovery (not available in CLI mode)
    if (ctx.host && !ctx.host.ls) {
        ctx.host.ls = { discover: function() { return null; } };
    }
})();
"#;
