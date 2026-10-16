// A float after a string constant: MIR prints the string's allocation block between the two
// functions, so skipping that block must not hide the float that follows it.
// expect-float-fn: later_bits

pub fn message() -> &'static str {
    "plain text before a float"
}

pub fn later_bits() -> u64 {
    0.75f64.to_bits()
}
