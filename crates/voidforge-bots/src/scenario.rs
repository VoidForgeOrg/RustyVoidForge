//! The end-to-end scenario (Technical Plan P3.6; decision log D-052): alice and bob log in
//! through dex, join, build a Shipyard, build ships, race their Colony Ships to one free planet,
//! and the winner carries cargo between its two planets. Every assertion is on a view read back
//! over the API, never on a command's 200 alone, and each claim that could pass with nothing in
//! place is preceded by a check that the case is really there.
//!
//! The bots never read the rules file: every wait's deadline comes from the views' own `eta`,
//! `arrives_at` and `valid_until` ([`crate::wait`]).

use std::time::Duration;

use tokio::time::Instant;
use tracing::info;
use voidforge_api_types::command::{
    AssembleFleet, BuildBuilding, CommandRequest, QueueShip, SendFleet,
};
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::view::{BuildingView, FleetView, FleetsView, PlanetView, PlayerView};
use voidforge_api_types::wire::{
    BuildingId, BuildingType, Cargo, FleetId, Mission, PlanetId, PlayerId, Position, ShipId,
    ShipType, Timestamp,
};

use crate::Failure;
use crate::client::{Player, Refused, Reply};
use crate::login::{self, ALICE, BOB};
use crate::wait::{self, Poll, between, deadline_after, unix_ms};

/// How long the server may take to accept tokens (dex's keys load in the background, D-050).
const READY_WITHIN: Duration = Duration::from_secs(30);

/// The board is rebuilt every second in the e2e run (`--leaderboard-secs 1`), so a new one is due
/// well within this.
const BOARD_WITHIN: Duration = Duration::from_secs(10);

/// Ships queued behind one another, or throttled by ingots, have no fixed `eta` to wait for: a
/// waiting build has none, and a throttled one's moves. Measured: 19 s for both of alice's ships.
const SHIPS_WITHIN: Duration = Duration::from_secs(90);

/// How long the winner's homeworld may take to refine the cargo's ingots, if it's short.
const INGOTS_WITHIN: Duration = Duration::from_secs(60);

/// The least gap between the two colony fleets' arrivals: never a same-millisecond tie, whose
/// tie-break the core's tests own (the advisor's ruling).
const RACE_GAP_MS: i64 = 2_000;

/// The predicted gap a target needs, above [`RACE_GAP_MS`] for the rounding of the prediction
/// and the time between the two sends.
const PREDICTED_GAP_MS: i64 = 3_000;

/// The cargo the winner carries to its colony: ingots, within the Cargo Vessel's 500,000,000 g.
const CARGO_INGOTS_G: i64 = 400_000_000;

/// A fleet ID no one has: entity IDs are a small engine counter (D-045).
const NO_SUCH_FLEET: FleetId = FleetId(1 << 52);

