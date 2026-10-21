//! The command forms (P4.4, D-061): a submitted form read into the api-types [`CommandRequest`],
//! with the web's own checks first (phase4-ux §6.3), and the words of its outcome (§5.3, §6.4).
//!
//! - Every form posts `action` (the API's command `type`), `key` (the form key the page was
//!   rendered with) and `return_to` (the page it's on), beside the command's own fields. IDs are
//!   hidden fields, types and missions are radios, Assemble's ships are repeated `ship` fields,
//!   and cargo is typed in tonnes and converted exactly to grams.
//! - A form also carries the names its success banner reads (a building's type, a fleet's
//!   origin or planet): the 200 carries only the command's own events (D-045), and a tampered
//!   name only changes what the player reads about their own action (§5.3).
//! - Nothing here calls the API: a [`Check`] is refused before any call.

use std::collections::BTreeSet;

use voidforge_api_types::command::{
    AssembleFleet, BuildBuilding, BuildingRef, CancelShipBuild, CommandAccepted, CommandRequest,
    FleetRef, QueueShip, SendFleet,
};
use voidforge_api_types::error::ErrorCode;
use voidforge_api_types::event::Event;
use voidforge_api_types::wire::{
    BuildingId, BuildingType, Cargo, FleetId, Mission, PlanetId, ShipBuildId, ShipId, ShipType,
};

use crate::format;
use crate::messages::Fill;
use crate::routes::local_path;

/// §6.3: a form key this session didn't issue (Q3): an old tab, a dropped key, or another site.
pub const EXPIRED_FORM: &str =
    "That form has expired. The page has fresh forms now: submit it again.";
/// §5.2: the host had already answered this form's key; its first answer follows.
pub const ALREADY_SUBMITTED: &str =
    "This form was already submitted; this is its result. Nothing was done twice.";
/// §5.1: after a 401 on a submit and the login that follows it.
pub const ORDER_NOT_SENT: &str = "Your session ended, so that order wasn't sent. Submit it again.";
/// A submit on an expired login: D-057's carried line, worded with "order" as the rest of the
/// copy is (dev-design's U9).
pub const ACTION_EXPIRED: &str =
    "Your login expired, so that order wasn't sent. Log in again, then submit it again.";
/// A submit without a login: the session ended or was never there, so there's nothing to send
/// with (P4.4's dev-design question U1, built as recommended).
pub const NOT_LOGGED_IN: &str =
    "You're not logged in, so that order wasn't sent. Log in, then submit it again.";

/// A form as it was posted: every field, in order, so the retry page can post it again exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Posted {
    /// The fields, decoded, as sent.
    pub pairs: Vec<(String, String)>,
}

impl Posted {
    /// Reads an `application/x-www-form-urlencoded` body. Anything undecodable reads as empty
    /// fields, which the checks then refuse.
    #[must_use]
    pub fn parse(body: &[u8]) -> Self {
        let pairs = reqwest::Url::parse("http://x/")
            .map(|mut u| {
                u.set_query(Some(&String::from_utf8_lossy(body)));
                u.query_pairs().into_owned().collect()
            })
            .unwrap_or_default();
        Self { pairs }
    }

    /// The first value of `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.pairs
            .iter()
            .filter(move |(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The form key, if it sent one.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.get("key").filter(|k| !k.is_empty())
    }

    /// Where the outcome is shown: `return_to` when it's a local path, otherwise `/` (§5.1.5).
    #[must_use]
    pub fn return_to(&self) -> &str {
        self.get("return_to").and_then(local_path).unwrap_or("/")
    }
}

/// §5.5 (P4.4's D1): the retry page's back link, labelled from `return_to`'s path.
#[must_use]
pub fn back_label(return_to: &str) -> String {
    let path = return_to.split('?').next().unwrap_or("");
    if let Some(n) = path.strip_prefix("/planets/").and_then(number) {
        return format!("Back to Planet #{n}");
    }
    match path {
        "/fleets" => "Back to your fleets",
        "/universe" => "Back to the Universe",
        "/" => "Back to Home",
        _ => "Back",
    }
    .to_owned()
}

/// Why the web refused a form without calling (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// No building type chosen.
    BuildingType,
    /// No ship type chosen.
    ShipType,
    /// No mission chosen.
    Mission,
    /// No fleet chosen (the planet page's Send a fleet here).
    Fleet,
    /// No ship ticked.
    Ships,
    /// A cargo field that isn't tonnes.
    Cargo,
    /// A typed target that isn't a planet number.
    PlanetNumber,
    /// A ship twice: the API's `duplicate_ship_ids`, caught first.
    DuplicateShips,
    /// A field the web's own forms always send well is missing or wrong: a bug or a tampered
    /// form (P4.4's dev-design question U2, built as recommended).
    Malformed,
}

