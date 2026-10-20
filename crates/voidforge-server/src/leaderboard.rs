//! The leaderboard (Technical Plan P3.5; Game Design, Scoring; decision log D-045). Every joined
//! player, ranked by score, built from one published read model at one time and served with that
//! time as its `as_of`.
//!
//! A player's score on the board is computed by the route's own path: `view_for` of
//! `Query::Player` at the build's `t`, the call `GET /players/me` makes. So the board and a
//! player's own view can't disagree for the same time: they're one function on one World.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use voidforge_api_types::view as api;
use voidforge_host::view::{Query, View, ViewError};
use voidforge_host::{HostHandle, PlayerId, ReadModel};

use crate::api::{STALE_WAIT, time, wire};

/// How often the board is rebuilt by default (Game Design: "refreshes every 60 seconds").
pub const DEFAULT_EVERY: Duration = Duration::from_secs(60);

/// How soon a failed build is retried while no board has been built yet.
pub const RETRY_UNTIL_FIRST: Duration = Duration::from_secs(1);

/// A built board, as the route serves it: its game time, and its JSON, serialized once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// The game time it was built at, the `as_of`.
    pub t: i64,
    /// `api::Leaderboard` as JSON.
    pub json: Vec<u8>,
}

/// The latest board: `None` until a build succeeds.
pub type Board = watch::Receiver<Option<Arc<Built>>>;

/// Where the job publishes.
pub type Publish = watch::Sender<Option<Arc<Built>>>;

/// An empty board channel: the route answers 503 until something is published.
#[must_use]
pub fn channel() -> (Publish, Board) {
    watch::channel(None)
}

/// Builds the board now, as the routes project: `t = max(clock − epoch, world time)` on the
/// latest published read model. A read model behind a due event waits up to [`STALE_WAIT`] for the
/// host's next publish, then fails as [`BuildError::Stale`].
///
/// # Errors
/// [`BuildError`]; also [`BuildError::Stale`] if the host has stopped while the model was stale.
pub async fn build_now(
    host: &mut HostHandle,
    now_unix_ms: &(dyn Fn() -> i64 + Send + Sync),
    epoch_unix_ms: i64,
) -> Result<Built, BuildError> {
    let deadline = tokio::time::Instant::now() + STALE_WAIT;
    loop {
        let read = host.read();
        let now = now_unix_ms()
            .checked_sub(epoch_unix_ms)
            .ok_or(BuildError::Clock)?;
        let t = now.max(read.world.time_ms);
        match build(&read, t, epoch_unix_ms) {
            Err(BuildError::Stale) => {}
            Ok(board) => {
                let json = serde_json::to_vec(&board).map_err(|_| BuildError::Serialize)?;
                return Ok(Built { t, json });
            }
            Err(e) => return Err(e),
        }
        match tokio::time::timeout_at(deadline, host.changed()).await {
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => return Err(BuildError::Stale),
        }
    }
}

/// The job: waits, rebuilds the board, and publishes each success; a failure keeps the previous
/// board. The first of a run of failures logs at error level, the rest at debug, and the next
/// success at info with the count. It waits `every` once a board exists, and
/// [`RETRY_UNTIL_FIRST`] until then. Each wait starts after a build ends, so a slow build never
/// bunches builds up (the effect of `MissedTickBehavior::Delay`). It runs until its task is
/// dropped: `serve` drops it with its server runtime (D-050).
pub async fn run(
    mut host: HostHandle,
    now_unix_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    epoch_unix_ms: i64,
    every: Duration,
    publish: Publish,
) {
    // Failed builds in a row: the first is logged at error level, the rest at debug, and the
    // recovery at info, so a host stuck for minutes doesn't flood the log (dev-qa's I2).
    let mut failing: u64 = 0;
    loop {
        // The wait comes first: `serve` builds once itself before `ready:`.
        let wait = if publish.borrow().is_some() {
            every
        } else {
            RETRY_UNTIL_FIRST
        };
        tokio::time::sleep(wait).await;
        match build_now(&mut host, now_unix_ms.as_ref(), epoch_unix_ms).await {
            Ok(built) => {
                if failing > 0 {
                    tracing::info!(failed = failing, "the leaderboard builds again");
                }
                failing = 0;
                tracing::debug!(t = built.t, bytes = built.json.len(), "leaderboard built");
                publish.send_replace(Some(Arc::new(built)));
            }
            Err(error) => {
                failing = failing.saturating_add(1);
                if failing == 1 {
                    tracing::error!(%error, "the leaderboard build failed; the previous board stays");
                } else {
                    tracing::debug!(%error, failing, "the leaderboard build failed again");
                }
            }
        }
    }
}

/// Why a board couldn't be built. The previous board stays.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// The read model is behind an event due at or before `t`: the host hasn't published the step
    /// yet. A projection never crosses an event.
    #[error("the read model is behind a due event")]
    Stale,
    /// A player's view failed (`ScoreOutOfRange`, or an inconsistent World).
    #[error("the view of a player failed: {0}")]
    View(ViewError),
    /// The view gave something other than a player view.
    #[error("a player query gave another view")]
    NotAPlayerView,
    /// `t` can't be put on the wire.
    #[error("the board's time: {0}")]
    Time(time::OutOfRange),
    /// More players than a `u32` rank can count.
    #[error("more players than a rank can count")]
    TooManyPlayers,
    /// The clock is before the epoch by more than `i64` milliseconds.
    #[error("the clock is before the epoch by more than i64")]
    Clock,
    /// The board didn't serialize.
    #[error("the board didn't serialize")]
    Serialize,
}

