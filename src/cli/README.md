# pidag CLI Module

This module organizes all `pidag` subcommand implementations. Each command is in its own file, imported and re-exported by `mod.rs`.

## Architecture

```
pidag::cli
├── mod.rs         re-exports all command functions
├── run.rs         pidag run — execute DAG from JSON
├── show.rs        pidag show — display run results
├── list.rs        pidag list — list stored runs
├── attach.rs      pidag attach — initialize pidag in project
├── sdd.rs         pidag sdd — generate SDD loop DAG from spec
├── serve.rs       pidag serve — start JSON-RPC server
├── mcp.rs         pidag mcp — start MCP (Model Context Protocol) server
├── ui.rs          pidag ui — start trace UI web server
└── describe.rs    pidag describe — render DAG as markdown + mermaid
```

## Command Function Signature

Every command implements:

```rust
pub async fn <command>(args: &[String]) -> Result<(), Box<dyn std::error::Error>>
```

- **Input**: Remaining CLI arguments after the subcommand (e.g., `pidag run foo.json --vault x.db` → `args = ["foo.json", "--vault", "x.db"]`)
- **Output**: `Result` allowing the binary dispatcher to handle errors uniformly
- **Async**: All commands are async to support operations like `tokio::spawn` and futures

## Usage From Binary

The dispatcher in `src/bin/pidag.rs` matches subcommands to CLI functions:

```rust
match args[1].as_str() {
    "run"      => cli::run(&args[2..]).await,
    "show"     => cli::show(&args[2..]).await,
    "list"     => cli::list(&args[2..]).await,
    // ... etc
}
```

## Argument Parsing

Each command parses its own arguments inline:

```rust
pub async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("error: 'run' requires a <dag.json> argument");
        std::process::exit(1);
    }

    let dag_path = &args[0];
    let mut concurrency = 4usize;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--concurrency" => {
                if i + 1 < args.len() {
                    concurrency = args[i + 1].parse().unwrap_or(4);
                    i += 2;
                } else {
                    eprintln!("error: --concurrency requires an argument");
                    std::process::exit(1);
                }
            }
            // ... handle other options
        }
    }

    // Command implementation
    Ok(())
}
```

### Future Improvement: Use a CLI Parser

Consider migrating to `clap` or `structopt` for more robust argument handling:

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
```

This would allow declarative command/subcommand definitions and automatic help generation.

## Crate Dependencies

Commands import types from the parent `pidag` crate:

```rust
use pidag::{Dag, Store, RedbStore, Config, ...};
```

Commands are implemented in the `pidag` library, making them testable independently of the binary.

## Adding a New Command

1. Create `src/cli/newcmd.rs`:
   ```rust
   pub async fn newcmd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
       // implementation
   }
   ```

2. Add to `src/cli/mod.rs`:
   ```rust
   pub mod newcmd;
   pub use self::newcmd::newcmd;
   ```

3. Add to binary dispatcher in `src/bin/pidag.rs`:
   ```rust
   "newcmd" => cli::newcmd(&args[2..]).await,
   ```

4. Update help text in binary's `print_help()`.

## Testing

Commands can be tested directly via the public API:

```rust
#[tokio::test]
async fn test_run_command() {
    let args = vec!["test.json".to_string()];
    assert!(pidag::cli::run(&args).await.is_ok());
}
```

(Note: This requires test fixtures and a vault setup; see `crates/pidag/tests/` for integration test patterns.)

## See Also

- `src/bin/pidag.rs` — Binary entry point and dispatcher
- `src/lib.rs` — Crate root, exposes `pub mod cli`
- `CLAUDE.md` — Project-specific development guidelines
