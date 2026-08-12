use super::error::PidagError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Built-in fallback model chains. This is the ONLY place hardcoded model
/// strings live in the crate (per spec R1 / exit criterion EC2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub free: Vec<String>,
    pub paid: Vec<String>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            // GLM 5.2 (NVIDIA) as the primary free planner model; llama-3.3-70b
            // as the paid fallback for iteration 3+. Mirrors config.rs.md.
            free: vec!["nvidia/z-ai/glm-5.2".to_string()],
            paid: vec!["nvidia/meta/llama-3.3-70b-instruct".to_string()],
        }
    }
}

impl ModelsConfig {
    /// Build a `Vec<ModelRef>` for the given iteration. Iterations 1 and 2 use
    /// free models only; iteration 3 (and beyond) chains free then paid.
    pub fn models_for_iter(&self, iteration: usize) -> Vec<super::dag::ModelRef> {
        let free = self.free.iter().map(|m| super::dag::ModelRef {
            name: m.clone(),
            paid: false,
        });
        if iteration >= 3 {
            let paid = self.paid.iter().map(|m| super::dag::ModelRef {
                name: m.clone(),
                paid: true,
            });
            free.chain(paid).collect()
        } else {
            free.collect()
        }
    }
}

/// Configuration for agent backends (spec-21).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Backend name: "mock", "pi", etc. If not set, LLM nodes use default routing (A2aWorker/PiPrintWorker).
    #[serde(default)]
    pub backend: Option<String>,
}

/// Environment variable name for overriding the agent backend.
pub const ENV_AGENT_BACKEND: &str = "PIDAG_AGENT_BACKEND";

/// Configuration for RPC/MCP server (spec-22).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    /// TTL for completed runs in seconds before TTL sweep evicts them (default 60)
    #[serde(default = "default_completed_run_ttl_secs")]
    pub completed_run_ttl_secs: u64,
}

/// Default TTL for completed runs.
fn default_completed_run_ttl_secs() -> u64 {
    60
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            completed_run_ttl_secs: 60,
        }
    }
}

/// Configuration for pidag, typically read from .pidag/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub project: ProjectConfig,
    pub worker: WorkerConfig,
    pub sdd: SddConfig,
    #[serde(default)]
    pub models: ModelsConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub rpc: RpcConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub default_model: String,
    pub timeout_secs: u64,
    /// Extended-thinking level passed to `pi --thinking` for worker nodes.
    /// Defaults to "low" (observed: high/xhigh thinking can run away and never
    /// terminate on long implement prompts). Valid: off|minimal|low|medium|high|xhigh|max.
    #[serde(default = "default_thinking_level")]
    pub thinking_level: String,
    /// Default provider for bare model names (no prefix). When a model string
    /// like "deepseek-v4-flash" carries no provider prefix, this is used to
    /// resolve the full provider/modelId pair. Defaults to None.
    #[serde(default)]
    pub provider: Option<String>,
}

