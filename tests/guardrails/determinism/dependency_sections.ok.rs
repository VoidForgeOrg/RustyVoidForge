// Harness control (review-r1's gate LOW on run_fixtures.py's dedupe): voidforge-core declares
// serde_json only as a dev-dependency, so this library-code fixture compiles only if the harness
// adds serde_json under [dependencies] rather than treating the dev-dependency as declared.
// dependency: serde_json = { workspace = true }

pub fn encoded(value: u64) -> Option<String> {
    serde_json::to_string(&value).ok()
}