/// Runs the scenario against the API at `api`, logging in through dex at `issuer`. `run` makes
/// the idempotency keys unique to this run.
///
/// # Errors
/// The first failed check, missed deadline or transport error.
pub async fn run(
    http: &reqwest::Client,
    api: &str,
    issuer: &str,
    run: &str,
) -> Result<(), Failure> {
    let keys = Keys(run.to_owned());

    // 1. Login.
    let step = "login";
    let alice = Player::new(
        ALICE.name,
        http.clone(),
        api,
        login::password(http, issuer, ALICE).await?,
    );
    let bob = Player::new(
        BOB.name,
        http.clone(),
        api,
        login::password(http, issuer, BOB).await?,
    );
    info!(
        step,
        "alice and bob have ID tokens from dex's password grant ({})",
        login::CLIENT_ID
    );

    // 2. Join.
    let a = join(&alice).await?;
    let b = join(&bob).await?;
    let step = "join";
    ensure(a.player_id != b.player_id, step, || {
        format!("alice and bob are one player: {:?}", a.player_id)
    })?;
    ensure(a.homeworld_id != b.homeworld_id, step, || {
        format!("alice and bob share homeworld {:?}", a.homeworld_id)
    })?;
    let joined = a.as_of.clone().max(b.as_of.clone());
    let board = board_after(&alice, &joined, "join: the leaderboard").await?;
    for (name, player) in [("alice", &a.player_id), ("bob", &b.player_id)] {
        ensure(board.iter().any(|(_, id, _)| id == player), step, || {
            format!("the leaderboard doesn't list {name}: {board:?}")
        })?;
    }
    info!(
        step,
        "both joined once, each owns its homeworld, and the leaderboard lists both"
    );

    // 3. Build: a Shipyard each, and alice's command repeated under its key.
    let step = "build";
    let shipyard = |planet_id| {
        CommandRequest::BuildBuilding(BuildBuilding {
            planet_id,
            building_type: BuildingType::Shipyard,
        })
    };
    let key = keys.of("alice", "build-shipyard");
    let first = accepted(alice.command(&shipyard(a.homeworld_id), &key).await?, step)?;
    let again = accepted(alice.command(&shipyard(a.homeworld_id), &key).await?, step)?;
    ensure(again == first, step, || {
        format!("a repeated key answered {again:?}, not the first answer {first:?}")
    })?;
    let drill = CommandRequest::BuildBuilding(BuildBuilding {
        planet_id: a.homeworld_id,
        building_type: BuildingType::Drill,
    });
    let conflict = alice.command(&drill, &key).await?;
    ensure(
        matches!(&conflict, Err(r) if r.status == 422 && r.body.code == ErrorCode::IdempotencyKeyConflict),
        step,
        || {
            format!(
                "the key reused for another command answered {conflict:?}, not 422 idempotency_key_conflict"
            )
        },
    )?;
    accepted(
        bob.command(&shipyard(b.homeworld_id), &keys.of("bob", "build-shipyard"))
            .await?,
        step,
    )?;
    // By effect: the repeat built nothing more.
    let home = view(alice.planet(a.homeworld_id).await?, step)?;
    let yards = home
        .buildings
        .iter()
        .filter(|b| kind(b) == BuildingType::Shipyard)
        .count();
    ensure(yards == 1, step, || {
        format!(
            "alice's homeworld has {yards} Shipyards after one command sent twice: {:?}",
            home.buildings
        )
    })?;
    let yard_a = operational_shipyard(&alice, a.homeworld_id).await?;
    let yard_b = operational_shipyard(&bob, b.homeworld_id).await?;
    info!(
        step,
        "both Shipyards are operational; the repeated key returned the first answer and built nothing more"
    );

    // 4. Ships: a Colony Ship each, and a Cargo Vessel for alice.
    let step = "ships";
    let queue = |shipyard_id, ship_type| {
        CommandRequest::QueueShip(QueueShip {
            shipyard_id,
            ship_type,
        })
    };
    accepted(
        alice
            .command(
                &queue(yard_a, ShipType::ColonyShip),
                &keys.of("alice", "queue-colony-ship"),
            )
            .await?,
        step,
    )?;
    accepted(
        alice
            .command(
                &queue(yard_a, ShipType::CargoVessel),
                &keys.of("alice", "queue-cargo-vessel"),
            )
            .await?,
        step,
    )?;
    accepted(
        bob.command(
            &queue(yard_b, ShipType::ColonyShip),
            &keys.of("bob", "queue-colony-ship"),
        )
        .await?,
        step,
    )?;
    let ships_a = ships(
        &alice,
        a.homeworld_id,
        &[ShipType::ColonyShip, ShipType::CargoVessel],
    )
    .await?;
    let ships_b = ships(&bob, b.homeworld_id, &[ShipType::ColonyShip]).await?;
    info!(step, "the ships are on the rosters");

    // 5. The race.
    let step = "race";
    let home_a = view(alice.planet(a.homeworld_id).await?, step)?.position;
    let home_b = view(bob.planet(b.homeworld_id).await?, step)?.position;
    let fleet_a = assemble(
        &alice,
        &keys,
        a.homeworld_id,
        ships_a[0],
        Cargo::default(),
        "colony",
    )
    .await?;
    let fleet_b = assemble(
        &bob,
        &keys,
        b.homeworld_id,
        ships_b[0],
        Cargo::default(),
        "colony",
    )
    .await?;
    let target = race_target(&alice, home_a, home_b, &fleet_a, &fleet_b).await?;
    let send = |fleet_id, mission| {
        CommandRequest::SendFleet(SendFleet {
            fleet_id,
            target_planet_id: target,
            mission,
        })
    };
    // Back to back: the race is decided by the arrival times the fleet views report.
    accepted(
        alice
            .command(
                &send(fleet_a.id, Mission::Colonize),
                &keys.of("alice", "send-colony"),
            )
            .await?,
        step,
    )?;
    accepted(
        bob.command(
            &send(fleet_b.id, Mission::Colonize),
            &keys.of("bob", "send-colony"),
        )
        .await?,
        step,
    )?;
    let arrives_a = in_transit_to(&alice, fleet_a.id, target).await?;
    let arrives_b = in_transit_to(&bob, fleet_b.id, target).await?;
    let unowned = view(alice.planet(target).await?, step)?.owner;
    ensure(unowned.is_none(), step, || {
        format!("planet {target:?} was owned before either fleet arrived: {unowned:?}")
    })?;
    let gap = unix_ms(&arrives_b) - unix_ms(&arrives_a);
    ensure(gap.abs() >= RACE_GAP_MS, step, || {
        format!(
            "the arrivals are {gap} ms apart, under {RACE_GAP_MS} ms: alice {arrives_a:?}, bob {arrives_b:?}"
        )
    })?;
    info!(
        step,
        "both colony fleets are in transit to planet {}: alice arrives {}, bob {}",
        target.0,
        arrives_a.as_str(),
        arrives_b.as_str()
    );

    // 7. Privacy, while alice's colony fleet is in flight (D-030; D-049 S1).
    let step = "privacy";
    let alices = view(alice.fleets().await?, step)?;
    ensure(
        alices.fleets.iter().any(|f| id_of(f) == fleet_a.id),
        step,
        || {
            format!(
                "alice's own fleets don't list her colony fleet {:?}: {alices:?}",
                fleet_a.id
            )
        },
    )?;
    let bobs = view(bob.fleets().await?, step)?;
    for f in &bobs.fleets {
        ensure(
            !alices.fleets.iter().any(|g| id_of(g) == id_of(f)),
            step,
            || format!("bob's fleets show alice's fleet {:?}: {bobs:?}", id_of(f)),
        )?;
    }
    let move_to_bob = |fleet_id| {
        CommandRequest::SendFleet(SendFleet {
            fleet_id,
            target_planet_id: b.homeworld_id,
            mission: Mission::Move,
        })
    };
    let foreign = refused(
        bob.command(
            &move_to_bob(fleet_a.id),
            &keys.of("bob", "send-alices-fleet"),
        )
        .await?,
        step,
    )?;
    let unknown = refused(
        bob.command(
            &move_to_bob(NO_SUCH_FLEET),
            &keys.of("bob", "send-no-fleet"),
        )
        .await?,
        step,
    )?;
    ensure(unknown.body.code == ErrorCode::UnknownFleet, step, || {
        format!("a fleet no one has answered {unknown:?}, not unknown_fleet")
    })?;
    ensure(foreign == unknown, step, || {
        format!(
            "alice's fleet, sent by bob, answered {foreign:?}; a fleet no one has answered {unknown:?}"
        )
    })?;
    info!(
        step,
        "bob sees none of alice's fleets, and her fleet answers him exactly as a fleet no one has"
    );

    // 5, continued: the arrivals.
    let step = "race";
    let later = arrives_a.clone().max(arrives_b.clone());
    let deadline = deadline_after(between(&alices.as_of, &later));
    wait::until("race: the arrivals", deadline, || async {
        let fa = view(alice.fleets().await?, step)?;
        let fb = view(bob.fleets().await?, step)?;
        let flying = |v: &FleetsView| {
            v.fleets
                .iter()
                .any(|f| matches!(f, FleetView::InTransit { target_planet_id, .. } if *target_planet_id == target))
        };
        Ok(if flying(&fa) || flying(&fb) {
            Poll::Pending {
                next: next_of(&fa),
                last: format!("alice {fa:?}; bob {fb:?}"),
            }
        } else {
            Poll::Ready(())
        })
    })
    .await?;
    let (winner, loser, won, lost) = if arrives_a < arrives_b {
        (&alice, &bob, (&a, &fleet_a), (&b, &fleet_b))
    } else {
        (&bob, &alice, (&b, &fleet_b), (&a, &fleet_a))
    };
    let owner = view(winner.planet(target).await?, step)?.owner;
    ensure(owner.as_ref() == Some(&won.0.player_id), step, || {
        format!(
            "planet {target:?} is owned by {owner:?}, not the earlier arrival {} ({:?})",
            winner.name, won.0.player_id
        )
    })?;
    let mine = view(winner.me().await?, step)?;
    ensure(mine.planet_ids.contains(&target), step, || {
        format!(
            "{}'s planets don't include its colony {target:?}: {:?}",
            winner.name, mine.planet_ids
        )
    })?;
    let theirs = view(loser.me().await?, step)?;
    ensure(!theirs.planet_ids.contains(&target), step, || {
        format!(
            "{}'s planets include the planet it lost: {:?}",
            loser.name, theirs.planet_ids
        )
    })?;
    // The winner's colony fleet carried only its Colony Ship, so colonizing removed it.
    let after = view(winner.fleets().await?, step)?;
    ensure(
        !after.fleets.iter().any(|f| id_of(f) == won.1.id),
        step,
        || {
            format!(
                "{}'s colony fleet, its only ship used up, is still listed: {after:?}",
                winner.name
            )
        },
    )?;
    // The loser's failed: nothing consumed, the Colony Ship still aboard, stationed at the target.
    let left = view(loser.fleets().await?, step)?;
    let stayed = left.fleets.iter().find(|f| id_of(f) == lost.1.id);
    ensure(
        matches!(stayed, Some(FleetView::Stationed { at_planet_id, ships, cargo, .. })
            if *at_planet_id == target
                && ships.iter().map(|s| s.ship_id).eq([lost.1.ship])
                && *cargo == Cargo::default()),
        step,
        || {
            format!(
                "{}'s failed colony fleet isn't stationed at {target:?} with its Colony Ship: {stayed:?}",
                loser.name
            )
        },
    )?;
    info!(
        step,
        "{} colonized planet {}; {}'s colonization failed, its Colony Ship still aboard",
        winner.name,
        target.0,
        loser.name
    );

    // 6. Transport: the winner carries ingots from its homeworld to its colony.
    let step = "transport";
    let home = won.0.homeworld_id;
    let vessel = if std::ptr::eq(winner, &alice) {
        ships_a[1]
    } else {
        // bob has no Cargo Vessel yet: queue one.
        accepted(
            bob.command(
                &queue(yard_b, ShipType::CargoVessel),
                &keys.of("bob", "queue-cargo-vessel"),
            )
            .await?,
            step,
        )?;
        ships(&bob, home, &[ShipType::CargoVessel]).await?[0]
    };
    ingots_at_least(winner, home, CARGO_INGOTS_G).await?;
    let colony = view(winner.planet(target).await?, step)?.stocks.iron_ingot;
    ensure(colony.rate_g_per_h == 0, step, || {
        format!(
            "the colony's ingots change by themselves ({colony:?}), so a delivery can't be measured exactly"
        )
    })?;
    let load = Cargo {
        iron_ore_g: 0,
        iron_ingot_g: CARGO_INGOTS_G,
    };
    let fleet = assemble(winner, &keys, home, vessel, load, "cargo").await?;
    ensure(fleet.cargo == load, step, || {
        format!("the cargo fleet carries {:?}, not {load:?}", fleet.cargo)
    })?;
    accepted(
        winner
            .command(
                &send(fleet.id, Mission::Transport),
                &keys.of(winner.name, "send-cargo"),
            )
            .await?,
        step,
    )?;
    let arrives = in_transit_to(winner, fleet.id, target).await?;
    let as_of = view(winner.fleets().await?, step)?.as_of;
    let landed = wait::until(
        "transport: the arrival",
        deadline_after(between(&as_of, &arrives)),
        || async {
            let v = view(winner.fleets().await?, step)?;
            let f = v.fleets.iter().find(|f| id_of(f) == fleet.id).cloned();
            Ok(match f {
                Some(FleetView::Stationed {
                    at_planet_id,
                    cargo,
                    ..
                }) if at_planet_id == target => Poll::Ready(cargo),
                other => Poll::Pending {
                    next: next_of(&v),
                    last: format!("{other:?}"),
                },
            })
        },
    )
    .await?;
    ensure(landed == Cargo::default(), step, || {
        format!("the cargo fleet still carries {landed:?} at the colony")
    })?;
    let delivered = view(winner.planet(target).await?, step)?.stocks.iron_ingot;
    ensure(
        delivered.amount_g == colony.amount_g + CARGO_INGOTS_G,
        step,
        || {
            format!(
                "the colony holds {} g of ingots, not {} + {CARGO_INGOTS_G}",
                delivered.amount_g, colony.amount_g
            )
        },
    )?;
    info!(
        step,
        "{} delivered {CARGO_INGOTS_G} g of ingots to planet {}", winner.name, target.0
    );

    // 8. The board at the end.
    let step = "leaderboard";
    let board = board_after(&alice, &arrives, "leaderboard: after the transport").await?;
    for (rank, id, score) in &board {
        let ahead = board.iter().filter(|(_, _, s)| s > score).count();
        ensure(
            usize::try_from(*rank).is_ok_and(|r| r == ahead + 1),
            step,
            || format!("{id:?} is ranked {rank} with {ahead} players ahead: {board:?}"),
        )?;
    }
    // The winner leads: a planet is 1,000 points, while the loser keeps a Colony Ship (400) and
    // a few hundred points of stock.
    ensure(
        board
            .first()
            .is_some_and(|(_, id, _)| *id == won.0.player_id),
        step,
        || {
            format!(
                "the leaderboard doesn't rank {} first: {board:?}",
                winner.name
            )
        },
    )?;
    info!(step, "the board ranks both, {} first", winner.name);
    Ok(())
}

