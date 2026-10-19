//! Commands, part 2: the five fleet commands (Architectural Design, Commands and Fleets; Game
//! Design, Player actions and Ships and fleets; decision log D-029). They follow D-028's check
//! order, who → what → whose → state, and call D-027's engine-side send, recall and unload.
//!
//! Fleets and ships are visible only to their owner, so **another player's fleet or ship is
//! unknown**: it fails the existence check itself, as `unknown_fleet` or `unknown_ship`, and every
//! later check sees it exactly as one that doesn't exist (D-028 amended by D-049; no answer tells
//! a player another's fleet or ship exists). A planet is public, so `not_owner` stays for planets.

use crate::fleet::{distance, travel_time_ms};
use crate::rules::Rules;
use crate::state::{
    Cargo, EngineError, Fleet, FleetId, FleetState, Mission, PlanetId, ShipKind, ShipLocation,
    World,
};

use super::{Action, Command, Rejection};

impl World {
    /// The first rule a fleet command breaks at world time. The player has joined.
    pub(super) fn check_fleet(&self, command: &Command, rules: &Rules) -> Result<(), Rejection> {
        let player = command.player;
        // The player's own fleet, or unknown: another player's is never told apart (D-049).
        let fleet = |id: FleetId| -> Result<&Fleet, Rejection> {
            self.fleets
                .get(&id)
                .filter(|f| f.owner == player)
                .ok_or(Rejection::UnknownFleet)
        };
        let stationed = |f: &Fleet| match f.state {
            FleetState::Stationed { at } => Ok(at),
            _ => Err(Rejection::FleetNotStationed),
        };
        match &command.action {
            Action::AssembleFleet {
                planet,
                ships,
                cargo,
            } => {
                let p = self.planets.get(planet).ok_or(Rejection::UnknownPlanet)?;
                // Another player's ship is unknown, at the same check as one that doesn't exist.
                if ships
                    .iter()
                    .any(|s| self.ships.get(s).is_none_or(|x| x.owner != player))
                {
                    return Err(Rejection::UnknownShip);
                }
                if ships.is_empty() {
                    return Err(Rejection::NoShips);
                }
                if cargo.ore_g < 0 || cargo.ingots_g < 0 {
                    return Err(Rejection::InvalidCargo);
                }
                let loads = *cargo != Cargo::default();
                if loads && p.owner != Some(player) {
                    return Err(Rejection::NotOwner);
                }
                if ships
                    .iter()
                    .any(|s| self.ships[s].location != ShipLocation::Roster(*planet))
                {
                    return Err(Rejection::ShipNotOnRoster);
                }
                if loads {
                    // A stock that cannot be read at now cannot be loaded from.
                    let now = self.time_ms;
                    let covers = |stock: crate::quantity::Quantity, want: i64| {
                        stock.value_at(now).is_ok_and(|have| have >= want)
                    };
                    if !covers(p.ore, cargo.ore_g) || !covers(p.ingots, cargo.ingots_g) {
                        return Err(Rejection::InsufficientStock);
                    }
                    let capacity = ships.iter().try_fold(0i64, |sum, s| {
                        let kind = self.ships[s].kind;
                        sum.checked_add(crate::fleet::ship_spec(rules, kind).cargo_capacity_g)
                    });
                    let load = cargo.ore_g.checked_add(cargo.ingots_g);
                    if !matches!((load, capacity), (Some(l), Some(c)) if l <= c) {
                        return Err(Rejection::InsufficientCapacity);
                    }
                }
                Ok(())
            }
            Action::SendFleet {
                fleet: id,
                target,
                mission,
            } => {
                let f = fleet(*id)?;
                let to = self.planets.get(target).ok_or(Rejection::UnknownPlanet)?;
                let at = stationed(f)?;
                if at == *target {
                    return Err(Rejection::SamePlanet);
                }
                let has = |kind: ShipKind| f.ships.iter().any(|s| self.ships[s].kind == kind);
                match mission {
                    Mission::Colonize => {
                        if !has(ShipKind::ColonyShip) {
                            return Err(Rejection::NoColonyShip);
                        }
                        if to.owner.is_some() {
                            return Err(Rejection::TargetOwned);
                        }
                    }
                    Mission::Transport => {
                        if !has(ShipKind::CargoVessel) {
                            return Err(Rejection::NoCargoVessel);
                        }
                        if to.owner != Some(player) {
                            return Err(Rejection::TargetNotOwned);
                        }
                    }
                    Mission::Move => {}
                }
                // The arrival must be a real instant after now: never a zero travel time, nor
                // one past the end of time, so no command can poison the engine (D-029).
                let travel = self
                    .fleet_speed(*id, rules)
                    .ok()
                    .zip(distance(self.planets[&at].position, to.position))
                    .and_then(|(speed, d)| travel_time_ms(d, speed))
                    .filter(|&ms| ms > 0)
                    .and_then(|ms| self.time_ms.checked_add(ms));
                if travel.is_none() {
                    return Err(Rejection::UnreachableTarget);
                }
                Ok(())
            }
            Action::RecallFleet { fleet: id } => match fleet(*id)?.state {
                // The return, now plus the time traveled, must be a real instant: near the end
                // of time it may not be, and a player's command must never poison (D-029).
                FleetState::InTransit { departed_at_ms, .. } => {
                    let now = self.time_ms;
                    // Checked: a departure far in the past can make either step overflow (D-032).
                    match now
                        .checked_sub(departed_at_ms)
                        .and_then(|traveled| traveled.checked_add(now))
                    {
                        Some(_) => Ok(()),
                        None => Err(Rejection::UnreachableTarget),
                    }
                }
                FleetState::Returning { .. } => Err(Rejection::AlreadyRecalled),
                FleetState::Stationed { .. } => Err(Rejection::FleetNotInTransit),
            },
            Action::DisbandFleet { fleet: id } => {
                let f = fleet(*id)?;
                stationed(f)?;
                if f.cargo != Cargo::default() {
                    return Err(Rejection::CargoAboard);
                }
                Ok(())
            }
            Action::TriggerUnload { fleet: id } => {
                let at = stationed(fleet(*id)?)?;
                if self.planets[&at].owner != Some(player) {
                    return Err(Rejection::PlanetNotOwned);
                }
                Ok(())
            }
            _ => unreachable!("only fleet commands are checked here"),
        }
    }

