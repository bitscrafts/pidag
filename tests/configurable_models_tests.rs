//! Tests for the configurable-models spec (R1-R7, T1-T6).
//!
//! Verifies the priority chain:
//!   DAG JSON > CLI --model > ENV PIDAG_DEFAULT_MODEL > .pidag/config.toml [models] > built-in
//!
//! All temp files are written under `_tmp/` per workspace convention.

use pidag::{Config, ModelsConfig, SddConfig, SddGenerator};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `PIDAG_DEFAULT_MODEL` is process-global, and `cargo test` runs tests in
/// parallel within a binary. Tests that touch the env var must hold this lock
/// to avoid interfering with each other.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const SPEC_TEMPLATE: &str = r#"## TDD Contract

| Test | Given | Expected |
|------|-------|----------|
| T1 | input | output |

## Architecture

Build it this way.

## Exit Criteria

- Criteria 1

## Guardrails

- Guard 1
"#;

fn fresh_tmpdir(name: &str) -> PathBuf {
    let tmpdir = PathBuf::from(format!("_tmp/configurable_models/{}", name));
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    tmpdir
}

fn write_spec(tmpdir: &Path) -> PathBuf {
    let spec_path = tmpdir.join("spec.md");
    std::fs::write(&spec_path, SPEC_TEMPLATE).unwrap();
    spec_path
}

// ============================================================================
// T1: test_config_loads_models_from_toml
// ============================================================================
#[test]
fn test_config_loads_models_from_toml() {
    let tmpdir = fresh_tmpdir("t1_load_from_toml");
    let config_path = tmpdir.join("config.toml");
    std::fs::write(
        &config_path,
        r#"[project]
root = "."

[worker]
default_model = "nvidia/z-ai/glm-5.2"
timeout_secs = 120

[sdd]
max_iterations = 3

[models]
free = ["alpha/free-m1", "alpha/free-m2"]
paid = ["alpha/paid-m1"]
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    assert_eq!(config.models.free, vec!["alpha/free-m1", "alpha/free-m2"]);
    assert_eq!(config.models.paid, vec!["alpha/paid-m1"]);
}

// ============================================================================
// T2: test_cli_model_overrides_config
// ============================================================================
#[test]
fn test_cli_model_overrides_config() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    // Ensure no env leakage from T3 while we test the no-CLI path.
    let prev = std::env::var(pidag::config::ENV_DEFAULT_MODEL).ok();
    unsafe {
        std::env::remove_var(pidag::config::ENV_DEFAULT_MODEL);
    }

    let tmpdir = fresh_tmpdir("t2_cli_override");
    let config_path = tmpdir.join("config.toml");
    std::fs::write(
        &config_path,
        r#"[project]
root = "."

[models]
free = ["config/free-model"]
paid = ["config/paid-model"]
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();

    // No CLI flag and no env → config values used
    let resolved = config.resolve_models(None);
    assert_eq!(resolved.free, vec!["config/free-model"]);
    assert_eq!(resolved.paid, vec!["config/paid-model"]);

    // CLI flag overrides free chain only; paid chain preserved from config.
    // CLI also wins over env (which is still unset here).
    let resolved = config.resolve_models(Some("cli/override-model"));
    assert_eq!(resolved.free, vec!["cli/override-model"]);
    assert_eq!(resolved.paid, vec!["config/paid-model"]);

    // Restore prior env state.
    unsafe {
        if let Some(v) = &prev {
            std::env::set_var(pidag::config::ENV_DEFAULT_MODEL, v);
        }
    }
}

// ============================================================================
// T3: test_env_model_overrides_config
// ============================================================================
#[test]
fn test_env_model_overrides_config() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let env_key = pidag::config::ENV_DEFAULT_MODEL;
    let unique_value = "env/override-model-unique-3a7f2";
    let previous = std::env::var(env_key).ok();
    // SAFETY: env var mutation is serialized by ENV_LOCK; we restore the prior
    // value afterwards.
    unsafe {
        std::env::set_var(env_key, unique_value);
    }

    let result = std::panic::catch_unwind(|| {
        let tmpdir = fresh_tmpdir("t3_env_override");
        let config_path = tmpdir.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[project]
root = "."

[models]
free = ["config/free-model"]
paid = ["config/paid-model"]
"#,
        )
        .unwrap();

        let config = Config::load(&config_path).unwrap();
        let resolved = config.resolve_models(None);
        assert_eq!(resolved.free, vec![unique_value]);
        assert_eq!(resolved.paid, vec!["config/paid-model"]);
    });

    // Restore prior env state.
    unsafe {
        match &previous {
            Some(v) => std::env::set_var(env_key, v),
            None => std::env::remove_var(env_key),
        }
    }

    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