impl Check {
    /// The sentence (§6.3), and the API's code where the check stands in for one.
    #[must_use]
    pub fn copy(self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::BuildingType => ("Choose a building type.", None),
            Self::ShipType => ("Choose a ship type.", None),
            Self::Mission => ("Choose a mission.", None),
            Self::Fleet => ("Choose a fleet.", None),
            Self::Ships => ("Pick at least one ship.", None),
            Self::Cargo => ("Enter cargo in tonnes, like 250 or 12.5.", None),
            Self::PlanetNumber => ("Enter a planet number, like 12.", None),
            Self::DuplicateShips => (
                "A ship was picked twice. Reload the page and pick each ship once.",
                Some(ErrorCode::DuplicateShipIds.as_str()),
            ),
            Self::Malformed => (
                "That form couldn't be read, so nothing was sent. This is a bug in the web client; reload the page and try again.",
                None,
            ),
        }
    }
}

/// What a success's first line says, from the form's own fields (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    /// `build_building`.
    Built(BuildingType, u64),
    /// `cancel_construction`: the type and the building.
    ConstructionCancelled(BuildingType, u64),
    /// `demolish_building`.
    Demolishing(BuildingType, u64),
    /// `queue_ship`: the ship type and the Shipyard.
    Queued(ShipType, u64),
    /// `cancel_ship_build`.
    BuildCancelled(u64),
    /// `assemble_fleet`: how many ships, and the planet.
    Assembled(usize, u64),
    /// `send_fleet`: the fleet, the target and the mission.
    Sent(u64, u64, Mission),
    /// `recall_fleet`: the fleet and where it returns to.
    TurnedBack(u64, u64),
    /// `disband_fleet`: the fleet and the planet its ships stay on.
    Disbanded(u64, u64),
    /// `trigger_unload`: the words come from its event, if any.
    Unloaded,
}

/// A checked form: the command, its success words, and the numbers its refusals name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// What `POST /commands` sends.
    pub command: CommandRequest,
    /// The success line's facts.
    pub done: Done,
    /// The planet and target a refusal's copy names (§6.1).
    pub fill: Fill,
}

impl Order {
    /// Where the browser goes after a 200: Fleets after Assemble and Send, where the fleet is
    /// managed and `arrives_at` shows (§5.3); otherwise the form's page.
    #[must_use]
    pub fn after_success<'a>(&self, return_to: &'a str) -> &'a str {
        match self.done {
            Done::Assembled(..) | Done::Sent(..) => "/fleets",
            _ => return_to,
        }
    }
}

/// A number the web takes: digits only, within u64.
fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn building_type(text: &str) -> Option<BuildingType> {
    match text {
        "drill" => Some(BuildingType::Drill),
        "refinery" => Some(BuildingType::Refinery),
        "shipyard" => Some(BuildingType::Shipyard),
        _ => None,
    }
}

fn ship_type(text: &str) -> Option<ShipType> {
    match text {
        "colony_ship" => Some(ShipType::ColonyShip),
        "cargo_vessel" => Some(ShipType::CargoVessel),
        _ => None,
    }
}

fn mission(text: &str) -> Option<Mission> {
    match text {
        "colonize" => Some(Mission::Colonize),
        "transport" => Some(Mission::Transport),
        "move" => Some(Mission::Move),
        _ => None,
    }
}

