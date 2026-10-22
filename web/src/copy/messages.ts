import { ApiError } from "../api/client";
import type { components } from "../api/schema";

export type ErrorCode = components["schemas"]["ErrorCode"];

/** Where a failure happened. A retry's copy differs: an order's says whether it was sent. */
export type Context = "page" | "command";

/** The numbers some copy names, when the screen has them. */
export interface Fill {
  planet?: number;
  target?: number;
}

/** What the screen does with a failure. */
type Kind = "inline" | "retry" | "login" | "universe-full";

interface Message {
  copy: string;
  /** The API's code, shown beside the copy; null for the app's own states (§6.2). */
  code: ErrorCode | null;
  kind: Kind;
}

const UNREACHABLE = {
  page: "Can't reach the game server. Try again in a moment.",
  command:
    "Can't reach the game server. Your order may or may not have gone through. Trying again is safe: it won't run twice.",
};

const UNEXPECTED = {
  page: "The game server answered something unexpected. This is a bug; please report it.",
  command:
    "The game server answered something unexpected, so your order may or may not have gone through. Trying again is safe: it won't run twice. This is a bug; please report it.",
};

export const LOGIN_FAILED = "The login didn't complete. Log in again.";

/** The app's own checks before an order is sent (§6.3). */
export const checks = {
  ships: "Pick at least one ship.",
  cargo: "Enter cargo in tonnes, like 250 or 12.5.",
  target: "Enter a planet number, like 12.",
};

export const ORDER_LOST =
  "Your login expired, so that order wasn't sent. Log in again, then submit it again.";

/**
 * The message for a failed call; anything but an ApiError (an unreadable success body) is an
 * unexpected answer. The API's own `message` is never shown.
 */
export function failure(error: unknown, context: Context, fill: Fill = {}): Message {
  const f = error instanceof ApiError ? error.failure : ({ kind: "unexpected", status: 0 } as const);
  if (f.kind === "unreachable") return { copy: UNREACHABLE[context], code: null, kind: "retry" };
  const text = f.kind === "refused" ? copy(f.code, context, fill) : null;
  if (f.kind === "unexpected" || text === null) {
    return { copy: UNEXPECTED[context], code: null, kind: "retry" };
  }
  return { copy: withWait(text, f.retryAfter), code: f.code, kind: kindOf(f.code, context) };
}

function kindOf(code: ErrorCode, context: Context): Kind {
  switch (code) {
    case "missing_token":
    case "invalid_token":
      return "login";
    case "no_free_homeworld":
      return context === "page" ? "universe-full" : "inline";
    case "rate_limited":
    case "internal":
    case "busy":
    case "unavailable":
      return "retry";
    default:
      return "inline";
  }
}

/** "in a moment" names the wait once Retry-After is over 2 s (§5.5). */
function withWait(text: string, retryAfter: number | null): string {
  return retryAfter !== null && retryAfter > 2
    ? text.replace("in a moment", `in ${retryAfter.toString()} seconds`)
    : text;
}

const numbered = (n: number | undefined, known: (n: string) => string, unknown: string) =>
  n === undefined ? unknown : known(n.toString());

/**
 * §6.1's sentence for `code`. The switch has no default arm for a known code, so a code added to
 * the schema fails the typecheck until it has its sentence; a code the schema doesn't know is null.
 */
