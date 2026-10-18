//! A log record's one-line JSON form (P2.6, decision log D-040): the golden files' records. It
//! reads back exactly, extreme values included, and refuses what a Postgres row would refuse.

use voidforge_core::command::{Action, Command, Rejection};
use voidforge_core::state::{BuildingId, BuildingKind, DomainEvent, FleetId, PlanetId, PlayerId};
use voidforge_store::log::{IdempotencyKey, LogError, LogRecord, Outcome, Payload, RecordKind};
use voidforge_store::snapshot::EngineBuild;

fn command_record(player: PlayerId) -> LogRecord {
    LogRecord {
        seq: i64::MAX,
        kind: RecordKind::Command,
        game_time: i64::MAX,
        wall_time_ms: i64::MIN,
        player: Some(player),
        idempotency_key: Some(IdempotencyKey::new("k-\"quoted\"\\").unwrap()),
        payload: Payload::Command(Command {
            player,
            action: Action::RecallFleet {
                fleet: FleetId(u64::MAX),
            },
        }),
        outcome: Outcome::Rejected(Rejection::UnknownFleet),
        events: vec![DomainEvent::BuildingCompleted {
            planet: PlanetId(u64::MAX),
            building: BuildingId(1),
            kind: BuildingKind::Shipyard,
        }],
        rules_version: u32::MAX,
        engine_build: EngineBuild::new("0.1.0+golden").unwrap(),
    }
}

fn advance_record() -> LogRecord {
    LogRecord {
        seq: 1,
        kind: RecordKind::Advance,
        game_time: 0,
        wall_time_ms: 0,
        player: None,
        idempotency_key: None,
        payload: Payload::Empty,
        outcome: Outcome::Empty,
        events: Vec::new(),
        rules_version: 0,
        engine_build: EngineBuild::new("b").unwrap(),
    }
}

#[test]
fn a_line_reads_back_exactly_with_extreme_values() {
    for record in [
        command_record(PlayerId(u128::MAX)),
        command_record(PlayerId(0)),
        advance_record(),
        LogRecord {
            outcome: Outcome::Accepted {
                own_events: u32::MAX,
            },
            ..command_record(PlayerId(1))
        },
    ] {
        let line = record.to_line().unwrap();
        assert!(!line.contains('\n'), "one line: {line}");
        assert_eq!(LogRecord::from_line(&line).unwrap(), record, "{line}");
    }
}

#[test]
fn a_line_holds_the_documented_json_forms_as_values() {
    let line = advance_record().to_line().unwrap();
    assert_eq!(
        line,
        r#"{"seq":1,"kind":"advance","game_time":0,"wall_time_ms":0,"player_id":null,"idempotency_key":null,"payload":null,"outcome":null,"events":[],"rules_version":0,"engine_build":"b"}"#
    );
    let line = command_record(PlayerId(u128::MAX)).to_line().unwrap();
    // The player as its UUID; the outcome as D-036's object; the payload and events as values.
    assert!(line.contains(r#""player_id":"ffffffff-ffff-ffff-ffff-ffffffffffff""#));
    assert!(line.contains(r#""outcome":{"result":"rejected","reason":"unknown_fleet"}"#));
    assert!(line.contains(r#""payload":{"player":340282366920938463463374607431768211455,"#));
}

#[test]
fn a_line_that_is_not_a_valid_record_is_refused() {
    let good = advance_record().to_line().unwrap();
    for (bad, why) in [
        (good.replace("\"advance\"", "\"tick\""), "kind"),
        (
            good.replace("\"engine_build\":\"b\"", "\"engine_build\":\"\""),
            "build",
        ),
        (
            good.replace("\"idempotency_key\":null", "\"idempotency_key\":\"a b\""),
            "key",
        ),
        (
            good.replace("\"player_id\":null", "\"player_id\":\"not-a-uuid\""),
            "player",
        ),
        (
            good.replace("\"seq\":1,", "\"seq\":1,\"extra\":0,"),
            "unknown field",
        ),
        (good.replace("}", ""), "truncated"),
    ] {
        assert!(
            matches!(LogRecord::from_line(&bad), Err(LogError::Corrupt { .. })),
            "{why}: {bad}"
        );
    }
}
