# Spec: Spec Naming Enforcement

## Overview

**Project**: /projects/pidag
**Phase**: 4b - Spec Validation

Wire `validate_spec_name()` into `pidag sdd` to reject specs without `NN-` prefix.

---

## Requirements

1. `pidag sdd` calls `validate_spec_name()` before generating DAG
2. Specs not matching `^[0-9]{2}-.*\.md$` are rejected with clear error
3. Run ID includes spec stem: `run-YYYYMMDD-HHMMSS-{spec}-{hash}`

---

## Exit Criteria

- [ ] `pidag sdd specs/fibonacci.md` fails with "invalid spec name" error
- [ ] `pidag sdd specs/01-fibonacci.md` succeeds
- [ ] Run ID contains spec stem (e.g., `run-20260805-091234-01-fibonacci-abc123`)

---

## Guardrails

- Do NOT break existing working specs with NN- prefix
- Error message must explain the required format
