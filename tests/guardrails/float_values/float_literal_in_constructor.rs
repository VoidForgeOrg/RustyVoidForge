// A float literal given to an enum constructor from a dependency, as voidforge-rules might build
// a TOML value: the MIR line is `toml::Value::Float(const 0.5f64)`, with no float-typed local and
// no float-named API, so only the literal's `f64` suffix shows the float.
// dependency: toml = "=1.1.8"
// expect-float-fn: half_value

pub fn half_value() -> toml::Value {
    toml::Value::Float(0.5)
}
