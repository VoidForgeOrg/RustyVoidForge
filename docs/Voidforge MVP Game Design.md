# Voidforge MVP Game Design

Oct 1, 2026 · @Tomáš Grbálik

## What Voidforge is

Voidforge is a persistent space strategy MMO: players build empires across a universe of planets, manage physical economies and expand through exploration and colonization. It is a headless engine. All logic runs on the server behind an open API, and any client (web, mobile, CLI, VR) can play.

The MVP core loop:

1. Start with a working Homeworld: a Drill mines ore and a Refinery turns it into ingots.
2. Grow the economy with more Drills and Refineries, within the planet's slots and storage.
3. Build a Shipyard, then Colony Ships and Cargo Vessels.
4. Colonize new planets and supply them with ingots by cargo fleet, because new colonies start empty.
5. Climb the leaderboard, where score counts everything a player owns.

## Universe and planets

The universe is a 3D space of free-floating planets. Planets are the core entity, and in the MVP every planet is fully visible to every player.

- 3D coordinates determine distances, and distances set fleet travel times.
- A planet is owned by exactly one player or by no one. Colonization is the only way to gain one, and ownership is permanent in the MVP.

Every planet, owned or not, has the same properties:

| Property | What it is |
| --- | --- |
| Iron Ore | Unlimited on every planet; a Drill extracts at the same rate anywhere |
| Building slots | A fixed number of identical slots; any building fits any slot |
| Planetary storage | A fixed capacity per resource type |

All of these, plus ownership, are visible to all players at all times. Fleets are visible only to their owner.

### Starting conditions

Each new player receives a Homeworld: a pre-colonized planet with 1 Drill and 1 Refinery, both operational, plus starting resources in storage (see Rules v0).

New colonies start empty, with no buildings. Players must ship Iron Ingots in and build from scratch, so a Colony Ship should travel with a Cargo Vessel carrying building materials. Expansion is a logistical commitment, not a land grab.

## Resources and economy

Voidforge has a physical economy: resources exist on specific planets, in planetary storage, and there is no global bank. Moving them between planets takes a cargo fleet.

The MVP has one linear chain: Iron Ore, extracted by Drills, is refined into Iron Ingots at 1 t of ore to 2 t of ingots, and ingots become buildings and ships. The chain is designed to branch into multiple ores and alloys after the MVP.

| Resource | Kind | Source | Used for |
| --- | --- | --- | --- |
| Iron Ore | Raw, stored | Drills extract it; every planet's supply is unlimited | Refining |
| Iron Ingots | Refined, stored | Refineries: 1 t of ore gives 2 t of ingots | Building construction and ship building |

Ore never runs out: every planet has an unlimited supply, and a Drill extracts at the same rate wherever it stands. Colonizing is how an empire adds building slots, and with them production.

Storage has a fixed capacity per resource type (for example 10,000 Iron Ore and 5,000 Iron Ingots). When it is full, the buildings producing that resource slow down to match what is taken out, and halt only if nothing is.

Competing consumers split the available supply evenly, so no building starves the others. If a consumer needs less than its equal share, the unused part goes evenly to the others. A planet producing 10 t of ore per hour with two Refineries that each want 8 t per hour gives each Refinery 5 t per hour, so building ratios matter.

## Buildings

Buildings occupy a planet's fixed, identical slots and form the backbone of its economy. Slots are a hard limit, so every building is a choice.

| Building | Function | Halts when |
| --- | --- | --- |
| Drill | Extracts Iron Ore into storage, continuously | Ore storage is full and nothing draws from it; otherwise it slows to match |
| Refinery | Converts Iron Ore from storage into Iron Ingots at 1:2, continuously | No ore is in stock or arriving, or ingot storage is full and nothing draws from it; with empty ore storage it runs at what the Drills deliver |
| Shipyard | Builds ships from Iron Ingots, up to 3 in parallel, with an unlimited queue | No ingots are available; resumes when supply returns |

Each ship type has a build duration and a total ingot cost, consumed continuously over the build. Queued ships start automatically as parallel slots free up.

### Construction

- Each building type has a build duration and a total Iron Ingot cost, consumed continuously from storage at cost divided by duration.
- A building under construction occupies its slot at once and starts working when it is complete.
- If ingots run out, construction halts and resumes when supply returns.
- Several buildings can be built in parallel on one planet, limited by slots and ingot supply.
- Construction can be cancelled at any time: nothing is refunded and the slot frees immediately.

Continuous consumption lets players start ambitious builds early and feed them over time, including by cargo fleet from other planets.

### Demolition

