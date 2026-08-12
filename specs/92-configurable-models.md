# Configurable Models — Remove Hardcoded Model Names

- **Project**: `${WORKSPACE}/20260601-on-research/crates/pidag`
- **Crate**: `pidag`
- **Priority**: CRITICAL (blocks provider switching)

---

## Overview

Model names are hardcoded throughout the pidag codebase, making it impossible to
switch providers when one hits rate limits (429) or goes down. This violates basic
configuration principles and blocks production use.

**Current state**: 6 hardcoded references to `google/gemini-3.6-flash` and 1 to
`google/gemini-2.5-pro`.

**Target state**: All model configuration comes from `.pidag/config.toml` or CLI flags.

---

## Requirements

**R1**: Remove ALL hardcoded model names from source code.

**R2**: `.pidag/config.toml` defines model chains:
```toml
[models]
# Free models (tried in order)
free = [
  "nvidia/z-ai/glm-5.2",
  "google/gemini-3.6-flash",
]

# Paid models (fallback when free exhausted)
paid = [
  "nvidia/meta/llama-3.3-70b-instruct",
  "google/gemini-2.5-pro",
]
```

**R3**: SDD generator reads models from config, not hardcoded values.

**R4**: CLI flag `--model <model>` overrides config default.

**R5**: CLI flag `--models-config <path>` allows custom config file.

**R6**: DAG JSON `models` array takes precedence over config (already works).

**R7**: Environment variable `PIDAG_DEFAULT_MODEL` overrides config.

---

## Architecture

```
Priority (highest to lowest):
1. DAG JSON node.models array (per-node override)
2. CLI --model flag (session override)
3. ENV PIDAG_DEFAULT_MODEL (environment override)
4. .pidag/config.toml [models] section (project default)
5. Built-in fallback (only if nothing else specified)
```

### Config Loading

```rust
// crates/pidag/src/config.rs

pub struct ModelsConfig {
    pub free: Vec<String>,
    pub paid: Vec<String>,
}

impl Config {
    pub fn load_models(&self) -> ModelsConfig {
        // 1. Check CLI override
        // 2. Check ENV override
        // 3. Load from config.toml
        // 4. Return built-in fallback only if all else fails
    }
}
```

### SDD Generator Update

```rust
// crates/pidag/src/sdd.rs

fn generate_impl_node(config: &Config, iteration: usize) -> Node {
    let models_config = config.load_models();

    Node {
        models: if iteration == 3 {
            // Iteration 3: include paid fallbacks
            models_config.free.iter()
                .map(|m| ModelRef { name: m.clone(), paid: false })
                .chain(models_config.paid.iter()
                    .map(|m| ModelRef { name: m.clone(), paid: true }))
                .collect()
        } else {
            // Iterations 1-2: free only
            models_config.free.iter()
                .map(|m| ModelRef { name: m.clone(), paid: false })
                .collect()
        },
        // ...
    }
}
```

---

## TDD Contract

| # | Test name | Given | Expected |
|---|-----------|-------|----------|
| T1 | `test_config_loads_models_from_toml` | config.toml with [models] | ModelsConfig populated |
| T2 | `test_cli_model_overrides_config` | --model flag | Flag takes precedence |
| T3 | `test_env_model_overrides_config` | PIDAG_DEFAULT_MODEL set | Env takes precedence |
| T4 | `test_dag_models_override_all` | DAG JSON with models | DAG wins |
| T5 | `test_sdd_uses_config_models` | config.toml models | Generated DAG uses config |
| T6 | `test_fallback_when_no_config` | No config file | Uses built-in defaults |

---

## Exit Criteria

- [x] `grep -r "google/gemini" crates/pidag/src/ | grep -v test | wc -l` returns 0
- [x] `grep -r "nvidia/z-ai" crates/pidag/src/ | grep -v test | grep -v config.rs | wc -l` returns 0
- [x] `.pidag/config.toml` example includes [models] section
- [x] `pidag sdd spec.md --model nvidia/z-ai/glm-5.2` works
- [x] `cargo test -p pidag` passes (134+ tests)
- [x] `cargo clippy -p pidag -- -D warnings` clean

---

## Guardrails

- Must NOT break existing DAG JSON format
- Must NOT require config.toml (fallback to built-in defaults)
- Must NOT change Worker trait or dispatch logic
- Built-in defaults are LAST resort, not first choice
- Config validation: warn if model format looks wrong (no `/`)

---

## Files to Modify

1. `crates/pidag/src/config.rs` - Add ModelsConfig, load_models()
2. `crates/pidag/src/sdd.rs` - Use config instead of hardcoded
3. `crates/pidag/src/bin/pidag.rs` - Add --model flag, --models-config flag
4. `crates/pidag/src/config.rs.md` - Update documentation
5. Tests in `crates/pidag/tests/`

---

## Available Models (for defaults)

**NVIDIA (working)**:
- `nvidia/z-ai/glm-5.2` - Free, fast
- `nvidia/meta/llama-3.3-70b-instruct` - Paid, high quality
- `nvidia/deepseek-ai/deepseek-v4-flash` - Free, coding

**Google (quota issues on free tier)**:
- `google/gemini-3.6-flash` - Free tier limited
- `google/gemini-2.5-pro` - Paid

**Recommended defaults**:
```toml
[models]
free = ["nvidia/z-ai/glm-5.2", "nvidia/deepseek-ai/deepseek-v4-flash"]
paid = ["nvidia/meta/llama-3.3-70b-instruct"]
```
