# Legacy vault fixture

A vault written by the build at commit `479171a`, **before** spec-34 replaces the
stringly-typed node state with a `NodeStatus` enum.

Its only job is to fail if that refactor changes the wire format. `NodeStatus` must
serialise to exactly the strings already on disk — `"Done"`, `"Failed"`,
`"Blocked"` — because vaults written by earlier builds have to keep loading. A
compatibility test written *after* the change proves nothing about what is already
stored, which is why this was generated first.

## Contents

Run `legacy1`, three shell nodes covering all three terminal states:

| node | state | how |
|---|---|---|
| `alpha` | `Done` | `echo done-alpha` |
| `beta` | `Failed` | `exit 3` |
| `gamma` | `Blocked` | depends on `beta`, which failed |

`Blocked` is present only because a defect was fixed first: `RedbSink::emit` had no
arm for `Event::NodeBlocked` and `terminal_set` filtered to `Done | Failed`, so the
state was never persisted and `load_checkpoint`'s `"Blocked"` branch was
unreachable. Generating this fixture is what surfaced that (`479171a`).

## Using it

```rust
let store = RedbStore::open(Path::new("tests/fixtures/legacy_vault/legacy.redb"))?;
let terminal = store.terminal_set("legacy1").await?;
// must contain ("alpha","Done"), ("beta","Failed"), ("gamma","Blocked")
```

Do not regenerate it after spec-34 lands — that would defeat its purpose.
