//! Server-rendered HTML frontend and OIDC login; reaches the server only over HTTP.
//!
//! P4.1 (D-057) is the login: an authorization code flow with PKCE through dex, a server-side
//! session holding the ID token, Home in its two states, and the logout. The binary in
//! `main.rs` wires these together; tests drive [`routes::app`] directly.

pub mod api;
pub mod config;
pub mod oidc;
pub mod pages;
pub mod routes;
pub mod session;