/// Idempotency keys unique to a run, readable in the log: `e2e-<run>-<player>-<step>`.
struct Keys(String);

impl Keys {
    fn of(&self, player: &str, step: &str) -> String {
        format!("e2e-{}-{player}-{step}", self.0)
    }
}

/// A fleet the scenario assembled.
struct Assembled {
    id: FleetId,
    ship: ShipId,
    speed_du_per_h: i64,
    cargo: Cargo,
}

/// Fails `step` with `detail` unless `holds`.
fn ensure(holds: bool, step: &str, detail: impl FnOnce() -> String) -> Result<(), Failure> {
    if holds {
        Ok(())
    } else {
        Err(Failure::Assertion {
            step: step.to_owned(),
            detail: detail(),
        })
    }
}

/// A view the scenario needs: a refusal fails `step`.
fn view<T>(reply: Reply<T>, step: &str) -> Result<T, Failure> {
    reply.map_err(|r| Failure::Assertion {
        step: step.to_owned(),
        detail: format!(
            "refused: HTTP {} {:?}: {}",
            r.status, r.body.code, r.body.message
        ),
    })
}

/// A command the scenario needs accepted.
fn accepted<T>(reply: Reply<T>, step: &str) -> Result<T, Failure> {
    view(reply, step)
}

/// A command the scenario needs refused.
fn refused<T: std::fmt::Debug>(reply: Reply<T>, step: &str) -> Result<Refused, Failure> {
    match reply {
        Err(r) => Ok(r),
        Ok(accepted) => Err(Failure::Assertion {
            step: step.to_owned(),
            detail: format!("accepted, where a refusal was due: {accepted:?}"),
        }),
    }
}

