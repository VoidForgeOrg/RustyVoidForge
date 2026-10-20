//! The server's library: what the binary's subcommands and the integration tests share. P3.2 adds
//! the token checks ([`auth`]). P3.3 adds accounts ([`accounts`]: a principal becomes a player,
//! who joins the game exactly once) and the game-code mapping ([`codes`]). P3.4 adds the routes
//! ([`api`]); P3.4s adds [`serve`], which runs them over a recovered host. P3.5 adds the
//! [`leaderboard`], and P3.8 the rate limits ([`limit`]).

pub mod accounts;
pub mod api;
pub mod auth;
pub mod codes;
pub mod leaderboard;
pub mod limit;
pub mod rules_view;
pub mod serve;
