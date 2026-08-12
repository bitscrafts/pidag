# Spec: logtool — a three-module log processing toolkit

**Project**: `/projects/pidag/_tmp/e2e-split`

## Overview

A small Python toolkit for working with line-oriented logs. Three independent
modules: parsing, filtering, and reporting. Standard library only.

## Requirements

- R1: `logparse.py` exposes `parse_line(s)` returning a dict with `level`, `msg`
  for lines shaped `LEVEL: message`.
- R2: `logfilter.py` exposes `by_level(records, level)` returning matching records.
- R3: `logreport.py` exposes `count_by_level(records)` returning a dict of counts.
- R4: Each module is independently importable and has no cross-module imports.

## Architecture

Three files: `logparse.py`, `logfilter.py`, `logreport.py`. One test file per
module: `test_logparse.py`, `test_logfilter.py`, `test_logreport.py`.
Standard library only. No package, no `__init__.py`.

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| P1 | parse well-formed | `INFO: hello` | `{'level':'INFO','msg':'hello'}` |
| P2 | parse malformed | `garbage` | `None` |
| F1 | filter matches | 3 records, 2 INFO | 2 returned |
| F2 | filter no match | no ERROR records | empty list |
| R1t | count levels | 2 INFO 1 WARN | `{'INFO':2,'WARN':1}` |
| R2t | count empty | `[]` | `{}` |

## Exit Criteria

- [ ] `test -f logparse.py`
- [ ] `test -f logfilter.py`
- [ ] `test -f logreport.py`
- [ ] `test -f test_logparse.py`
- [ ] `test -f test_logfilter.py`
- [ ] `test -f test_logreport.py`
- [ ] `python3 -c "import logparse; assert logparse.parse_line('INFO: hi')=={'level':'INFO','msg':'hi'}"`
- [ ] `python3 -c "import logparse; assert logparse.parse_line('garbage') is None"`
- [ ] `python3 -c "import logfilter; assert logfilter.by_level([{'level':'INFO'}],'INFO')==[{'level':'INFO'}]"`
- [ ] `python3 -c "import logfilter; assert logfilter.by_level([{'level':'INFO'}],'ERROR')==[]"`
- [ ] `python3 -c "import logreport; assert logreport.count_by_level([{'level':'INFO'},{'level':'INFO'}])=={'INFO':2}"`
- [ ] `python3 -c "import logreport; assert logreport.count_by_level([])=={}"`
- [ ] `python3 -m unittest test_logparse -v`
- [ ] `python3 -m unittest test_logfilter -v`
- [ ] `python3 -m unittest test_logreport -v`

## Guardrails

- Standard library only. No pip installs, no third-party imports.
- No cross-module imports between logparse/logfilter/logreport.
