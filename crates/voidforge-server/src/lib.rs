//! The server's library: what the binary's subcommands and the integration tests share. P3.2 adds
//! the token checks ([`auth`]). P3.3 adds accounts ([`accounts`]: a principal becomes a player,
//! who joins the game exactly once) and the game-code mapping ([`codes`]). Routes and the
//! leaderboard arrive with P3.4 and P3.5.

pub mod accounts;
pub mod auth;
pub mod codes;
