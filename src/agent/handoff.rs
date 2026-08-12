//! HANDOFF.md reading and updating.
//!
//! The autonomous driver reads a project's `HANDOFF.md` to pick the next spec,
//! and appends a concise session summary after implementation.

use std::path::Path;

/// Read the text of a project's `HANDOFF.md`. Returns `Ok(None)` when the file
/// does not exist yet.
pub fn read_handoff(project_root: &Path) -> Result<Option<String>, String> {
    let path = project_root.join("HANDOFF.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("failed to read HANDOFF.md: {e}")),
    }
}

/// Try to find a spec file referenced by the handoff's "next steps". Returns
/// the filename (e.g. `specs/03-queue.md` or `03-queue.md`) if it appears to
/// name one, otherwise `None`.
pub fn spec_nominated_by_handoff(handoff: Option<&str>) -> Option<String> {
    let text = handoff?;
    // Look for a `specs/NN-*.md` or `NN-*.md` mention in the handoff.
    for line in text.lines() {
        // e.g. `specs/03-queue.md`, `spec 03-queue.md`, or `next: 03-queue`
        for tok in line.split_whitespace() {
            let cleaned = tok.trim_matches(|c: char| c == '`' || c == ',' || c == '.' || c == ')');
            if cleaned.contains("spec")
                || cleaned.starts_with("specs/")
                || cleaned.starts_with("NN-")
            {
                // Prefer explicit specs/NN-x.md paths.
                if cleaned.contains('/') && cleaned.ends_with(".md") {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

/// Append a dated session summary to `HANDOFF.md`, creating it if absent.
pub fn update_handoff(
    project_root: &Path,
    spec_file: &str,
    outcome: &str,
    duration_secs: u64,
) -> Result<(), String> {
    let path = project_root.join("HANDOFF.md");
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("failed to read HANDOFF.md: {e}")),
    };

    let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let block = format!(
        "\n---\n\n## Auto session ({stamp})\n\n- **State**: {outcome}\n- **Spec**: {spec_file}\n- **Duration**: {duration_secs}s\n- **Driver**: pidag auto\n"
    );

    let updated = format!("{existing}{block}");
    std::fs::write(&path, updated).map_err(|e| format!("failed to write HANDOFF.md: {e}"))
}

/// The declared flavor of an autonomous work target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// Implement a pending spec (dispatch `pidag sdd <spec> --run`).
    Implement,
    /// Open-ended research / improvement (dispatch a flexible DAG).
    Research,
    /// Debug a failing behaviour (reproduce + fix + regression test).
    Debug,
    /// Refactor existing code (analyse + tests + apply).
    Refactor,
}

/// A parsed HANDOFF "Work Direction" declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPlan {
    /// The kind of work to do.
    pub kind: WorkKind,
    /// Optional spec file or free-form target named by the block.
    pub target: Option<String>,
}

/// Try to read a HANDOFF "Work Direction" block:
///
/// ```markdown
/// ## Work Direction
/// - mode: implement | research | refactor | debug | improve
/// - target: <spec file or free-form improvement>
/// ```
///
/// Returns `None` when no such block exists (caller falls back to the
/// pending-spec rule).
pub fn work_direction(handoff: Option<&str>) -> Option<WorkPlan> {
    let text = handoff?;
    let mut kind: Option<WorkKind> = None;
    let mut target: Option<String> = None;
    let mut capturing = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if capturing {
            // End of the block at the next heading.
            if trimmed.starts_with('#') {
                break;
            }
            if let Some(v) = strip_key(trimmed, "mode") {
                match v.trim() {
                    "implement" => kind = Some(WorkKind::Implement),
                    "research" | "improve" => kind = Some(WorkKind::Research),
                    "debug" => kind = Some(WorkKind::Debug),
                    "refactor" => kind = Some(WorkKind::Refactor),
                    _ => {}
                }
            } else if let Some(v) = strip_key(trimmed, "target") {
                target = Some(v.trim().to_string());
            }
            continue;
        }
        // Look for the heading that starts the block.
        if trimmed.starts_with('#') && trimmed.to_lowercase() == "## work direction" {
            capturing = true;
        }
    }

    kind.map(|k| WorkPlan { kind: k, target })
}

/// Pull the value for a `key:` list item, normalising leading `- ` bullets.
fn strip_key(line: &str, key: &str) -> Option<String> {
    let no_bullet = line.trim_start_matches("- ").trim_start_matches("* ");
    let (k, v) = no_bullet.split_once(':')?;
    if k.trim() == key {
        Some(v.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominate_spec_from_next_steps() {
        let h = Some("## Next steps\n- implement specs/03-queue.md next");
        assert_eq!(
            spec_nominated_by_handoff(h),
            Some("specs/03-queue.md".to_string())
        );
    }

    #[test]
    fn no_nomination_when_no_spec() {
        let h = Some("## Next steps\n- refactor the worker timeouts");
        assert_eq!(spec_nominated_by_handoff(h), None);
    }

    #[test]
    fn none_handoff_yields_none() {
        assert_eq!(spec_nominated_by_handoff(None), None);
    }

    #[test]
    fn parses_implement_direction() {
        let h = Some("## Work Direction\n- mode: implement\n- target: specs/05-queue.md");
        let plan = work_direction(h);
        assert!(plan.is_some());
        let p = plan.unwrap();
        assert_eq!(p.kind, WorkKind::Implement);
        assert_eq!(p.target.as_deref(), Some("specs/05-queue.md"));
    }

    #[test]
    fn parses_research_direction() {
        let h = Some("## Work Direction\n- mode: research\n- target: splitter speedups");
        let p = work_direction(h).unwrap();
        assert_eq!(p.kind, WorkKind::Research);
        assert_eq!(p.target.as_deref(), Some("splitter speedups"));
    }

    #[test]
    fn no_direction_when_absent() {
        let h = Some("## Next steps\n- implement specs/03-queue.md next");
        assert_eq!(work_direction(h), None);
    }

    #[test]
    fn direction_ends_at_next_heading() {
        let h = Some("## Work Direction\n- mode: debug\n\n## Next steps\n- go");
        assert_eq!(work_direction(h).unwrap().kind, WorkKind::Debug);
    }
}