/// A choice the player makes (a radio or the fleet select): absent or empty is `missing`; a
/// value no form offers is [`Check::Malformed`].
fn chosen<T>(
    p: &Posted,
    name: &str,
    missing: Check,
    read: fn(&str) -> Option<T>,
) -> Result<T, Check> {
    match p.get(name).filter(|v| !v.is_empty()) {
        None => Err(missing),
        Some(v) => read(v).ok_or(Check::Malformed),
    }
}

/// A hidden field the form always sends.
fn hidden<T>(p: &Posted, name: &str, read: fn(&str) -> Option<T>) -> Result<T, Check> {
    p.get(name).and_then(read).ok_or(Check::Malformed)
}

/// Reads and checks a form (§6.3), before any call.
///
/// # Errors
/// The first [`Check`] it fails.
pub fn order(p: &Posted) -> Result<Order, Check> {
    let action = p.get("action").unwrap_or("");
    let mut fill = Fill::default();
    let (command, done) = match action {
        "build_building" => {
            let planet = hidden(p, "planet_id", number)?;
            fill.planet = Some(planet);
            let t = chosen(p, "building_type", Check::BuildingType, building_type)?;
            (
                CommandRequest::BuildBuilding(BuildBuilding {
                    planet_id: PlanetId(planet),
                    building_type: t,
                }),
                Done::Built(t, planet),
            )
        }
        "cancel_construction" | "demolish_building" => {
            let b = hidden(p, "building_id", number)?;
            let t = hidden(p, "building_type", building_type)?;
            let r = BuildingRef {
                building_id: BuildingId(b),
            };
            if action == "cancel_construction" {
                (
                    CommandRequest::CancelConstruction(r),
                    Done::ConstructionCancelled(t, b),
                )
            } else {
                (CommandRequest::DemolishBuilding(r), Done::Demolishing(t, b))
            }
        }
        "queue_ship" => {
            let yard = hidden(p, "shipyard_id", number)?;
            let t = chosen(p, "ship_type", Check::ShipType, ship_type)?;
            (
                CommandRequest::QueueShip(QueueShip {
                    shipyard_id: BuildingId(yard),
                    ship_type: t,
                }),
                Done::Queued(t, yard),
            )
        }
        "cancel_ship_build" => {
            let n = hidden(p, "ship_build_id", number)?;
            (
                CommandRequest::CancelShipBuild(CancelShipBuild {
                    ship_build_id: ShipBuildId(n),
                }),
                Done::BuildCancelled(n),
            )
        }
        "assemble_fleet" => {
            let planet = hidden(p, "planet_id", number)?;
            fill.planet = Some(planet);
            let ids = p
                .all("ship")
                .map(|s| number(s).map(ShipId))
                .collect::<Option<Vec<_>>>()
                .ok_or(Check::Malformed)?;
            if ids.is_empty() {
                return Err(Check::Ships);
            }
            let mut seen = BTreeSet::new();
            if !ids.iter().all(|id| seen.insert(*id)) {
                return Err(Check::DuplicateShips);
            }
            // A form without cargo fields (not your planet) sends none.
            let tonnes = |name| format::parse_tonnes(p.get(name).unwrap_or("")).ok_or(Check::Cargo);
            let cargo = Cargo {
                iron_ore_g: tonnes("ore")?,
                iron_ingot_g: tonnes("ingots")?,
            };
            let n = ids.len();
            (
                CommandRequest::AssembleFleet(AssembleFleet {
                    planet_id: PlanetId(planet),
                    ship_ids: ids,
                    cargo,
                }),
                Done::Assembled(n, planet),
            )
        }
        "send_fleet" => {
            let fleet = chosen(p, "fleet_id", Check::Fleet, number)?;
            let target = number(p.get("target_planet_id").unwrap_or("").trim())
                .ok_or(Check::PlanetNumber)?;
            fill.target = Some(target);
            let m = chosen(p, "mission", Check::Mission, mission)?;
            (
                CommandRequest::SendFleet(SendFleet {
                    fleet_id: FleetId(fleet),
                    target_planet_id: PlanetId(target),
                    mission: m,
                }),
                Done::Sent(fleet, target, m),
            )
        }
        "recall_fleet" => {
            let fleet = hidden(p, "fleet_id", number)?;
            let origin = hidden(p, "origin", number)?;
            (
                CommandRequest::RecallFleet(FleetRef {
                    fleet_id: FleetId(fleet),
                }),
                Done::TurnedBack(fleet, origin),
            )
        }
        "disband_fleet" => {
            let fleet = hidden(p, "fleet_id", number)?;
            let planet = hidden(p, "planet_id", number)?;
            (
                CommandRequest::DisbandFleet(FleetRef {
                    fleet_id: FleetId(fleet),
                }),
                Done::Disbanded(fleet, planet),
            )
        }
        "trigger_unload" => {
            let fleet = hidden(p, "fleet_id", number)?;
            (
                CommandRequest::TriggerUnload(FleetRef {
                    fleet_id: FleetId(fleet),
                }),
                Done::Unloaded,
            )
        }
        _ => return Err(Check::Malformed),
    };
    Ok(Order {
        command,
        done,
        fill,
    })
}