Demolition frees a slot but takes time (see Rules v0) and refunds nothing. The building stops working the moment demolition begins, and the slot frees when it completes. A Shipyard can't be demolished while it has ships building or queued.

## Ships and fleets

Ships are built at Shipyards, wait on a planet's ship roster, and travel in fleets. Fleets carry out three MVP missions: Colonize, Transport and Move.

| Ship | Purpose | Notes |
| --- | --- | --- |
| Colony Ship | Colonizes an uncolonized planet | Consumed on use; built from Iron Ingots; has a travel speed |
| Cargo Vessel | Transports resources between planets | Fixed tonnage capacity, mixed cargo; built from Iron Ingots; has a travel speed |

### Fleets

- A fleet is one or more ships, of any mix of types, launched from a planet.
- It travels as a unit at the speed of its slowest ship, in a straight line between planets.
- Its cargo capacity is the sum of its Cargo Vessels. In the MVP cargo is tracked as fleet totals, not per ship.

### Missions

| Mission | Requires | Target | On arrival |
| --- | --- | --- | --- |
| Colonize | At least one Colony Ship | An uncolonized planet; an order to an owned planet is refused | One Colony Ship is consumed and the planet becomes the player's; extra Colony Ships stay; cargo unloads automatically, partially if storage is full. If another player claimed it first, the mission fails: nothing is consumed and cargo stays aboard. If two colony fleets arrive at the same moment, the one assembled first wins |
| Transport | At least one Cargo Vessel | A planet the player owns | Cargo, loaded from the origin's storage before departure, unloads into storage; if storage is full the rest stays aboard for a later unload |
| Move | Any ships | Any planet: own, uncolonized or another player's | The ships relocate; no cargo is unloaded |

After any arrival, successful or not, the fleet stays stationed at the destination.

### Travel and recall

Travel time follows from the 3D distance between the two planets and the fleet's speed. A fleet in transit can be recalled once: it turns around and takes as long to return as it had traveled, so a fleet recalled 5 hours into a 12-hour trip returns in 5 hours. It arrives stationed at its origin with cargo and any Colony Ship intact, delivering nothing and claiming nothing.

### Roster and lifecycle

- Each planet keeps a ship roster: ships stationed there and available for new fleets.
- Ships finished by a Shipyard join that planet's roster.
- Arriving fleets stay stationed; their ships reach a roster only when the fleet is disbanded.
- A fleet can be disbanded at any planet, but not while it carries cargo.
- Ships may stay on the roster of an unowned planet indefinitely.

![Fleet lifecycle: 3 states, 6 transitions](FleetRoster.png)

Assembly takes ships off the roster and loads any cargo; every arrival leaves the fleet stationed until it is sent again or disbanded.

## Game engine model

Voidforge has no global tick or heartbeat. Game state is computed on demand and changes only at discrete events, so nothing runs in the background between them.

Lazy calculation: any value is its last checkpoint plus rate × elapsed time. A Drill mining 2 t of ore per hour, checked 3 hours after the last checkpoint, has added 6 t.

Scheduled events: many events are known the moment their cause happens, and they are queued for that time.

- Storage filling: ore storage with 1,000 t of free space, gaining 2 t/h, schedules a full-storage event at hour 500.
- Fleet arrival: distance and speed fix the arrival time at departure.
- Construction and ship completion: scheduled from the current ingot supply, and moved whenever that supply changes.

Checkpoints: when an event fires or a player acts, the engine saves the affected entities' current state and resets their baseline. Triggers include a building coming online, storage filling or emptying, a fleet arriving and any player command.

Cascades: chain reactions resolve within a single checkpoint, so state is always consistent.

- Ore storage empties → Refineries can use only what the Drills deliver → ingot production drops → construction and ship builds slow or halt once ingots run out.
- Ingot storage fills with nothing drawing from it → Refinery halts → ore storage fills → Drill halts.
- A Cargo Vessel unloads ingots → halted construction and ship builds resume.

The engine is the final arbiter: every player action is validated against the current calculated state before it is accepted.

Game time is real time. If the server goes down, the world keeps running, and on restart the engine catches up on every missed event in order. Pausing game time during maintenance is post-MVP.

## Scoring

Each player's score counts everything they own, including partial credit for ships and buildings under construction, so score tracks the resources a player has invested, and drives the public leaderboard.

| Counts | Including |
| --- | --- |
| Planets | Every colonized planet |
| Buildings | By number and type; a building under construction counts in proportion to the ingots delivered so far; cancelled or demolished buildings do not count |
| Ships | On rosters and in fleets, stationed or moving, each counted once; a ship under construction counts in proportion to the ingots delivered so far |
| Resources | In planetary storage and aboard fleets, stationed or moving |

