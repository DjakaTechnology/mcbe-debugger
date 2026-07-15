use zed_extension_api::{
    self as zed,
    serde_json::{self, json, Value},
    DebugAdapterBinary, DebugConfig, DebugRequest, DebugScenario,
    DebugTaskDefinition, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, TcpArguments, TcpArgumentsTemplate, Worktree,
};

const SIDECAR_BINARY_NAME: &str = "mc-dap-server";
const DEFAULT_MC_PORT: u16 = 19144;
const DEFAULT_MC_HOST: &str = "127.0.0.1";

struct MinecraftDebugExtension {
    cached_sidecar_path: Option<String>,
}

impl zed::Extension for MinecraftDebugExtension {
    fn new() -> Self {
        Self {
            cached_sidecar_path: None,
        }
    }

    fn get_dap_binary(
        &mut self,
        _adapter_name: String,
        config: DebugTaskDefinition,
        _user_provided_debug_adapter_path: Option<String>,
        worktree: &Worktree,
    ) -> Result<DebugAdapterBinary, String> {
        let tcp_template = config.tcp_connection.unwrap_or(TcpArgumentsTemplate {
            host: None,
            port: None,
            timeout: None,
        });
        let TcpArguments {
            host,
            port,
            timeout,
        } = zed::resolve_tcp_template(tcp_template)?;

        let mut configuration: Value = serde_json::from_str(&config.config)
            .map_err(|e| format!("invalid JSON configuration: {e}"))?;
        if let Some(obj) = configuration.as_object_mut() {
            obj.entry("cwd")
                .or_insert_with(|| worktree.root_path().into());
            obj.entry("port").or_insert_with(|| DEFAULT_MC_PORT.into());
            obj.entry("host")
                .or_insert_with(|| DEFAULT_MC_HOST.into());
        }

        let sidecar_command = self.resolve_sidecar(worktree)?;

        Ok(DebugAdapterBinary {
            command: Some(sidecar_command),
            arguments: vec![format!("--dap-port={}", port)],
            connection: Some(TcpArguments {
                host,
                port,
                timeout,
            }),
            cwd: Some(worktree.root_path()),
            envs: vec![],
            request_args: StartDebuggingRequestArguments {
                request: Self::request_kind(&configuration)?,
                configuration: configuration.to_string(),
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        config: Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        Self::request_kind(&config)
    }

    fn dap_config_to_scenario(
        &mut self,
        config: DebugConfig,
    ) -> Result<DebugScenario, String> {
        match config.request {
            DebugRequest::Launch(launch) => {
                let mut env_map = serde_json::Map::new();
                for (k, v) in launch.envs {
                    env_map.insert(k, Value::String(v));
                }
                let obj = json!({
                    "program": launch.program,
                    "cwd": launch.cwd,
                    "args": launch.args,
                    "env": Value::Object(env_map),
                    "stopOnEntry": config.stop_on_entry.unwrap_or(false),
                    "host": DEFAULT_MC_HOST,
                    "port": DEFAULT_MC_PORT,
                });
                Ok(DebugScenario {
                    adapter: config.adapter,
                    label: config.label,
                    build: None,
                    config: obj.to_string(),
                    tcp_connection: None,
                })
            }
            DebugRequest::Attach(_) => Err("attach mode not yet implemented".into()),
        }
    }
}

impl MinecraftDebugExtension {
    fn resolve_sidecar(&mut self, worktree: &Worktree) -> Result<String, String> {
        if let Some(cached) = &self.cached_sidecar_path {
            return Ok(cached.clone());
        }
        if let Some(path) = worktree.which(SIDECAR_BINARY_NAME) {
            self.cached_sidecar_path = Some(path.clone());
            return Ok(path);
        }
        Err(format!(
            "{SIDECAR_BINARY_NAME} not found on PATH. Build it with `cargo build -p mc-dap-server --release` and ensure it is on PATH."
        ))
    }

    fn request_kind(
        config: &Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        config
            .get("request")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "launch" => Some(StartDebuggingRequestArgumentsRequest::Launch),
                "attach" => Some(StartDebuggingRequestArgumentsRequest::Attach),
                _ => None,
            })
            .ok_or_else(|| "missing or invalid `request` field".into())
    }
}

zed::register_extension!(MinecraftDebugExtension);