/// Default thinking level for worker nodes.
fn default_thinking_level() -> String {
    "low".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SddConfig {
    pub max_iterations: usize,
    pub validate_script: PathBuf,
    pub quality_gate_script: PathBuf,
}

impl Default for SddConfig {
    fn default() -> Self {
        // Script path resolution order:
        // 1. Environment variable (PIDAG_VALIDATE_SCRIPT / PIDAG_QUALITY_GATE_SCRIPT)
        // 2. Pi skills path (container): /root/.pi/agent/skills/<skill>/run.sh
        // 3. /usr/local/scripts (container legacy)
        // 4. Claude skills path (local): ~/.claude/skills/.../scripts/*.sh
        let validate_script = std::env::var("PIDAG_VALIDATE_SCRIPT")
            .ok()
            .or_else(|| {
                let paths = [
                    "/root/.pi/agent/skills/validate-exit-criteria/run.sh",
                    "/usr/local/scripts/validate-exit-criteria.sh",
                ];
                paths
                    .iter()
                    .find(|p| PathBuf::from(p).exists())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| {
                // Fallback for local Claude Code usage
                "~/.claude/skills/loop-engineer/scripts/validate-exit-criteria.sh".to_string()
            });

        let quality_gate_script = std::env::var("PIDAG_QUALITY_GATE_SCRIPT")
            .ok()
            .or_else(|| {
                let paths = [
                    "/root/.pi/agent/skills/quality-gate/run.sh",
                    "/usr/local/scripts/quality-gate.sh",
                ];
                paths
                    .iter()
                    .find(|p| PathBuf::from(p).exists())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| {
                // Fallback for local Claude Code usage
                "~/.claude/skills/rust-specialist/scripts/quality-gate.sh".to_string()
            });

        Self {
            max_iterations: 3,
            validate_script: PathBuf::from(validate_script),
            quality_gate_script: PathBuf::from(quality_gate_script),
        }
    }
}

/// Environment variable name for overriding the default model.
pub const ENV_DEFAULT_MODEL: &str = "PIDAG_DEFAULT_MODEL";

impl Config {
    /// Load config from a TOML file, or return defaults if file does not exist.
    pub fn load(path: &Path) -> Result<Self, PidagError> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| PidagError::Parse(format!("Failed to read config: {}", e)))?;

        Self::from_toml_str(&content)
    }

    /// Parse config from TOML string.
    pub fn from_toml_str(content: &str) -> Result<Self, PidagError> {
        let value: toml::Value = toml::from_str(content)
            .map_err(|e| PidagError::Parse(format!("TOML parse error: {}", e)))?;

        let project = ProjectConfig {
            root: PathBuf::from(
                value
                    .get("project")
                    .and_then(|p| p.get("root"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("."),
            ),
        };

        let fallback_default_model = ModelsConfig::default()
            .free
            .first()
            .cloned()
            .unwrap_or_default();
        let worker = WorkerConfig {
            default_model: value
                .get("worker")
                .and_then(|w| w.get("default_model"))
                .and_then(|m| m.as_str())
                .unwrap_or(&fallback_default_model)
                .to_string(),
            timeout_secs: value
                .get("worker")
                .and_then(|w| w.get("timeout_secs"))
                .and_then(|t| t.as_integer())
                .unwrap_or(120) as u64,
            thinking_level: value
                .get("worker")
                .and_then(|w| w.get("thinking_level"))
                .and_then(|t| t.as_str())
                .unwrap_or("low")
                .to_string(),
            provider: value
                .get("worker")
                .and_then(|w| w.get("provider"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string()),
        };

        let sdd_defaults = SddConfig::default();
        let sdd = SddConfig {
            max_iterations: value
                .get("sdd")
                .and_then(|s| s.get("max_iterations"))
                .and_then(|m| m.as_integer())
                .unwrap_or(sdd_defaults.max_iterations as i64) as usize,
            validate_script: PathBuf::from(
                value
                    .get("sdd")
                    .and_then(|s| s.get("validate_script"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(sdd_defaults.validate_script.to_str().unwrap_or("")),
            ),
            quality_gate_script: PathBuf::from(
                value
                    .get("sdd")
                    .and_then(|s| s.get("quality_gate_script"))
                    .and_then(|q| q.as_str())
                    .unwrap_or(sdd_defaults.quality_gate_script.to_str().unwrap_or("")),
            ),
        };

        let models = Self::parse_models(&value);

        let agent = AgentConfig {
            backend: value
                .get("agent")
                .and_then(|a| a.get("backend"))
                .and_then(|b| b.as_str())
                .map(|s| s.to_string()),
        };

        let rpc = RpcConfig {
            completed_run_ttl_secs: value
                .get("rpc")
                .and_then(|r| r.get("completed_run_ttl_secs"))
                .and_then(|t| t.as_integer())
                .unwrap_or(60) as u64,
        };

        Ok(Config {
            project,
            worker,
            sdd,
            models,
            agent,
            rpc,
        })
    }

    fn parse_models(value: &toml::Value) -> ModelsConfig {
        let defaults = ModelsConfig::default();
        let free = value
            .get("models")
            .and_then(|m| m.get("free"))
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or(defaults.free);
        let paid = value
            .get("models")
            .and_then(|m| m.get("paid"))
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or(defaults.paid);
        ModelsConfig { free, paid }
    }

    /// Resolve the effective `ModelsConfig` given an optional CLI `--model`
    /// override. Priority (per spec R4, R7):
    /// 1. CLI `--model` flag (overrides free chain)
    /// 2. `PIDAG_DEFAULT_MODEL` env var (overrides free chain)
    /// 3. `[models]` section of the loaded config
    /// 4. `ModelsConfig::default()` (built-in fallback, last resort)
    pub fn resolve_models(&self, cli_model: Option<&str>) -> ModelsConfig {
        if let Some(m) = cli_model {
            return ModelsConfig {
                free: vec![m.to_string()],
                paid: self.models.paid.clone(),
            };
        }
        if let Ok(env_model) = std::env::var(ENV_DEFAULT_MODEL)
            && !env_model.is_empty()
        {
            return ModelsConfig {
                free: vec![env_model],
                paid: self.models.paid.clone(),
            };
        }
        self.models.clone()
    }

    /// Return the default `config.toml` template written by `pidag attach`.
    /// This is the canonical template; no model string literals live in the
    /// binary itself.
    pub fn default_config_toml(project_root: &Path) -> String {
        let models_defaults = ModelsConfig::default();
        let sdd_defaults = SddConfig::default();
        let free_list = models_defaults
            .free
            .iter()
            .map(|m| format!("  \"{}\"", m))
            .collect::<Vec<_>>()
            .join(",\n");
        let paid_list = models_defaults
            .paid
            .iter()
            .map(|m| format!("  \"{}\"", m))
            .collect::<Vec<_>>()
            .join(",\n");
        let default_model = models_defaults.free.first().cloned().unwrap_or_default();
        let validate_script = sdd_defaults.validate_script.display();
        let quality_gate_script = sdd_defaults.quality_gate_script.display();
        let max_iter = sdd_defaults.max_iterations;
        let root_canon = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let root_display = root_canon.display();

        format!(
            r#"[project]
root = "{root}"

[worker]
default_model = "{default_model}"
timeout_secs = 120
# provider = "deepseek"  # optional: default provider for bare model names

[sdd]
max_iterations = {max_iter}
validate_script = "{validate_script}"
quality_gate_script = "{quality_gate_script}"

[models]
# A2A remote agents: use a URL with an optional #skill fragment.
#   "https://agents.example.com:7422/agents/hermes#research"
# Routed to A2aWorker (curl + JSON-RPC) instead of PiPrintWorker.
# Free models tried in order (SDD iterations 1-2)
free = [
{free_list}
]

# Paid models appended as fallback on iteration 3+
paid = [
{paid_list}
]

[agent]
# Backend name for LLM node orchestration. If not set, uses default routing.
# backend = "mock"

[rpc]
# TTL for completed runs in the server's run map (seconds)
completed_run_ttl_secs = 60
"#,
            root = root_display,
            default_model = default_model,
            max_iter = max_iter,
            validate_script = validate_script,
            quality_gate_script = quality_gate_script,
            free_list = free_list,
            paid_list = paid_list,
        )
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project: ProjectConfig {
                root: PathBuf::from("."),
            },
            worker: WorkerConfig {
                default_model: ModelsConfig::default()
                    .free
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                timeout_secs: 120,
                thinking_level: default_thinking_level(),
                provider: None,
            },
            sdd: SddConfig::default(),
            models: ModelsConfig::default(),
            agent: AgentConfig::default(),
            rpc: RpcConfig::default(),
        }
    }
}