fn kind(building: &BuildingView) -> BuildingType {
    match building {
        BuildingView::UnderConstruction { building_type, .. }
        | BuildingView::Operational { building_type, .. }
        | BuildingView::Demolishing { building_type, .. } => *building_type,
    }
}

fn id_of(fleet: &FleetView) -> FleetId {
    match fleet {
        FleetView::Stationed { fleet_id, .. }
        | FleetView::InTransit { fleet_id, .. }
        | FleetView::Returning { fleet_id, .. } => *fleet_id,
    }
}

fn next_of(view: &FleetsView) -> Option<(Timestamp, Timestamp)> {
    view.valid_until.clone().map(|v| (view.as_of.clone(), v))
}

fn next_of_planet(view: &PlanetView) -> Option<(Timestamp, Timestamp)> {
    view.valid_until.clone().map(|v| (view.as_of.clone(), v))
}

/// The first call joins (D-047). Until dex's keys load the server answers 503 `unavailable`,
/// which means not ready yet. Then: the homeworld is in the player's planets and owned by it,
/// and a second call is the same player.
async fn join(player: &Player) -> Result<PlayerView, Failure> {
    let step = "join";
    let deadline = Instant::now() + READY_WITHIN;
    let me = wait::until("join: the server accepts tokens", deadline, || async {
        Ok(match player.me().await {
            Err(Failure::NotReady(last)) => Poll::Pending { next: None, last },
            other => Poll::Ready(other?),
        })
    })
    .await?;
    let me = view(me, step)?;
    ensure(me.planet_ids.contains(&me.homeworld_id), step, || {
        format!(
            "{}'s planets don't include its homeworld: {me:?}",
            player.name
        )
    })?;
    let home = view(player.planet(me.homeworld_id).await?, step)?;
    ensure(home.owner.as_ref() == Some(&me.player_id), step, || {
        format!(
            "{}'s homeworld is owned by {:?}, not {:?}",
            player.name, home.owner, me.player_id
        )
    })?;
    let again = view(player.me().await?, step)?;
    ensure(
        again.player_id == me.player_id && again.homeworld_id == me.homeworld_id,
        step,
        || {
            format!(
                "{}'s second call is another player: {again:?}, first {me:?}",
                player.name
            )
        },
    )?;
    info!(
        step,
        "{} joined as {} with homeworld {}",
        player.name,
        me.player_id.as_str(),
        me.homeworld_id.0
    );
    Ok(me)
}