    /// Applies a validated fleet command at world time; returns the planet to re-solve, if its
    /// storage changed.
    pub(super) fn execute_fleet(
        &mut self,
        command: &Command,
        rules: &Rules,
    ) -> Result<Option<PlanetId>, EngineError> {
        match &command.action {
            Action::AssembleFleet {
                planet,
                ships,
                cargo,
            } => {
                let id = FleetId(self.allocate_id()?);
                for s in ships {
                    self.ships.get_mut(s).expect("checked").location = ShipLocation::Fleet(id);
                }
                let p = self.planets.get_mut(planet).expect("checked");
                p.roster.retain(|s| !ships.contains(s));
                self.fleets.insert(
                    id,
                    Fleet {
                        id,
                        owner: command.player,
                        ships: ships.clone(),
                        cargo: *cargo,
                        version: 0,
                        state: FleetState::Stationed { at: *planet },
                    },
                );
                if *cargo == Cargo::default() {
                    return Ok(None);
                }
                self.bring_planet(*planet)?;
                self.load(*planet, *cargo)?;
                Ok(Some(*planet))
            }
            Action::SendFleet {
                fleet,
                target,
                mission,
            } => {
                self.send_fleet(*fleet, *target, *mission, rules)?;
                Ok(None)
            }
            Action::RecallFleet { fleet } => {
                self.recall_fleet(*fleet)?;
                Ok(None)
            }
            Action::DisbandFleet { fleet } => {
                let f = self.fleets.remove(fleet).expect("checked");
                let FleetState::Stationed { at } = f.state else {
                    unreachable!("checked")
                };
                for s in &f.ships {
                    self.ships.get_mut(s).expect("in the fleet").location =
                        ShipLocation::Roster(at);
                }
                let p = self.planets.get_mut(&at).expect("its station");
                p.roster.extend(f.ships.iter().copied());
                Ok(None)
            }
            Action::TriggerUnload { fleet } => {
                let FleetState::Stationed { at } = self.fleets[fleet].state else {
                    unreachable!("checked")
                };
                self.bring_planet(at)?;
                self.unload(*fleet, at)?;
                Ok(Some(at))
            }
            _ => unreachable!("only fleet commands are executed here"),
        }
    }
}
