use crate::{ToolDefinition, ToolExecutionResult, ToolOutput};
use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use serde::Deserialize;
use serde_json::Value;
use std::ffi::{c_char, CStr, CString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalCommandHandler {
    program: String,
    args: Vec<String>,
}

impl ExternalCommandHandler {
    fn from_definition(definition: &ToolDefinition) -> Option<Self> {
        if definition.handler == "exec" {
            let (program, args) = definition.handler_args.split_first()?;
            return Some(Self {
                program: program.clone(),
                args: args.to_vec(),
            });
        }
        definition
            .handler
            .strip_prefix("exec:")
            .map(|program| Self {
                program: program.to_string(),
                args: definition.handler_args.clone(),
            })
    }

    fn execute(
        &self,
        definition: &ToolDefinition,
        cwd: &Path,
        input: Value,
    ) -> Result<ToolExecutionResult> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .env("PUFFER_TOOL_ID", &definition.id)
            .env("PUFFER_TOOL_NAME", &definition.name)
            .env("PUFFER_TOOL_HANDLER", &definition.handler)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn external tool handler {}", self.program))?;
        if let Some(stdin) = child.stdin.as_mut() {
            let payload = serde_json::to_vec(&input)?;
            stdin.write_all(&payload).with_context(|| {
                format!("failed to write external tool input for {}", definition.id)
            })?;
        }
        let output = child
            .wait_with_output()
            .with_context(|| format!("failed to wait for external tool {}", definition.id))?;
        let metadata = parse_output_metadata(&output.stdout);
        Ok(ToolExecutionResult {
            tool_id: definition.id.clone(),
            success: output.status.success(),
            output: ToolOutput {
                stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                metadata,
            },
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SharedLibraryHandler {
    path: String,
    symbol: String,
}

impl SharedLibraryHandler {
    fn execute(
        &self,
        definition: &ToolDefinition,
        cwd: &Path,
        input: Value,
    ) -> Result<ToolExecutionResult> {
        type SharedToolFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
        type SharedToolFreeFn = unsafe extern "C" fn(*mut c_char);

        let library_path = resolve_library_path(cwd, &self.path);
        let payload = CString::new(serde_json::to_string(&serde_json::json!({
            "cwd": cwd.display().to_string(),
            "tool": {
                "id": definition.id,
                "name": definition.name,
                "handler": definition.handler,
            },
            "input": input,
        }))?)?;
        // SAFETY: The shared library path and symbol name are user-provided configuration.
        // The supported ABI is explicit and narrow: the symbol accepts a NUL-terminated
        // UTF-8 JSON string and returns a heap-allocated UTF-8 JSON string that is freed
        // through the companion `puffer_tool_free_string` symbol in the same library.
        unsafe {
            let library = Library::new(&library_path).with_context(|| {
                format!(
                    "failed to load shared library tool handler {}",
                    library_path.display()
                )
            })?;
            let entry: Symbol<'_, SharedToolFn> = library
                .get(self.symbol.as_bytes())
                .with_context(|| format!("failed to load tool symbol {}", self.symbol))?;
            let free: Symbol<'_, SharedToolFreeFn> = library
                .get(b"puffer_tool_free_string")
                .context("failed to load puffer_tool_free_string")?;
            let raw = entry(payload.as_ptr());
            if raw.is_null() {
                return Err(anyhow!(
                    "shared library tool {} returned a null response",
                    definition.id
                ));
            }
            let output = CStr::from_ptr(raw).to_string_lossy().into_owned();
            free(raw);
            parse_shared_library_output(definition, output)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ProviderBuiltinHandler {
    WebSearch,
}

#[derive(Debug, Clone)]
pub(crate) enum ToolRuntime {
    Builtin(crate::ToolKind),
    ProviderBuiltin(ProviderBuiltinHandler),
    RuntimeLocal,
    External(ExternalCommandHandler),
    SharedLibrary(SharedLibraryHandler),
}

pub(crate) fn runtime_from_definition(definition: &ToolDefinition) -> Result<ToolRuntime> {
    if let Some(kind) = crate::builtin_tool_kind(&builtin_handler_name(&definition.handler)) {
        return Ok(ToolRuntime::Builtin(kind));
    }
    if definition.handler == "provider:web_search" {
        return Ok(ToolRuntime::ProviderBuiltin(
            ProviderBuiltinHandler::WebSearch,
        ));
    }
    if is_runtime_local_handler(&definition.handler) {
        return Ok(ToolRuntime::RuntimeLocal);
    }
    if let Some(handler) = ExternalCommandHandler::from_definition(definition) {
        return Ok(ToolRuntime::External(handler));
    }
    if let Some(path) = &definition.shared_lib {
        return Ok(ToolRuntime::SharedLibrary(SharedLibraryHandler {
            path: path.clone(),
            symbol: definition.handler.clone(),
        }));
    }
    Err(anyhow!("unsupported tool handler {}", definition.handler))
}

pub(crate) fn execute_runtime(
    runtime: &ToolRuntime,
    definition: &ToolDefinition,
    cwd: &Path,
    input: Value,
) -> Result<ToolExecutionResult> {
    match runtime {
        ToolRuntime::Builtin(kind) => {
            let typed = crate::parse_builtin_input(*kind, input)?;
            crate::execute_builtin_tool(&definition.id, *kind, cwd, typed)
        }
        ToolRuntime::ProviderBuiltin(ProviderBuiltinHandler::WebSearch) => Err(anyhow!(
            "tool {} is provider-backed and must be executed by the runtime",
            definition.id
        )),
        ToolRuntime::RuntimeLocal => Err(anyhow!(
            "tool {} is runtime-backed and must be executed by the conversation runtime",
            definition.id
        )),
        ToolRuntime::External(handler) => handler.execute(definition, cwd, input),
        ToolRuntime::SharedLibrary(handler) => handler.execute(definition, cwd, input),
    }
}

pub(crate) fn builtin_handler_name(handler: &str) -> String {
    handler
        .strip_prefix("builtin:")
        .unwrap_or(handler)
        .to_string()
}

fn is_runtime_local_handler(handler: &str) -> bool {
    matches!(
        handler,
        "runtime:agent"
            | "runtime:skill"
            | "runtime:tool_search"
            | "runtime:browser"
            | "runtime:glob"
            | "runtime:notebook_edit"
            | "runtime:sleep"
            | "runtime:list_mcp_resources"
            | "runtime:read_mcp_resource"
            | "runtime:mcp_call"
            | "runtime:project_memory"
    ) || handler.starts_with("runtime:claude_")
        || handler.starts_with("runtime:workflow:")
}

fn resolve_library_path(cwd: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

#[derive(Debug, Deserialize)]
struct SharedLibraryOutput {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    metadata: Value,
}

fn parse_shared_library_output(
    definition: &ToolDefinition,
    output: String,
) -> Result<ToolExecutionResult> {
    let parsed: SharedLibraryOutput = serde_json::from_str(&output).with_context(|| {
        format!(
            "shared library tool {} returned invalid JSON output",
            definition.id
        )
    })?;
    Ok(ToolExecutionResult {
        tool_id: definition.id.clone(),
        success: parsed.success,
        output: ToolOutput {
            stdout: parsed.stdout,
            stderr: parsed.stderr,
            metadata: parsed.metadata,
        },
    })
}

fn parse_output_metadata(stdout: &[u8]) -> Value {
    serde_json::from_slice::<Value>(stdout).unwrap_or_else(|_| Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints};

    #[test]
    fn runtime_from_definition_accepts_sleep_runtime_handler() {
        let definition = ToolDefinition {
            id: "Sleep".to_string(),
            name: "Sleep".to_string(),
            description: "Wait without keeping bash busy".to_string(),
            handler: "runtime:sleep".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        };
        let runtime = runtime_from_definition(&definition).expect("runtime");
        assert!(matches!(runtime, ToolRuntime::RuntimeLocal));
    }

    #[test]
    fn shared_library_runtime_executes_json_abi() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("tool.rs");
        let library = temp
            .path()
            .join(format!("libshared_tool.{}", shared_library_extension()));
        std::fs::write(&source, SHARED_TOOL_TEST_SOURCE).expect("write source");
        let status = Command::new("rustc")
            .arg("--crate-type")
            .arg("cdylib")
            .arg("--edition")
            .arg("2021")
            .arg(&source)
            .arg("-o")
            .arg(&library)
            .status()
            .expect("rustc");
        assert!(
            status.success(),
            "failed to compile shared library test helper"
        );

        let definition = ToolDefinition {
            id: "shared".to_string(),
            name: "shared".to_string(),
            description: "Shared".to_string(),
            handler: "puffer_tool_entrypoint".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: Some(library.display().to_string()),
            enabled_if: None,
            display: ToolDisplayHints::default(),
        };
        let runtime = runtime_from_definition(&definition).expect("runtime");
        let result = execute_runtime(
            &runtime,
            &definition,
            temp.path(),
            serde_json::json!({ "path": "demo.txt" }),
        )
        .expect("execution");
        assert!(result.success);
        assert!(result.output.stdout.contains("demo.txt"));
        assert_eq!(
            result.output.metadata["received"]["input"]["path"],
            Value::String("demo.txt".to_string())
        );
    }

    #[cfg(target_os = "linux")]
    fn shared_library_extension() -> &'static str {
        "so"
    }

    #[cfg(target_os = "macos")]
    fn shared_library_extension() -> &'static str {
        "dylib"
    }

    #[cfg(target_os = "windows")]
    fn shared_library_extension() -> &'static str {
        "dll"
    }

    const SHARED_TOOL_TEST_SOURCE: &str = r#"
use std::ffi::{c_char, CStr, CString};

#[no_mangle]
pub extern "C" fn puffer_tool_entrypoint(input: *const c_char) -> *mut c_char {
    let input = unsafe { CStr::from_ptr(input) }.to_string_lossy().into_owned();
    let escaped = input.replace('\\', "\\\\").replace('\"', "\\\"");
    let payload = format!(
        "{{\"success\":true,\"stdout\":\"{}\",\"stderr\":\"\",\"metadata\":{{\"received\":{}}}}}",
        escaped,
        input
    );
    CString::new(payload).expect("payload").into_raw()
}

#[no_mangle]
pub extern "C" fn puffer_tool_free_string(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(value);
    }
}
"#;
}
