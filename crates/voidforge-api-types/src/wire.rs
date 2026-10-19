//! The values every message shares: IDs, times, positions, the enums and cargo (D-045, Q1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

macro_rules! entity_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        ///
        /// A JSON integer. Entity IDs come from one engine counter, so leaving JavaScript's safe
        /// range (2^53) would take about 9×10^15 entities (D-045).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
        #[serde(transparent)]
        #[schema(value_type = u64)]
        pub struct $name(pub u64);
    };
}

entity_id!(
    /// A planet.
    PlanetId
);
entity_id!(
    /// A building.
    BuildingId
);
entity_id!(
    /// A ship.
    ShipId
);
entity_id!(
    /// A ship build in a Shipyard.
    ShipBuildId
);
entity_id!(
    /// A fleet.
    FleetId
);

/// A player: the UUIDv7 the accounts table assigns (Decision 9), hyphenated lowercase. The
/// server converts it; this crate checks only the text's form, the same form the schema's
/// pattern requires, so serde and the schema refuse the same strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String")]
#[schema(
    value_type = String,
    format = "uuid",
    pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    example = "01928c5e-7a3b-7c2d-9e4f-5a6b7c8d9e0f"
)]
pub struct PlayerId(String);

impl PlayerId {
    /// The ID's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PlayerId {
    type Error = String;

    /// Lowercase hex in groups of 8, 4, 4, 4 and 12, joined by hyphens.
    fn try_from(text: String) -> Result<Self, String> {
        let valid = text.len() == 36
            && text.bytes().enumerate().all(|(i, b)| match i {
                8 | 13 | 18 | 23 => b == b'-',
                _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
            });
        if valid {
            Ok(Self(text))
        } else {
            Err(format!("not a lowercase hyphenated UUID: {text:?}"))
        }
    }
}

/// A game time: the universe epoch plus game time, as RFC 3339 UTC with milliseconds, always
/// (`2026-10-01T21:00:00.000Z`). Game time is in milliseconds, so a time cut to seconds would be
/// off by up to 999 ms. The server formats it (P3.4, with D-044's stable epoch); this crate
/// checks the form and the calendar, as the schema's pattern and `date-time` format do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String")]
#[schema(
    value_type = String,
    format = DateTime,
    pattern = "^[0-9]{4}-[0-9]{2}-[0-9]{2}T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]\\.[0-9]{3}Z$",
    example = "2026-10-01T21:00:00.000Z"
)]
pub struct Timestamp(String);

impl Timestamp {
    /// The time's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Timestamp {
    type Error = String;

    /// `YYYY-MM-DDTHH:MM:SS.mmmZ`, with a real calendar date (leap years included) and a time
    /// of day. Never a leap second: game time has none.
    fn try_from(text: String) -> Result<Self, String> {
        let b = text.as_bytes();
        let shape = b.len() == 24
            && b.iter().enumerate().all(|(i, c)| match i {
                4 | 7 => *c == b'-',
                10 => *c == b'T',
                13 | 16 => *c == b':',
                19 => *c == b'.',
                23 => *c == b'Z',
                _ => c.is_ascii_digit(),
            });
        let number = |from: usize, to: usize| -> u32 {
            b[from..to]
                .iter()
                .fold(0, |n, c| n * 10 + u32::from(c - b'0'))
        };
        let valid = shape && {
            let (year, month, day) = (number(0, 4), number(5, 7), number(8, 10));
            let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
            let days = match month {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 if leap => 29,
                2 => 28,
                _ => 0,
            };
            (1..=days).contains(&day)
                && number(11, 13) < 24
                && number(14, 16) < 60
                && number(17, 19) < 60
        };
        if valid {
            Ok(Self(text))
        } else {
            Err(format!(
                "not an RFC 3339 UTC time with milliseconds: {text:?}"
            ))
        }
    }
}

/// A point in the universe, in distance units (D-021: 1,000 per world unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// X, in distance units.
    pub x: i64,
    /// Y, in distance units.
    pub y: i64,
    /// Z, in distance units.
    pub z: i64,
}

/// What a building is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuildingType {
    /// Mines iron ore.
    Drill,
    /// Turns iron ore into iron ingots.
    Refinery,
    /// Builds ships.
    Shipyard,
}

/// What a ship is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShipType {
    /// Colonizes an unowned planet.
    ColonyShip,
    /// Carries cargo.
    CargoVessel,
}

/// What a fleet does at its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mission {
    /// Colonize an unowned planet (needs a Colony Ship).
    Colonize,
    /// Carry cargo to one of the player's planets (needs a Cargo Vessel).
    Transport,
    /// Move to another planet.
    Move,
}

/// Grams of each resource, loaded or carried.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Cargo {
    /// Grams of iron ore.
    pub iron_ore_g: i64,
    /// Grams of iron ingots.
    pub iron_ingot_g: i64,
}