// ============================================================================
// T4: test_dag_models_override_all
// ============================================================================
#[test]
fn test_dag_models_override_all() {
    // DAG JSON node.models array always wins (already handled by the scheduler).
    // Here we verify the DAG JSON parser populates node.models from the file,
    // independent of any Config or ModelsConfig.
    let tmpdir = fresh_tmpdir("t4_dag_override");
    let dag_path = tmpdir.join("dag.json");
    std::fs::write(
        &dag_path,
        r#"{
  "nodes": [
    {
      "id": "n1",
      "prompt": "do something",
      "depends_on": [],
      "models": [{"name": "dag/explicit-model", "paid": true}],
      "retry": {"attempts": 1, "backoff_ms": 0}
    }
  ]
}"#,
    )
    .unwrap();

    let dag_json = std::fs::read_to_string(&dag_path).unwrap();
    let dag: pidag::Dag = serde_json::from_str(&dag_json).unwrap();
    assert_eq!(dag.nodes.len(), 1);
    assert!(!dag.nodes[0].models.is_empty());
    assert_eq!(dag.nodes[0].models[0].name, "dag/explicit-model");
    assert!(dag.nodes[0].models[0].paid);
}

// ============================================================================
// T5: test_sdd_uses_config_models
// ============================================================================
#[test]
fn test_sdd_uses_config_models() {
    let tmpdir = fresh_tmpdir("t5_sdd_uses_config");
    let spec_path = write_spec(&tmpdir);

    let custom = ModelsConfig {
        free: vec!["custom/free-1".to_string(), "custom/free-2".to_string()],
        paid: vec!["custom/paid-1".to_string()],
    };

    let dag = SddGenerator::from_spec(&spec_path, &tmpdir, &custom, &SddConfig::default()).unwrap();

    // iter1 should have the two custom free models, paid flag false
    let iter1 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter1")
        .unwrap();
    assert_eq!(iter1.models.len(), 2);
    assert_eq!(iter1.models[0].name, "custom/free-1");
    assert!(!iter1.models[0].paid);
    assert_eq!(iter1.models[1].name, "custom/free-2");

    // iter3 should chain free then paid (paid marked true)
    let iter3 = dag
        .nodes
        .iter()
        .find(|n| n.id == "implement-iter3")
        .unwrap();
    assert_eq!(iter3.models.len(), 3);
    assert_eq!(iter3.models[2].name, "custom/paid-1");
    assert!(iter3.models[2].paid);

    // None of the generated model names should be the built-in defaults.
    for node in &dag.nodes {
        for m in &node.models {
            assert!(!m.name.contains("nvidia/z-ai"));
            assert!(!m.name.contains("llama-3.3"));
        }
    }
}

// ============================================================================
// T6: test_fallback_when_no_config
// ============================================================================
#[test]
fn test_fallback_when_no_config() {
    // Serialize with the env-mutating tests so PIDAG_MODEL doesn't leak in
    // (intermittent parallel failure: resolve_models reads the env).
    let _env_guard = ENV_LOCK.lock().unwrap();
    // Default Config -> built-in ModelsConfig::default()
    let config = Config::default();
    let models = config.models.clone();
    assert!(!models.free.is_empty());
    assert!(!models.paid.is_empty());
    // The single source of truth: built-in fallback chain.
    assert_eq!(models.free, ModelsConfig::default().free);
    assert_eq!(models.paid, ModelsConfig::default().paid);

    // resolve_models with no CLI and no env returns the same fallback.
    let resolved = config.resolve_models(None);
    assert_eq!(resolved.free, models.free);
    assert_eq!(resolved.paid, models.paid);
}

// ============================================================================
// T7: test_sdd_uses_config_script_paths (P0-2 fix)
// ============================================================================
// sdd.rs must use SddConfig.validate_script and SddConfig.quality_gate_script
// instead of hardcoded ~/.claude/skills/... paths. A custom SddConfig must
// propagate its script paths into the generated node prompts.
// ============================================================================
#[test]
fn test_sdd_uses_config_script_paths() {
    let tmpdir = fresh_tmpdir("t7_sdd_script_paths");
    let spec_path = write_spec(&tmpdir);

    let custom_sdd = SddConfig {
        max_iterations: 3,
        validate_script: PathBuf::from("/custom/validate-exit-criteria.sh"),
        quality_gate_script: PathBuf::from("/custom/quality-gate.sh"),
    };

    let dag = SddGenerator::from_spec(&spec_path, &tmpdir, &ModelsConfig::default(), &custom_sdd)
        .unwrap();

    // Every validate-* node prompt must contain the custom validate script.
    for node in dag.nodes.iter().filter(|n| n.id.starts_with("validate-")) {
        assert!(
            node.prompt.contains("/custom/validate-exit-criteria.sh"),
            "validate node {} did not use custom validate_script: {}",
            node.id,
            node.prompt
        );
        assert!(
            !node.prompt.contains("~/.claude/skills"),
            "validate node {} still uses hardcoded ~/.claude path: {}",
            node.id,
            node.prompt
        );
    }

    // Every quality-gate-* node prompt must contain the custom quality-gate script.
    for node in dag
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("quality-gate-"))
    {
        assert!(
            node.prompt.contains("/custom/quality-gate.sh"),
            "quality-gate node {} did not use custom quality_gate_script: {}",
            node.id,
            node.prompt
        );
        assert!(
            !node.prompt.contains("~/.claude/skills"),
            "quality-gate node {} still uses hardcoded ~/.claude path: {}",
            node.id,
            node.prompt
        );
    }
}