/// The leaderboard once a build is at or after `after`, as (rank, player, score) in its order.
async fn board_after(
    player: &Player,
    after: &Timestamp,
    step: &str,
) -> Result<Vec<(u32, PlayerId, i64)>, Failure> {
    wait::until(step, Instant::now() + BOARD_WITHIN, || async {
        Ok(match player.leaderboard().await {
            // Before the first build, the board is 503 unavailable (D-051).
            Err(Failure::NotReady(last)) => Poll::Pending { next: None, last },
            reply => {
                let board = view(reply?, step)?;
                if board.as_of >= *after {
                    Poll::Ready(
                        board
                            .entries
                            .into_iter()
                            .map(|e| (e.rank, e.player_id, e.score))
                            .collect(),
                    )
                } else {
                    Poll::Pending {
                        next: None,
                        last: format!("{board:?}"),
                    }
                }
            }
        })
    })
    .await
}

/// Waits until the homeworld's Shipyard is operational, by its `eta`.
async fn operational_shipyard(player: &Player, planet: PlanetId) -> Result<BuildingId, Failure> {
    let step = "build";
    let first = view(player.planet(planet).await?, step)?;
    let left = first
        .buildings
        .iter()
        .find_map(|b| match b {
            BuildingView::UnderConstruction {
                building_type: BuildingType::Shipyard,
                eta: Some(eta),
                ..
            } => Some(between(&first.as_of, eta)),
            _ => None,
        })
        .unwrap_or_default();
    wait::until(
        "build: the Shipyard completes",
        deadline_after(left),
        || async {
            let v = view(player.planet(planet).await?, step)?;
            let done = v.buildings.iter().find_map(|b| match b {
                BuildingView::Operational {
                    id,
                    building_type: BuildingType::Shipyard,
                    ..
                } => Some(*id),
                _ => None,
            });
            Ok(match done {
                Some(id) => Poll::Ready(id),
                None => Poll::Pending {
                    next: next_of_planet(&v),
                    last: format!("{}: {:?}", player.name, v.buildings),
                },
            })
        },
    )
    .await
}

