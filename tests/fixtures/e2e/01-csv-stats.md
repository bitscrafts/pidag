# Spec: csvstats — a CSV column statistics tool

**Project**: `/projects/pidag/_tmp/e2e-trial`

## Overview

A single-file Python CLI that reads a CSV and prints per-column statistics for
numeric columns. Real, self-contained, and objectively verifiable.

## Requirements

- R1: `csvstats.py <file.csv>` reads the file and detects which columns are numeric.
- R2: For each numeric column it prints `name,count,min,max,mean` — one row per column,
  comma separated, header line `column,count,min,max,mean`.
- R3: Non-numeric columns are skipped silently.
- R4: A missing file exits non-zero with a message on stderr.
- R5: Mean is rounded to 2 decimal places.

## Architecture

Single file `csvstats.py`, standard library only (`csv`, `sys`). No dependencies.
A `main()` guarded by `if __name__ == "__main__"`.

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| T1 | numeric detection | CSV with `name,age,city` | only `age` reported |
| T2 | stats correct | ages 10,20,30 | `age,3,10.0,30.0,20.0` |
| T3 | missing file | nonexistent path | exit non-zero, stderr non-empty |
| T4 | rounding | ages 1,2 | mean `1.5` |

Tests live in `test_csvstats.py` and run with `python3 -m unittest`.

## Exit Criteria

- [ ] `test -f csvstats.py`
- [ ] `test -f test_csvstats.py`
- [ ] `python3 -m unittest test_csvstats -v` exits 0
- [ ] `printf 'name,age\na,10\nb,20\n' > /tmp/t.csv && python3 csvstats.py /tmp/t.csv | grep -q 'age,2,10.0,20.0,15.0'`

## Guardrails

- Standard library only. No pip installs.
- Do not create files other than `csvstats.py` and `test_csvstats.py`.
