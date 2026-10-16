// Control for uuid_generation.rs: UUIDs that arrive in commands may be carried and parsed.
// dependency: uuid = { version = "=1.28.0" }
pub fn from_command(player_id: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(player_id)
}

pub fn parse(text: &str) -> bool {
    uuid::Uuid::parse_str(text).is_ok()
}
