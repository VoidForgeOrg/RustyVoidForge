// Determinism fixture: UUID generation. New entities take World.next_id; player UUIDs arrive in commands.
// expect: clippy::disallowed_methods uuid::Uuid::new_v1
// expect: clippy::disallowed_methods uuid::Uuid::now_v1
// expect: clippy::disallowed_methods uuid::Uuid::new_v3
// expect: clippy::disallowed_methods uuid::Uuid::new_v4
// expect: clippy::disallowed_methods uuid::Uuid::new_v5
// expect: clippy::disallowed_methods uuid::Uuid::new_v6
// expect: clippy::disallowed_methods uuid::Uuid::now_v6
// expect: clippy::disallowed_methods uuid::Uuid::new_v7
// expect: clippy::disallowed_methods uuid::Uuid::now_v7
// expect: clippy::disallowed_methods uuid::Uuid::new_v8
// dependency: uuid = { version = "=1.28.0", features = ["v1", "v3", "v4", "v5", "v6", "v7", "v8"] }

const NODE: [u8; 6] = [1, 2, 3, 4, 5, 6];

pub fn v1(ts: uuid::Timestamp) -> uuid::Uuid {
    uuid::Uuid::new_v1(ts, &NODE)
}

pub fn now_v1() -> uuid::Uuid {
    uuid::Uuid::now_v1(&NODE)
}

pub fn v3() -> uuid::Uuid {
    uuid::Uuid::new_v3(&uuid::Uuid::NAMESPACE_DNS, b"voidforge")
}

pub fn v4() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

pub fn v5() -> uuid::Uuid {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"voidforge")
}

pub fn v6(ts: uuid::Timestamp) -> uuid::Uuid {
    uuid::Uuid::new_v6(ts, &NODE)
}

pub fn now_v6() -> uuid::Uuid {
    uuid::Uuid::now_v6(&NODE)
}

pub fn v7(ts: uuid::Timestamp) -> uuid::Uuid {
    uuid::Uuid::new_v7(ts)
}

pub fn now_v7() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

pub fn v8() -> uuid::Uuid {
    uuid::Uuid::new_v8([0; 16])
}