/// Cargo as §6.4 reads it: `{ore} of ore and {ingots} of ingots`, leaving out a zero resource.
fn cargo_words(c: &Cargo) -> String {
    let parts: Vec<String> = [(c.iron_ore_g, "ore"), (c.iron_ingot_g, "ingots")]
        .iter()
        .filter(|(g, _)| *g != 0)
        .map(|(g, what)| format!("{} of {what}", format::mass(*g)))
        .collect();
    if parts.is_empty() {
        format!(
            "{} of ore and {} of ingots",
            format::mass(0),
            format::mass(0)
        )
    } else {
        parts.join(" and ")
    }
}

/// §6.4: one line per event the command returned.
#[must_use]
pub fn event_line(e: &Event) -> String {
    match e {
        Event::BuildingCompleted {
            planet_id,
            building_id,
            building_type,
        } => format!(
            "{} #{} on Planet #{} is complete.",
            format::building(*building_type),
            building_id.0,
            planet_id.0
        ),
        Event::ShipCompleted {
            planet_id,
            ship_id,
            ship_type,
            ..
        } => format!(
            "A {} (#{}) is ready on Planet #{}'s roster.",
            format::ship(*ship_type),
            ship_id.0,
            planet_id.0
        ),
        Event::DemolitionCompleted {
            planet_id,
            building_id,
            building_type,
        } => format!(
            "{} #{} on Planet #{} is demolished; its slot is free.",
            format::building(*building_type),
            building_id.0,
            planet_id.0
        ),
        Event::FleetArrived {
            fleet_id,
            planet_id,
            mission,
        } => format!(
            "Fleet #{} arrived at Planet #{} ({}).",
            fleet_id.0,
            planet_id.0,
            format::mission(*mission)
        ),
        Event::PlanetColonized { planet_id, .. } => {
            format!("Planet #{} is now yours.", planet_id.0)
        }
        Event::ColonizationFailed {
            planet_id,
            fleet_id,
        } => format!(
            "Planet #{} was taken first. Fleet #{} is stationed there with its cargo.",
            planet_id.0, fleet_id.0
        ),
        Event::CargoUnloaded {
            planet_id, cargo, ..
        } => format!(
            "Unloaded {} into Planet #{}.",
            cargo_words(cargo),
            planet_id.0
        ),
        Event::FleetRemoved { fleet_id, .. } => {
            format!("Fleet #{} used its last ship and is gone.", fleet_id.0)
        }
        Event::FleetReturned {
            fleet_id,
            planet_id,
        } => format!("Fleet #{} is back at Planet #{}.", fleet_id.0, planet_id.0),
    }
}