- Point values live in one rules table, listed in Rules v0, and are placeholders pending balancing: a Shipyard is worth more than a Drill, a Colony Ship more than a Cargo Vessel, and a unit of ingot more than a unit of ore.
- Score is recalculated on demand and never stored, so it always reflects current state.
- The leaderboard is visible to all players and refreshes every 60 seconds; a player's own score is always current.

After the MVP, leaderboards may split score into categories, such as top economy or most planets, to reward different playstyles.

## Player actions

These ten actions are every interaction a player has with the MVP, each submitted through the API.

| Area | Action | Rules |
| --- | --- | --- |
| Planet | Build a building | Needs a free slot; choose Drill, Refinery or Shipyard; consumes ingots continuously; occupies the slot at once; works once complete |
| Planet | Cancel construction | No refund; the slot frees immediately |
| Planet | Demolish a building | Takes time; the building stops working immediately; no refund; the slot frees when demolition completes; a Shipyard must have no ships building or queued |
| Shipyard | Queue ship construction | Choose Colony Ship or Cargo Vessel; up to 3 build in parallel, the rest wait and start automatically; ingots consumed continuously |
| Shipyard | Cancel ship construction | Works on active or queued ships; no refund; cancelling an active build frees its slot and starts the next queued ship |
| Fleet | Assemble fleet | Pick ships from a planet's roster, in any mix; optionally load cargo from storage at a planet you own, spread across Cargo Vessels by tonnage |
| Fleet | Send fleet on mission | Colonize, Transport or Move, with the requirements in Ships and fleets; the fleet departs |
| Fleet | Cancel fleet in transit | Turns the fleet around once; it returns in the time already traveled |
| Fleet | Disband fleet | Only when stationed and carrying no cargo; ships return to that planet's roster |
| Fleet | Trigger unload | At a planet the player owns; partial if storage is full, with the rest staying aboard |

A fleet carrying cargo at an unowned planet must first Move to a planet the player owns and unload there before it can be disbanded.

## Rules v0

These are placeholder numbers for the MVP, chosen to fit together and to match the examples above. Balancing will change them; the engine reads them from its rules files, so a new number never needs new code. Masses are in tons, rates are per hour, and distances are in world units.

| Planet value | Default |
| --- | --- |
| Building slots | 8 |
| Iron Ore storage cap | 10,000 t |
| Iron Ingot storage cap | 5,000 t |

| Building | Cost (ingots) | Build time | Demolition time | Output |
| --- | --- | --- | --- | --- |
| Drill | 150 t | 3 h | 1 h | 10 t of ore per hour |
| Refinery | 250 t | 4 h | 1 h | Takes 8 t of ore and makes 16 t of ingots per hour |
| Shipyard | 1,200 t | 12 h | 2 h | Builds up to 3 ships at once |

| Ship | Cost (ingots) | Build time | Speed | Cargo capacity |
| --- | --- | --- | --- | --- |
| Colony Ship | 800 t | 10 h | 10 units per hour | None |
| Cargo Vessel | 300 t | 4 h | 15 units per hour | 500 t |

Builds draw their cost evenly over their build time: a Drill takes 50 t of ingots per hour for 3 hours. A new Homeworld starts with 2,000 t of Iron Ore and 1,500 t of Iron Ingots, enough to start a Shipyard on day one while its Drill and Refinery bring in 16 t of ingots per hour.

| Score item | Points |
| --- | --- |
| Planet | 1,000 |
| Drill | 100 |
| Refinery | 150 |
| Shipyard | 500 |
| Colony Ship | 400 |
| Cargo Vessel | 150 |
| Iron Ore | 1 per 10 t |
| Iron Ingots | 2 per 10 t |

## Not in MVP

These features are planned for after the MVP and are out of scope here:

- Energy: Generators, power draw and overload, and later fuel-consuming generators
- Finite, depletable resource pools on each planet
- Fog of war, scouting and Scout Vessels
- Combat and military ships, with cargo bound to individual ships so that losses have economic weight
- Planet conquest, so that planets can be lost
- Alliances and clans
- A tech tree (Astrophysics, Materials Science, Applied Weaponry)
- A galactic market and player trading
- Planet types and traits (Volcanic, Frozen and others)
- Advanced travel: hyperlanes, jump gates and warp corridors
- More missions: Scout, Trade, Interception and military missions
- Branching resource trees with multiple ores and alloys
- Solar systems that group planets, with system-level mechanics and traits
- Slot types and specialization
- Expandable storage buildings
- Score categories on leaderboards
- Pausing game time during server maintenance
- Real-time notifications: push over SSE or WebSocket, and an event feed
