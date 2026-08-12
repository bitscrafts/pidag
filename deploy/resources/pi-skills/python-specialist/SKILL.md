---
name: python-specialist
description: >
  Python development using UV package manager exclusively. NEVER use pip/conda.
  Use for running scripts, adding dependencies, and managing Python projects.
version: 1.0.0
allowed-tools: [bash]
---

# python-specialist

Python development using UV package manager exclusively.

## Usage

```bash
pi --skill python-specialist <command> [args...]
```

## Commands

### run
Run a Python script via UV.

```bash
pi --skill python-specialist run script.py
pi --skill python-specialist run -m module_name
```

### add
Add a dependency using UV.

```bash
pi --skill python-specialist add requests
pi --skill python-specialist add "pandas>=2.0"
```

### init
Initialize a new Python project with UV.

```bash
pi --skill python-specialist init [project_name]
```

### sync
Sync dependencies from pyproject.toml.

```bash
pi --skill python-specialist sync
```

### venv
Create or manage virtual environment.

```bash
pi --skill python-specialist venv
```

## UV Rules (CRITICAL)

**ALWAYS use UV. NEVER use pip, conda, or virtualenv directly.**

```bash
# CORRECT
uv run script.py
uv add package-name
uv sync

# WRONG (NEVER DO THIS)
python script.py
pip install package
conda install package
```

## Project Structure

```
project/
├── pyproject.toml     # UV project config
├── uv.lock            # Lock file
├── .venv/             # Virtual environment
└── src/
    └── module/
        └── __init__.py
```

## Output

Returns JSON status of commands executed.