/// A success, in words: the first line (§5.3) and one line per other event (§6.4). An unload's
/// own `cargo_unloaded` is its first line, so it isn't repeated.
#[must_use]
pub fn done_lines(done: Done, accepted: &CommandAccepted) -> (String, Vec<String>) {
    let unloaded = accepted
        .events
        .iter()
        .position(|e| matches!(e, Event::CargoUnloaded { .. }));
    let first = match done {
        Done::Built(t, p) => format!(
            "Done: construction of a {} started on Planet #{p}. It draws ingots until it's complete.",
            format::building(t)
        ),
        Done::ConstructionCancelled(t, b) => format!(
            "Done: construction of {} #{b} cancelled. Its slot is free; the ingots delivered are lost.",
            format::building(t)
        ),
        Done::Demolishing(t, b) => format!(
            "Done: {} #{b} is being demolished. It has stopped working; its slot frees when demolition ends.",
            format::building(t)
        ),
        Done::Queued(t, s) => format!(
            "Done: a {} was queued at Shipyard #{s}.",
            format::ship(t)
        ),
        Done::BuildCancelled(n) => {
            format!("Done: build #{n} cancelled. The ingots delivered are lost.")
        }
        Done::Assembled(n, p) => format!(
            "Done: a fleet of {n} {} was assembled at Planet #{p}.",
            if n == 1 { "ship" } else { "ships" }
        ),
        Done::Sent(f, t, m) => format!(
            "Done: Fleet #{f} left for Planet #{t} ({}).",
            format::mission(m)
        ),
        Done::TurnedBack(f, o) => format!("Done: Fleet #{f} turned back to Planet #{o}."),
        Done::Disbanded(f, p) => {
            format!("Done: Fleet #{f} disbanded. Its ships are on Planet #{p}'s roster.")
        }
        Done::Unloaded => match unloaded.map(|i| &accepted.events[i]) {
            Some(Event::CargoUnloaded {
                planet_id, cargo, ..
            }) => format!(
                "Done: unloaded {} into Planet #{}.",
                cargo_words(cargo),
                planet_id.0
            ),
            _ => "Done, but nothing was unloaded: the planet's storage is full. The cargo stays aboard."
                .to_owned(),
        },
    };
    let more = accepted
        .events
        .iter()
        .enumerate()
        .filter(|(i, _)| !(done == Done::Unloaded && Some(*i) == unloaded))
        .map(|(_, e)| event_line(e))
        .collect();
    (first, more)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posted(pairs: &[(&str, &str)]) -> Posted {
        Posted {
            pairs: pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn a_body_decodes_in_order_with_repeats() {
        let p = Posted::parse(
            b"action=assemble_fleet&ship=3&ship=1&ore=1.5&return_to=%2Fplanets%2F4&x=a+b%26c",
        );
        assert_eq!(p.get("ship"), Some("3"));
        assert_eq!(p.all("ship").collect::<Vec<_>>(), ["3", "1"]);
        assert_eq!(p.return_to(), "/planets/4");
        assert_eq!(p.get("x"), Some("a b&c"));
    }

    #[test]
    fn return_to_is_a_local_path_or_home() {
        for (sent, used) in [
            (Some("/fleets"), "/fleets"),
            (Some("/universe?around=4"), "/universe?around=4"),
            (Some("//evil.example"), "/"),
            (Some("https://evil.example/"), "/"),
            (Some("/\\evil.example"), "/"),
            (Some("evil"), "/"),
            (Some(""), "/"),
            (None, "/"),
        ] {
            let p = posted(&sent.map(|s| vec![("return_to", s)]).unwrap_or_default());
            assert_eq!(p.return_to(), used, "{sent:?}");
        }
    }

    #[test]
    fn the_back_link_names_the_page_it_returns_to() {
        for (to, label) in [
            ("/planets/4", "Back to Planet #4"),
            ("/fleets", "Back to your fleets"),
            ("/fleets?target=9", "Back to your fleets"),
            ("/universe", "Back to the Universe"),
            ("/universe?x=1&y=2&z=3&radius=4", "Back to the Universe"),
            ("/", "Back to Home"),
            ("/planets/x", "Back"),
            ("/planets/4/demolish/2", "Back"),
            ("/leaderboard", "Back"),
        ] {
            assert_eq!(back_label(to), label, "{to}");
        }
    }

    #[test]
    fn each_action_reads_into_its_command() {
        let cases: Vec<(Vec<(&str, &str)>, CommandRequest)> = vec![
            (
                vec![
                    ("action", "build_building"),
                    ("planet_id", "4"),
                    ("building_type", "shipyard"),
                ],
                CommandRequest::BuildBuilding(BuildBuilding {
                    planet_id: PlanetId(4),
                    building_type: BuildingType::Shipyard,
                }),
            ),
            (
                vec![
                    ("action", "cancel_construction"),
                    ("building_id", "15"),
                    ("building_type", "drill"),
                ],
                CommandRequest::CancelConstruction(BuildingRef {
                    building_id: BuildingId(15),
                }),
            ),
            (
                vec![
                    ("action", "demolish_building"),
                    ("building_id", "12"),
                    ("building_type", "refinery"),
                ],
                CommandRequest::DemolishBuilding(BuildingRef {
                    building_id: BuildingId(12),
                }),
            ),
            (
                vec![
                    ("action", "queue_ship"),
                    ("shipyard_id", "15"),
                    ("ship_type", "cargo_vessel"),
                ],
                CommandRequest::QueueShip(QueueShip {
                    shipyard_id: BuildingId(15),
                    ship_type: ShipType::CargoVessel,
                }),
            ),
            (
                vec![("action", "cancel_ship_build"), ("ship_build_id", "20")],
                CommandRequest::CancelShipBuild(CancelShipBuild {
                    ship_build_id: ShipBuildId(20),
                }),
            ),
            (
                vec![
                    ("action", "assemble_fleet"),
                    ("planet_id", "4"),
                    ("ship", "31"),
                    ("ship", "30"),
                    ("ore", ""),
                    ("ingots", "500.000001"),
                ],
                CommandRequest::AssembleFleet(AssembleFleet {
                    planet_id: PlanetId(4),
                    ship_ids: vec![ShipId(31), ShipId(30)],
                    cargo: Cargo {
                        iron_ore_g: 0,
                        iron_ingot_g: 500_000_001,
                    },
                }),
            ),
            (
                vec![
                    ("action", "send_fleet"),
                    ("fleet_id", "7"),
                    ("target_planet_id", " 12 "),
                    ("mission", "colonize"),
                ],
                CommandRequest::SendFleet(SendFleet {
                    fleet_id: FleetId(7),
                    target_planet_id: PlanetId(12),
                    mission: Mission::Colonize,
                }),
            ),
            (
                vec![
                    ("action", "recall_fleet"),
                    ("fleet_id", "8"),
                    ("origin", "4"),
                ],
                CommandRequest::RecallFleet(FleetRef {
                    fleet_id: FleetId(8),
                }),
            ),
            (
                vec![
                    ("action", "disband_fleet"),
                    ("fleet_id", "8"),
                    ("planet_id", "9"),
                ],
                CommandRequest::DisbandFleet(FleetRef {
                    fleet_id: FleetId(8),
                }),
            ),
            (
                vec![("action", "trigger_unload"), ("fleet_id", "8")],
                CommandRequest::TriggerUnload(FleetRef {
                    fleet_id: FleetId(8),
                }),
            ),
        ];
        for (pairs, want) in cases {
            assert_eq!(
                order(&posted(&pairs)).map(|o| o.command),
                Ok(want),
                "{pairs:?}"
            );
        }
    }

    #[test]
    fn the_web_s_own_checks_refuse_before_any_call() {
        for (pairs, check) in [
            (
                vec![("action", "build_building"), ("planet_id", "4")],
                Check::BuildingType,
            ),
            (
                vec![
                    ("action", "build_building"),
                    ("planet_id", "4"),
                    ("building_type", ""),
                ],
                Check::BuildingType,
            ),
            (
                vec![
                    ("action", "build_building"),
                    ("planet_id", "4"),
                    ("building_type", "castle"),
                ],
                Check::Malformed,
            ),
            (
                vec![("action", "build_building"), ("building_type", "drill")],
                Check::Malformed,
            ),
            (
                vec![("action", "queue_ship"), ("shipyard_id", "15")],
                Check::ShipType,
            ),
            (
                vec![
                    ("action", "send_fleet"),
                    ("fleet_id", ""),
                    ("target_planet_id", "12"),
                    ("mission", "move"),
                ],
                Check::Fleet,
            ),
            (
                vec![
                    ("action", "send_fleet"),
                    ("fleet_id", "7"),
                    ("target_planet_id", "twelve"),
                    ("mission", "move"),
                ],
                Check::PlanetNumber,
            ),
            (
                vec![
                    ("action", "send_fleet"),
                    ("fleet_id", "7"),
                    ("target_planet_id", "-1"),
                    ("mission", "move"),
                ],
                Check::PlanetNumber,
            ),
            (
                vec![
                    ("action", "send_fleet"),
                    ("fleet_id", "7"),
                    ("target_planet_id", "12"),
                ],
                Check::Mission,
            ),
            (
                vec![("action", "assemble_fleet"), ("planet_id", "4")],
                Check::Ships,
            ),
            (
                vec![
                    ("action", "assemble_fleet"),
                    ("planet_id", "4"),
                    ("ship", "3"),
                    ("ship", "3"),
                ],
                Check::DuplicateShips,
            ),
            (
                vec![
                    ("action", "assemble_fleet"),
                    ("planet_id", "4"),
                    ("ship", "3"),
                    ("ore", "12.5t"),
                ],
                Check::Cargo,
            ),
            (
                vec![
                    ("action", "assemble_fleet"),
                    ("planet_id", "4"),
                    ("ship", "3"),
                    ("ingots", "-1"),
                ],
                Check::Cargo,
            ),
            (
                vec![
                    ("action", "assemble_fleet"),
                    ("planet_id", "4"),
                    ("ship", "x"),
                ],
                Check::Malformed,
            ),
            (
                vec![("action", "recall_fleet"), ("fleet_id", "8")],
                Check::Malformed,
            ),
            (vec![("action", "launch_missiles")], Check::Malformed),
            (vec![], Check::Malformed),
        ] {
            assert_eq!(order(&posted(&pairs)), Err(check), "{pairs:?}");
        }
    }

    #[test]
    fn a_refusal_names_the_form_s_planet_and_target() {
        let o = order(&posted(&[
            ("action", "build_building"),
            ("planet_id", "4"),
            ("building_type", "drill"),
        ]))
        .unwrap();
        assert_eq!(
            o.fill,
            Fill {
                planet: Some(4),
                target: None
            }
        );
        let o = order(&posted(&[
            ("action", "send_fleet"),
            ("fleet_id", "7"),
            ("target_planet_id", "12"),
            ("mission", "move"),
        ]))
        .unwrap();
        assert_eq!(
            o.fill,
            Fill {
                planet: None,
                target: Some(12)
            }
        );
        assert_eq!(o.after_success("/planets/4"), "/fleets");
    }

    fn accepted(events: serde_json::Value) -> CommandAccepted {
        serde_json::from_value(serde_json::json!({
            "seq": 5, "game_time": "2026-10-20T18:04:12.000Z", "events": events,
        }))
        .unwrap()
    }

    #[test]
    fn an_unload_says_what_it_moved_or_that_it_moved_nothing() {
        let moved = accepted(serde_json::json!([{
            "type": "cargo_unloaded", "planet_id": 4, "fleet_id": 8,
            "cargo": {"iron_ore_g": 0, "iron_ingot_g": 500_000_000},
        }]));
        assert_eq!(
            done_lines(Done::Unloaded, &moved),
            (
                "Done: unloaded 500.0 t of ingots into Planet #4.".to_owned(),
                vec![]
            )
        );
        assert_eq!(
            done_lines(Done::Unloaded, &accepted(serde_json::json!([]))).0,
            "Done, but nothing was unloaded: the planet's storage is full. The cargo stays aboard."
        );
        // On another action, the same event is an event line.
        assert_eq!(
            done_lines(Done::Disbanded(8, 4), &moved),
            (
                "Done: Fleet #8 disbanded. Its ships are on Planet #4's roster.".to_owned(),
                vec!["Unloaded 500.0 t of ingots into Planet #4.".to_owned()]
            )
        );
    }

    #[test]
    fn every_event_has_its_line() {
        let all = accepted(serde_json::json!([
            {"type": "building_completed", "planet_id": 4, "building_id": 15, "building_type": "shipyard"},
            {"type": "ship_completed", "planet_id": 4, "shipyard_id": 15, "ship_id": 30, "ship_type": "colony_ship"},
            {"type": "demolition_completed", "planet_id": 4, "building_id": 12, "building_type": "drill"},
            {"type": "fleet_arrived", "fleet_id": 8, "planet_id": 12, "mission": "colonize"},
            {"type": "planet_colonized", "planet_id": 12, "owner": "01890a5d-ac96-774b-bcce-b302099a8057", "fleet_id": 8, "ship_id": 30},
            {"type": "colonization_failed", "planet_id": 12, "fleet_id": 8},
            {"type": "cargo_unloaded", "planet_id": 12, "fleet_id": 8, "cargo": {"iron_ore_g": 250_000_000, "iron_ingot_g": 500_000_000}},
            {"type": "fleet_removed", "fleet_id": 8, "planet_id": 12},
            {"type": "fleet_returned", "fleet_id": 9, "planet_id": 4},
        ]));
        let (first, more) = done_lines(Done::Queued(ShipType::CargoVessel, 15), &all);
        assert_eq!(first, "Done: a Cargo Vessel was queued at Shipyard #15.");
        assert_eq!(
            more,
            [
                "Shipyard #15 on Planet #4 is complete.",
                "A Colony Ship (#30) is ready on Planet #4's roster.",
                "Drill #12 on Planet #4 is demolished; its slot is free.",
                "Fleet #8 arrived at Planet #12 (Colonize).",
                "Planet #12 is now yours.",
                "Planet #12 was taken first. Fleet #8 is stationed there with its cargo.",
                "Unloaded 250.0 t of ore and 500.0 t of ingots into Planet #12.",
                "Fleet #8 used its last ship and is gone.",
                "Fleet #9 is back at Planet #4.",
            ]
        );
    }

    #[test]
    fn each_success_line_is_section_5_3_s() {
        let none = accepted(serde_json::json!([]));
        for (done, line) in [
            (
                Done::Built(BuildingType::Shipyard, 4),
                "Done: construction of a Shipyard started on Planet #4. It draws ingots until it's complete.",
            ),
            (
                Done::ConstructionCancelled(BuildingType::Shipyard, 15),
                "Done: construction of Shipyard #15 cancelled. Its slot is free; the ingots delivered are lost.",
            ),
            (
                Done::Demolishing(BuildingType::Drill, 12),
                "Done: Drill #12 is being demolished. It has stopped working; its slot frees when demolition ends.",
            ),
            (
                Done::Queued(ShipType::ColonyShip, 15),
                "Done: a Colony Ship was queued at Shipyard #15.",
            ),
            (
                Done::BuildCancelled(20),
                "Done: build #20 cancelled. The ingots delivered are lost.",
            ),
            (
                Done::Assembled(2, 4),
                "Done: a fleet of 2 ships was assembled at Planet #4.",
            ),
            (
                Done::Assembled(1, 4),
                "Done: a fleet of 1 ship was assembled at Planet #4.",
            ),
            (
                Done::Sent(7, 12, Mission::Transport),
                "Done: Fleet #7 left for Planet #12 (Transport).",
            ),
            (
                Done::TurnedBack(8, 4),
                "Done: Fleet #8 turned back to Planet #4.",
            ),
            (
                Done::Disbanded(8, 9),
                "Done: Fleet #8 disbanded. Its ships are on Planet #9's roster.",
            ),
        ] {
            assert_eq!(
                done_lines(done, &none),
                (line.to_owned(), vec![]),
                "{done:?}"
            );
        }
    }
}