export function copy(code: ErrorCode, context: Context, fill: Fill = {}): string | null {
  const command = context === "command";
  switch (code) {
    case "malformed_body":
      return "The game didn't understand that order. This is a bug in the web client, not something you did; please report it.";
    case "missing_idempotency_key":
      return "The form was missing its submission key. This is a bug in the web client; reload the page and try again.";
    case "invalid_idempotency_key":
      return "The form's submission key wasn't valid. This is a bug in the web client; reload the page and try again.";
    case "duplicate_ship_ids":
      return "A ship was picked twice. Reload the page and pick each ship once.";
    case "invalid_query":
      return "Those numbers couldn't be used for a search. Check them, or use a smaller radius (at most 1,000 planets per search).";
    case "missing_token":
      return "You're not logged in. Log in to continue.";
    case "invalid_token":
      return "Your session ended. Log in again to continue.";
    case "not_found":
      return "That page doesn't exist on the game server. This is a bug in the web client; please report it.";
    case "already_joined":
      return "You've already joined this universe.";
    case "no_free_homeworld":
      return "There's no free homeworld for a new player, so your account can't join this universe. Trying again won't change that.";
    case "not_joined":
      return "Your account hasn't joined the game yet. Reload the page to join.";
    case "unknown_planet":
      return numbered(
        fill.planet,
        (p) => `There's no planet #${p}. Check the number on the Universe page.`,
        "There's no such planet. Check the number on the Universe page.",
      );
    case "unknown_building":
      return "That building no longer exists. It may have just been demolished or cancelled.";
    case "unknown_ship_build":
      return "That ship build no longer exists. It may have just finished or been cancelled.";
    case "not_owner":
      return numbered(
        fill.planet,
        (p) => `Planet #${p} isn't yours. You can build, and load cargo, only on your own planets.`,
        "That planet isn't yours. You can build, and load cargo, only on your own planets.",
      );
    case "no_free_slot":
      return "Every building slot on this planet is taken. Demolish or cancel a building to free one.";
    case "not_under_construction":
      return "That building isn't under construction any more. It may have just been completed.";
    case "not_operational":
      return "That building isn't working: it's under construction or being demolished.";
    case "shipyard_has_builds":
      return "That Shipyard still has ships building or waiting. Cancel them before demolishing it.";
    case "not_a_shipyard":
      return "That building isn't a Shipyard.";
    case "shipyard_not_operational":
      return "That Shipyard isn't working: it's under construction or being demolished.";
    case "unknown_fleet":
      return "That fleet no longer exists. It may have been disbanded, or used its last ship to colonize.";
    case "unknown_ship":
      return "One of those ships isn't available any more. Reload the page and pick again.";
    case "no_ships":
      return "Pick at least one ship.";
    case "invalid_cargo":
      return "Cargo can't be negative.";
    case "ship_not_on_roster":
      return "One of those ships isn't on this planet's roster any more. Reload the page and pick again.";
    case "insufficient_stock":
      return "This planet doesn't have that much in storage. Load less.";
    case "insufficient_capacity":
      return "Those ships can't carry that much. Load less, or add Cargo Vessels.";
    case "fleet_not_stationed":
      return "That fleet is flying, so it can't do that now. Wait until it's stationed.";
    case "fleet_not_in_transit":
      return "That fleet isn't flying a mission, so there's nothing to turn back. It may have just arrived.";
    case "already_recalled":
      return "That fleet has already turned back once, and it can't again.";
    case "same_planet":
      return numbered(
        fill.target,
        (t) => `The fleet is already at Planet #${t}. Choose another target.`,
        "The fleet is already at that planet. Choose another target.",
      );
    case "no_colony_ship":
      return "Colonizing needs a Colony Ship in the fleet.";
    case "target_owned":
      return numbered(
        fill.target,
        (t) => `Planet #${t} is already owned. Colonize needs an unowned planet; Move goes anywhere.`,
        "That planet is already owned. Colonize needs an unowned planet; Move goes anywhere.",
      );
    case "no_cargo_vessel":
      return "Transport needs a Cargo Vessel in the fleet.";
    case "target_not_owned":
      return "Transport goes only to your own planets. To go elsewhere, use Move.";
    case "unreachable_target":
      return numbered(
        fill.target,
        (t) => `Planet #${t} is too far: the trip would take more than 1,000 years.`,
        "That planet is too far: the trip would take more than 1,000 years.",
      );
    case "cargo_aboard":
      return "The fleet still carries cargo. Unload it at one of your planets before disbanding.";
    case "planet_not_owned":
      return "The fleet isn't at one of your planets, so it can't unload there. Move it to one of your planets first.";
    case "idempotency_key_conflict":
      return "That form was already used for a different order, so this one wasn't sent. The page has fresh forms now: submit it again.";
    case "rate_limited":
      return command
        ? "Too many requests at once. Try again in a moment; your order wasn't sent."
        : "Too many requests at once. Try again in a moment.";
    case "internal":
      return command
        ? "Something went wrong on the game server. Your order may or may not have gone through. Trying again is safe: it won't run twice."
        : "Something went wrong on the game server. Try again.";
    case "busy":
      return command
        ? "The game is busy. Try again in a moment; your order wasn't sent."
        : "The game is busy. Try again in a moment.";
    case "unavailable":
      return command
        ? "The game is starting up or catching up. Try again in a moment; your order wasn't sent."
        : "The game is starting up or catching up. Try again in a moment.";
    default:
      return unknownCode(code);
  }
}

// Typed `never`, so a schema code without a case above fails the typecheck. At runtime it's a
// code newer than this build's schema, which reads as an unexpected answer.
function unknownCode(code: never): null {
  console.warn("an error code this build doesn't know", code);
  return null;
}
