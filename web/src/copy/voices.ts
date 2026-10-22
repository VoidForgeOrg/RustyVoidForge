// The console's own voice (D-063): titles, headings and empty states. Verbs, numbers and §6.1's
// copy stay plain and live elsewhere.

export const titles = {
  planets: "Planetary Holdings",
  fleets: "Fleet Command",
  universe: "Star Charts",
  leaderboard: "Imperial Standings",
} as const;

export const access = {
  heading: "Imperial Access",
  prompt: "Identify yourself to take command of your holdings.",
  full: "Registry closed",
};

export const empties = {
  roster: "No ships in orbit.",
  fleets: "No fleets under your command.",
  near: "No worlds within range.",
  slot: "This slot stands empty.",
};

export const failures = {
  read: "Transmission failed",
  order: "Order refused",
  unconfirmed: "Order unconfirmed",
  login: "Access denied",
  uncharted: "Uncharted coordinates",
  unchartedCopy: "No such planet or building is on record. Check the number and try again.",
};

export const orders = {
  done: "Order executed",
  assembledUnsent: "The fleet was assembled; send it from Fleet Command.",
  assembled: (fleet: number, planet: number) =>
    `Fleet #${fleet.toString()} is assembled and stationed at Planet #${planet.toString()}.`,
};