/// Waits until the planet's roster has one ship of each of `types`, returning their IDs in that
/// order.
async fn ships(
    player: &Player,
    planet: PlanetId,
    types: &[ShipType],
) -> Result<Vec<ShipId>, Failure> {
    let step = "ships";
    wait::until(
        "ships: the ships complete",
        Instant::now() + SHIPS_WITHIN,
        || async {
            let v = view(player.planet(planet).await?, step)?;
            let found: Option<Vec<ShipId>> = types
                .iter()
                .map(|t| {
                    v.roster
                        .iter()
                        .find(|s| s.ship_type == *t)
                        .map(|s| s.ship_id)
                })
                .collect();
            Ok(match found {
                Some(ids) => Poll::Ready(ids),
                None => Poll::Pending {
                    next: next_of_planet(&v),
                    last: format!(
                        "{}: roster {:?}, builds {:?}",
                        player.name, v.roster, v.ship_builds
                    ),
                },
            })
        },
    )
    .await
}

/// Waits until the planet holds at least `grams` of ingots.
async fn ingots_at_least(player: &Player, planet: PlanetId, grams: i64) -> Result<(), Failure> {
    let step = "transport";
    wait::until(
        "transport: ingots for the cargo",
        Instant::now() + INGOTS_WITHIN,
        || async {
            let v = view(player.planet(planet).await?, step)?;
            Ok(if v.stocks.iron_ingot.amount_g >= grams {
                Poll::Ready(())
            } else {
                Poll::Pending {
                    next: None,
                    last: format!("{}: {:?}", player.name, v.stocks.iron_ingot),
                }
            })
        },
    )
    .await
}

