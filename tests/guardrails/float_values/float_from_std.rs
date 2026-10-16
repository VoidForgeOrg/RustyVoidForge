// A float returned by std and turned into an integer without naming its type.
// expect-float-fn: seconds_bits

pub fn seconds_bits(d: std::time::Duration) -> u64 {
    d.as_secs_f64().to_bits()
}