/// The board for `read` at game time `t`, under `epoch_unix_ms`.
///
/// # Errors
/// [`BuildError`]: a stale read model, a player view that fails, or a time the wire can't carry.
pub fn build(read: &ReadModel, t: i64, epoch_unix_ms: i64) -> Result<api::Leaderboard, BuildError> {
    let mut scores = Vec::with_capacity(read.world.players.len());
    for &player in read.world.players.keys() {
        match read
            .world
            .view_for(&Query::Player { player }, player, t, &read.rules)
        {
            Ok(View::Player(v)) => scores.push((v.score, player)),
            Err(ViewError::Stale { .. }) => return Err(BuildError::Stale),
            Err(e) => return Err(BuildError::View(e)),
            Ok(_) => return Err(BuildError::NotAPlayerView),
        }
    }
    let entries = rank(scores)?
        .into_iter()
        .map(|(rank, player, score)| api::LeaderboardEntry {
            rank,
            player_id: wire::player_id(player),
            score,
        })
        .collect();
    Ok(api::Leaderboard {
        as_of: time::timestamp(epoch_unix_ms, t).map_err(BuildError::Time)?,
        entries,
    })
}

/// D-045's order and ranks: score descending, then player ID ascending (the wire's hyphenated
/// lowercase UUID has fixed-width hex, so its text order is the ID's numeric order); competition
/// ranking, 1 plus the number of players with a strictly higher score, so ties share a rank and
/// the next rank skips (1, 2, 2, 4).
///
/// # Errors
/// [`BuildError::TooManyPlayers`] if a rank leaves `u32`.
pub fn rank(mut scores: Vec<(i64, PlayerId)>) -> Result<Vec<(u32, PlayerId, i64)>, BuildError> {
    scores.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut ranked = Vec::with_capacity(scores.len());
    let mut rank = 1;
    for (i, &(score, player)) in scores.iter().enumerate() {
        if i > 0 && scores[i - 1].0 != score {
            // Every player before this one scores strictly higher.
            rank = i
                .checked_add(1)
                .and_then(|r| u32::try_from(r).ok())
                .ok_or(BuildError::TooManyPlayers)?;
        }
        ranked.push((rank, player, score));
    }
    Ok(ranked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u128) -> PlayerId {
        PlayerId(n)
    }

    fn ranks(scores: &[(i64, u128)]) -> Vec<(u32, u128, i64)> {
        rank(scores.iter().map(|&(s, id)| (s, p(id))).collect())
            .unwrap()
            .into_iter()
            .map(|(r, id, s)| (r, id.0, s))
            .collect()
    }

    /// The lead's measurement (D-051): a build at 250 players (the default cube's practical
    /// limit) and at the 5,000-player cap, each with solved homeworlds. Run in release:
    /// `cargo nextest run -p voidforge-server --lib --release --run-ignored only -E
    /// 'test(build_time)' --no-capture`.
    #[test]
    #[ignore = "a measurement, run by hand in release"]
    fn build_time() {
        use std::sync::Arc;
        use voidforge_testkit::{WorldBuilder, v0_rules};
        for n in [250_u128, 5_000] {
            let rules = v0_rules();
            let mut b = WorldBuilder::new();
            let homes: Vec<_> = (1..=n).map(|i| b.player(PlayerId(i))).collect();
            let mut world = b.build();
            for h in homes {
                world.solve_planet(h, &rules).unwrap();
            }
            let read = ReadModel {
                seq: 0,
                world: Arc::new(world),
                rules: Arc::new(rules),
            };
            let started = std::time::Instant::now();
            let board = build(&read, 3_600_000, 1_760_000_000_000).unwrap();
            let built = started.elapsed();
            let json = serde_json::to_vec(&board).unwrap();
            println!(
                "{n} players: build {built:?}, total with JSON {:?}, {} bytes",
                started.elapsed(),
                json.len()
            );
            assert_eq!(board.entries.len() as u128, n);
        }
    }

    #[test]
    fn competition_ranking_with_ties_in_the_middle_and_at_the_end() {
        assert_eq!(
            ranks(&[(10, 5), (30, 1), (20, 9), (20, 2), (10, 3)]),
            vec![(1, 1, 30), (2, 2, 20), (2, 9, 20), (4, 3, 10), (4, 5, 10)]
        );
    }

    #[test]
    fn all_tied_share_rank_1_in_id_order_and_none_is_empty() {
        assert_eq!(
            ranks(&[(7, 3), (7, 1), (7, 2)]),
            vec![(1, 1, 7), (1, 2, 7), (1, 3, 7)]
        );
        assert!(ranks(&[]).is_empty());
    }

    #[test]
    fn strictly_ordered_scores_rank_1_to_n() {
        assert_eq!(
            ranks(&[(1, 1), (3, 2), (2, 3)]),
            vec![(1, 2, 3), (2, 3, 2), (3, 1, 1)]
        );
    }

    /// For many generated boards: each rank is 1 plus the count strictly ahead, and the order is
    /// (score descending, ID ascending), with every player once.
    #[test]
    fn rank_is_one_plus_the_count_strictly_ahead() {
        let mut seed: u64 = 0x5eed;
        let mut next = |m: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % m
        };
        for _ in 0..500 {
            let n = next(40);
            let scores: Vec<(i64, u128)> = (0..n)
                .map(|id| (i64::try_from(next(6)).unwrap(), u128::from(id)))
                .collect();
            let ranked = ranks(&scores);
            assert_eq!(ranked.len(), scores.len());
            for &(r, _, s) in &ranked {
                let ahead = scores.iter().filter(|&&(o, _)| o > s).count();
                assert_eq!(r as usize, ahead + 1);
            }
            for w in ranked.windows(2) {
                assert!(w[0].2 > w[1].2 || (w[0].2 == w[1].2 && w[0].1 < w[1].1));
            }
        }
    }
}
