# Guardrail fixtures

The P0.4 guardrails and their proof: the determinism bans in `voidforge-core`, `voidforge-rules` and `voidforge-worldgen` ([D-010](../../docs/decision-log.md#d-010-determinism-bans-in-core-rules-and-worldgen)), the type-aware float check ([D-013](../../docs/decision-log.md#d-013-type-aware-float-check)) and `voidforge-web`'s dependency isolation ([D-011](../../docs/decision-log.md#d-011-web-dependency-isolation-check)). The CI `lint` job runs all of these scripts.

```sh
python3 tests/guardrails/check_no_floats.py     # no f32/f64 in the three crates' compiled MIR
python3 tests/guardrails/check_web_deps.py      # the real check on this workspace
python3 tests/guardrails/run_fixtures.py        # every fixture case; --only TEXT, --verbose
```

`run_fixtures.py` works on a temporary copy of the workspace, so nothing here is a workspace member. The [harness docstring](run_fixtures.py) explains the matching rules ([D-012](../../docs/decision-log.md#d-012-guardrail-fixtures)).

| Folder | Holds |
| --- | --- |
| `determinism/<category>.rs` | Code using every banned item of one category, with an `// expect: <lint> <path>` line per diagnostic. It must be rejected in all three crates with exactly those diagnostics, and must pass in `voidforge-host` |
| `determinism/<category>.ok.rs` | The allowed alternative; must pass in all three crates |
| `float_values/<name>.rs` | Floats clippy cannot see; `check_no_floats.py` must reject them, naming every `// expect-float-fn:` item. `.ok.rs` must pass and `.err.rs` must exit 2 |
| `meta/` | Traps for the harness: a ban that does not fire must be reported MISSING, and an unrelated error must be reported ERROR |

`// dependency: <toml line>` adds a crate to the copy under test; its version is pinned in the [version table](../../docs/versions.md).

To ban something new:
1. Add the path to all three `clippy.toml` files; they must stay identical.
2. Add an `expect:` line and code that triggers it to the category's fixture.
3. Add a decision-log entry.

The run fails if a banned path has no fixture.
