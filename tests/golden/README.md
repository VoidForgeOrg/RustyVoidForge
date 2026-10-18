# Golden logs

One directory per scenario in `voidforge_testkit::golden`, recorded through the engine host (decision log [D-040](../../docs/decision-log.md#d-040-golden-logs-scenarios-recording-the-fixture-build-and-bless)):

- `start.vfsnap`: the start World, a snapshot envelope.
- `records.jsonl`: the log records, one per line: the command, its outcome and the events.
- `manifest.json`: the state hash after every record, the end hash and the scores.

These files change only through `just bless`. CI fails if they differ from a fresh recording. A reviewer checks every change, so a PR that blesses calls it out.
