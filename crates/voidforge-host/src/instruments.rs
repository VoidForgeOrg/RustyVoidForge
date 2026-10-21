//! The host's metrics and its command spans' names (P5.1, D-064; Architectural Design,
//! "Operations and scaling"). Measured here, in the shell: the core stays free of clocks and I/O.
//!
//! | Metric | Type, unit | What |
//! |---|---|---|
//! | `voidforge.host.inbox.depth` | gauge, `{command}` | commands submitted and not yet taken by the host |
//! | `voidforge.host.event.lag` | gauge, game-time `ms` | now minus the oldest queue entry once it's due, else 0 |
//! | `voidforge.host.step.duration` | histogram, `s` | one step: `kind` = `command` or `advance` |
//! | `voidforge.host.append.duration` | histogram, `s` | one durable write (a turn's records) |
//! | `voidforge.host.snapshot.age` | gauge, game-time `s` | now minus the newest snapshot's game time |
//!
//! Event lag and snapshot age are in game time (wall − epoch, the host's own clock), which is
//! wall time in the MVP: the game clock never pauses. If pausing game time during maintenance
//! ever lands (Game Design, post-MVP), revisit both (A2).
//!
//! The gauges are read on the exporter's schedule from atomics the host keeps current, against
//! the host's clock, so a stuck host shows a growing lag and an ageing snapshot.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use voidforge_core::command::Action;
use voidforge_telemetry::KeyValue;
use voidforge_telemetry::metrics::{Histogram, ObservableGauge};

use crate::clock::Clock;

/// No value yet: no due entry, or no snapshot known.
pub(crate) const NONE: i64 = i64::MIN;

/// What the gauges read, shared by the host and its handles.
#[derive(Debug)]
pub(crate) struct Gauges {
    /// Submitted and not yet taken.
    pub(crate) depth: AtomicI64,
    /// The oldest queue entry's game time, or [`NONE`].
    pub(crate) oldest_due: AtomicI64,
    /// The newest snapshot's game time, or [`NONE`].
    pub(crate) snapshot_at: AtomicI64,
}

impl Default for Gauges {
    fn default() -> Self {
        Self {
            depth: AtomicI64::new(0),
            oldest_due: AtomicI64::new(NONE),
            snapshot_at: AtomicI64::new(NONE),
        }
    }
}

/// Event lag at game time `now`: how long the oldest due entry has waited, 0 when none is due.
pub(crate) fn lag_ms(now: i64, oldest_due: i64) -> i64 {
    if oldest_due == NONE {
        return 0;
    }
    now.saturating_sub(oldest_due).max(0)
}

/// Snapshot age at game time `now`, in seconds; None before any snapshot is known.
#[allow(clippy::cast_precision_loss)] // ages far below 2^52 ms
pub(crate) fn snapshot_age_s(now: i64, snapshot_at: i64) -> Option<f64> {
    (snapshot_at != NONE).then(|| now.saturating_sub(snapshot_at).max(0) as f64 / 1000.0)
}

/// The host's instruments. The observable gauges stay registered while this lives.
pub(crate) struct Instruments {
    step: Histogram<f64>,
    append: Histogram<f64>,
    _gauges: [ObservableGauge<i64>; 2],
    _age: ObservableGauge<f64>,
}

impl Instruments {
    pub(crate) fn new<C: Clock>(gauges: &Arc<Gauges>, clock: &Arc<C>, epoch_unix_ms: i64) -> Self {
        let meter = voidforge_telemetry::meter("voidforge-host");
        let now = {
            let clock = clock.clone();
            move || clock.now_unix_ms().saturating_sub(epoch_unix_ms)
        };
        let depth = {
            let g = gauges.clone();
            meter
                .i64_observable_gauge("voidforge.host.inbox.depth")
                .with_unit("{command}")
                .with_description("Commands submitted to the host and not yet taken")
                .with_callback(move |o| o.observe(g.depth.load(Ordering::Relaxed).max(0), &[]))
                .build()
        };
        let lag = {
            let (g, now) = (gauges.clone(), now.clone());
            meter
                .i64_observable_gauge("voidforge.host.event.lag")
                .with_unit("ms")
                .with_description(
                    "Game-time ms since the oldest scheduled event came due, 0 when none is due",
                )
                .with_callback(move |o| {
                    o.observe(lag_ms(now(), g.oldest_due.load(Ordering::Relaxed)), &[])
                })
                .build()
        };
        let age = {
            let g = gauges.clone();
            meter
                .f64_observable_gauge("voidforge.host.snapshot.age")
                .with_unit("s")
                .with_description("Game-time seconds since the newest snapshot's game time")
                .with_callback(move |o| {
                    if let Some(age) = snapshot_age_s(now(), g.snapshot_at.load(Ordering::Relaxed))
                    {
                        o.observe(age, &[]);
                    }
                })
                .build()
        };
        Self {
            step: meter
                .f64_histogram("voidforge.host.step.duration")
                .with_unit("s")
                .with_description("One engine step: a command applied, or time advanced")
                .build(),
            append: meter
                .f64_histogram("voidforge.host.append.duration")
                .with_unit("s")
                .with_description("One durable write of a turn's records")
                .build(),
            _gauges: [depth, lag],
            _age: age,
        }
    }

    pub(crate) fn step(&self, took: Duration, command: bool) {
        let kind = if command { "command" } else { "advance" };
        self.step
            .record(took.as_secs_f64(), &[KeyValue::new("kind", kind)]);
    }

    pub(crate) fn append(&self, took: Duration) {
        self.append.record(took.as_secs_f64(), &[]);
    }
}

/// A command's type, as the API names it (its wire tag), for the command spans.
pub(crate) fn action_type(action: &Action) -> &'static str {
    match action {
        Action::JoinGame => "join_game",
        Action::BuildBuilding { .. } => "build_building",
        Action::CancelConstruction { .. } => "cancel_construction",
        Action::DemolishBuilding { .. } => "demolish_building",
        Action::QueueShip { .. } => "queue_ship",
        Action::CancelShipBuild { .. } => "cancel_ship_build",
        Action::AssembleFleet { .. } => "assemble_fleet",
        Action::SendFleet { .. } => "send_fleet",
        Action::RecallFleet { .. } => "recall_fleet",
        Action::DisbandFleet { .. } => "disband_fleet",
        Action::TriggerUnload { .. } => "trigger_unload",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lag_is_zero_until_due_and_counts_from_the_oldest_due_entry() {
        assert_eq!(lag_ms(1_000, NONE), 0);
        assert_eq!(lag_ms(1_000, 2_000), 0, "not yet due");
        assert_eq!(lag_ms(1_000, 1_000), 0);
        assert_eq!(lag_ms(1_500, 1_000), 500);
        assert_eq!(lag_ms(i64::MAX, i64::MIN + 1), i64::MAX, "saturates");
    }

    #[test]
    fn snapshot_age_is_seconds_of_game_time() {
        assert_eq!(snapshot_age_s(5_000, NONE), None);
        assert_eq!(snapshot_age_s(5_000, 2_000), Some(3.0));
        assert_eq!(snapshot_age_s(1_000, 2_000), Some(0.0), "never negative");
    }

    #[test]
    fn every_action_has_its_wire_tag() {
        // The tag is serde's: the same name the API and the log use.
        use voidforge_core::command::Action;
        let join = serde_json::to_value(Action::JoinGame).expect("serializes");
        assert_eq!(join, serde_json::json!(action_type(&Action::JoinGame)));
    }
}
