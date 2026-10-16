// A float read from a dependency, as voidforge-rules might from a TOML file.
// dependency: toml = "=1.1.8"
// expect-float-fn: float_bits
// expect-float-fn: float_bits::{closure#0}

pub fn float_bits(value: &toml::Value) -> Option<u64> {
    value.as_float().map(|x| x.to_bits())
}