/// Assembles a fleet of one ship with `cargo`, and finds it stationed in the player's fleets.
async fn assemble(
    player: &Player,
    keys: &Keys,
    planet_id: PlanetId,
    ship: ShipId,
    cargo: Cargo,
    what: &str,
) -> Result<Assembled, Failure> {
    let step = "assemble";
    let command = CommandRequest::AssembleFleet(AssembleFleet {
        planet_id,
        ship_ids: vec![ship],
        cargo,
    });
    accepted(
        player
            .command(&command, &keys.of(player.name, &format!("assemble-{what}")))
            .await?,
        step,
    )?;
    let v = view(player.fleets().await?, step)?;
    v.fleets
        .iter()
        .find_map(|f| match f {
            FleetView::Stationed {
                fleet_id,
                ships,
                cargo,
                speed_du_per_h,
                at_planet_id,
                ..
            } if *at_planet_id == planet_id && ships.iter().any(|s| s.ship_id == ship) => {
                Some(Assembled {
                    id: *fleet_id,
                    ship,
                    speed_du_per_h: *speed_du_per_h,
                    cargo: *cargo,
                })
            }
            _ => None,
        })
        .ok_or_else(|| Failure::Assertion {
            step: step.to_owned(),
            detail: format!(
                "{}'s {what} fleet with ship {ship:?} isn't stationed at {planet_id:?}: {v:?}",
                player.name
            ),
        })
}

/// The fleet's `arrives_at`, checking it's in transit to `target`.
async fn in_transit_to(
    player: &Player,
    fleet: FleetId,
    target: PlanetId,
) -> Result<Timestamp, Failure> {
    let step = "send";
    let v = view(player.fleets().await?, step)?;
    match v.fleets.iter().find(|f| id_of(f) == fleet) {
        Some(FleetView::InTransit {
            target_planet_id,
            arrives_at,
            ..
        }) if *target_planet_id == target => Ok(arrives_at.clone()),
        other => Err(Failure::Assertion {
            step: step.to_owned(),
            detail: format!(
                "{}'s fleet {fleet:?} isn't in transit to {target:?}: {other:?}",
                player.name
            ),
        }),
    }
}

/// A free planet for the race: of the unowned planets around the homeworlds' midpoint, the one
/// whose slower trip is shortest, among those whose trips differ by at least
/// [`PREDICTED_GAP_MS`].
async fn race_target(
    player: &Player,
    home_a: Position,
    home_b: Position,
    fleet_a: &Assembled,
    fleet_b: &Assembled,
) -> Result<PlanetId, Failure> {
    let step = "race";
    let mid = Position {
        x: home_a.x / 2 + home_b.x / 2,
        y: home_a.y / 2 + home_b.y / 2,
        z: home_a.z / 2 + home_b.z / 2,
    };
    // Every planet nearer the midpoint than the homeworlds are far apart.
    let radius = distance(home_a, home_b).ceil() as i64;
    let near = view(player.near(mid, radius).await?, step)?;
    let trip =
        |from: Position, to: Position, speed: i64| distance(from, to) * 3_600_000.0 / speed as f64;
    near.planets
        .iter()
        .filter(|p| p.owner.is_none())
        .map(|p| {
            let ta = trip(home_a, p.position, fleet_a.speed_du_per_h);
            let tb = trip(home_b, p.position, fleet_b.speed_du_per_h);
            (p.planet_id, ta, tb)
        })
        .filter(|(_, ta, tb)| (ta - tb).abs() >= PREDICTED_GAP_MS as f64)
        .min_by(|x, y| x.1.max(x.2).total_cmp(&y.1.max(y.2)))
        .map(|(id, ta, tb)| {
            info!(
                step,
                "the target is planet {}: about {:.1} s from alice, {:.1} s from bob",
                id.0,
                ta / 1000.0,
                tb / 1000.0
            );
            id
        })
        .ok_or_else(|| Failure::Assertion {
            step: step.to_owned(),
            detail: format!(
                "no free planet within {radius} du has trips {PREDICTED_GAP_MS} ms apart: {near:?}"
            ),
        })
}

/// The straight-line distance, in distance units.
fn distance(a: Position, b: Position) -> f64 {
    let d = |p: i64, q: i64| (p as f64 - q as f64).powi(2);
    (d(a.x, b.x) + d(a.y, b.y) + d(a.z, b.z)).sqrt()
}
