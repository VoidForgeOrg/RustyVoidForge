import { type Browser, expect, type Locator, type Page, test } from "@playwright/test";

// The web walk (D-062, D-063): alice logs in through dex's real pages, every screen renders from the
// live game, and the MVP loop runs through the console, each step checked against the API's JSON
// with alice's own token. e2e.sh starts the API, serves the built app and names them here.

const API = required("WALK_API");
const ISSUER = required("WALK_ISSUER");
// TEST-ONLY: the static dex user from deploy/dex/config*.yaml.
const USER = "alice@voidforge.test";
const PASSWORD = "alice-local-test-password";

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set; run the walk through .github/scripts/e2e.sh`);
  return value;
}

interface Building {
  id: number;
  type: string;
  state: string;
}
interface Planet {
  owner: string | null;
  buildings: Building[];
  roster: { ship_id: number; ship_type: string; owner: string }[];
  stocks: { iron_ingot: { amount_g: number } };
}
interface Fleet {
  fleet_id: number;
  state: string;
  at_planet_id?: number;
  target_planet_id?: number;
  mission?: string;
  ships: { ship_id: number }[];
  cargo: { iron_ore_g: number; iron_ingot_g: number };
}

/** alice's ID token through dex's password grant on the bots' public client. */
async function apiToken(): Promise<string> {
  const response = await fetch(`${ISSUER}/token`, {
    method: "POST",
    body: new URLSearchParams({
      grant_type: "password",
      username: USER,
      password: PASSWORD,
      client_id: "voidforge-cli",
      scope: "openid",
    }),
  });
  const body = (await response.json()) as { id_token?: string };
  if (!body.id_token)
    throw new Error(`dex's password grant gave no ID token (HTTP ${response.status.toString()})`);
  return body.id_token;
}

let token = "";

async function get<T>(path: string): Promise<T> {
  const response = await fetch(API + path, { headers: { Authorization: `Bearer ${token}` } });
  if (!response.ok) throw new Error(`GET ${path}: HTTP ${response.status.toString()}`);
  return (await response.json()) as T;
}

const planet = (id: number) => get<Planet>(`/planets/${id.toString()}`);
const fleets = async () => (await get<{ fleets: Fleet[] }>("/players/me/fleets")).fleets;

/** Polls `probe` until it answers something, as the game's time moves on. */
async function waitFor<T>(
  what: string,
  probe: () => Promise<T | null | undefined>,
  seconds = 90,
): Promise<T> {
  const until = Date.now() + seconds * 1_000;
  for (;;) {
    const value = await probe();
    if (value !== null && value !== undefined) return value;
    if (Date.now() > until) throw new Error(`timed out after ${seconds.toString()} s waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

/** Grams as the cargo field takes them: exact tonnes. */
const tonnes = (g: number) =>
  `${Math.floor(g / 1_000_000).toString()}.${(g % 1_000_000).toString().padStart(6, "0")}`;

/** Waits until the planet holds twice `grams` of ingots, since ship builds drain them, then loads `grams`. */
async function load(id: number, grams: number): Promise<number> {
  await waitFor(`Planet #${id.toString()} holding ${tonnes(2 * grams)} t of ingots`, async () =>
    (await planet(id)).stocks.iron_ingot.amount_g >= 2 * grams ? true : null,
  );
  return grams;
}

async function logInThroughDex(page: Page) {
  await page.getByRole("button", { name: "Log in" }).click();
  await page.getByPlaceholder("email address").fill(USER);
  await page.getByPlaceholder("password").fill(PASSWORD);
  await page.getByRole("button", { name: "Login" }).click();
}

/** Ticks a ship by its label, as a player does: HeroUI hides the input itself under the label. */
async function tick(stepper: Locator, name: string) {
  await stepper.getByText(name, { exact: true }).click();
  await expect(stepper.getByRole("checkbox", { name })).toBeChecked();
}

const ordered = (page: Page, line: string | RegExp) =>
  expect(page.getByText(line).first()).toBeVisible({ timeout: 15_000 });

test("alice plays the MVP loop through the console", async ({ page, browser }) => {
  // Logged out, the console offers only the login.
  await page.goto("/planets");
  await expect(page.getByRole("heading", { name: "Imperial Access" })).toBeVisible();

  await logInThroughDex(page);
  await expect(page).toHaveURL(/\/planets/);
  await expect(page.getByText(USER)).toBeVisible();

  token = await apiToken();
  const me = await get<{ player_id: string; homeworld_id: number }>("/players/me");
  const home = me.homeworld_id;
  const here = `/planets/${home.toString()}`;

  // Every screen renders from the live game.
  await expect(page.getByRole("article", { name: `#${home.toString()} Home` })).toBeVisible();
  await page.goto("/fleets");
  await expect(page.getByRole("heading", { name: "Fleet Command" })).toBeVisible();
  await page.goto("/universe");
  await expect(page.getByRole("grid", { name: "Planets in range, nearest first" })).toBeVisible();
  await page.goto("/leaderboard");
  await expect(page.getByRole("heading", { name: "Imperial Standings" })).toBeVisible();

  // Build a Shipyard with a double click: the button is disabled while the order is in flight,
  // so it builds once.
  const shipyards = async () => (await planet(home)).buildings.filter((b) => b.type === "shipyard");
  const before = (await shipyards()).length;
  await page.goto(`${here}/build`);
  await page.getByRole("button", { name: "Build Shipyard" }).dblclick();
  await ordered(page, `Construction of a Shipyard started on Planet #${home.toString()}.`);
  await page.waitForTimeout(1_000);
  expect((await shipyards()).length, "the double click built one Shipyard").toBe(before + 1);
  const yard = await waitFor("a Shipyard becoming operational", async () =>
    (await shipyards()).find((b) => b.state === "operational"),
  );

  // Queue a Colony Ship and a Cargo Vessel; wait until both are on the roster.
  await page.goto(`${here}/buildings/${yard.id.toString()}`);
  for (const ship of ["Colony Ship", "Cargo Vessel"]) {
    await page.getByRole("button", { name: `Queue ${ship}` }).click();
    await ordered(page, `A ${ship} was queued at Shipyard #${yard.id.toString()}.`);
  }
  const ships = await waitFor(
    "both ships on the homeworld's roster",
    async () => {
      const mine = (await planet(home)).roster.filter((s) => s.owner === me.player_id);
      const colony = mine.find((s) => s.ship_type === "colony_ship");
      const cargo = mine.find((s) => s.ship_type === "cargo_vessel");
      return colony && cargo ? { colony: colony.ship_id, cargo: cargo.ship_id } : null;
    },
    150,
  );

  // The stepper assembles both with 1 t of ingots and colonizes the nearest unowned planet.
  const loaded = await load(home, 1_000_000);
  await page.goto(`${here}?mission=true`);
  const stepper = page.getByRole("dialog", { name: "New mission" });
  await tick(stepper, `Colony Ship #${ships.colony.toString()}`);
  await tick(stepper, `Cargo Vessel #${ships.cargo.toString()}`);
  await stepper.getByRole("textbox", { name: "Ingots (t)" }).fill(tonnes(loaded));
  await stepper.getByRole("button", { name: "Next" }).click();
  await expect(stepper.getByRole("radio", { name: /Colonize/ })).toBeChecked();
  // The e2e universe has few planets, so the nearest unowned one may lie past the default radius.
  await stepper.getByRole("textbox", { name: "Radius (u)" }).fill("1000");
  await stepper.getByRole("textbox", { name: "Radius (u)" }).press("Tab");
  const unowned = stepper.getByRole("option", { name: /· unowned ·/ }).first();
  const colony = Number(/^#(\d+)/.exec((await unowned.textContent()) ?? "")?.[1]);
  await unowned.click();
  await expect(unowned).toHaveAttribute("aria-selected", "true");
  await stepper.getByRole("button", { name: "Next" }).click();
  await stepper.getByRole("button", { name: "Send" }).click();
  await ordered(page, new RegExp(`Fleet #\\d+ left for Planet #${colony.toString()} \\(Colonize\\)\\.`));

  const flying = (await fleets()).find((f) => f.state === "in_transit" && f.target_planet_id === colony);
  expect(flying?.mission).toBe("colonize");
  expect(flying?.cargo).toEqual({ iron_ore_g: 0, iron_ingot_g: loaded });
  const fid = flying?.fleet_id ?? 0;
  await waitFor(
    `Planet #${colony.toString()} becoming alice's`,
    async () => ((await planet(colony)).owner === me.player_id ? true : null),
    60,
  );

  // Disband the Cargo Vessel's fleet at the colony.
  await waitFor("the fleet stationed at the colony", async () =>
    (await fleets()).find((f) => f.fleet_id === fid && f.state === "stationed"),
  );
  await page.goto("/fleets");
  const card = page.locator("section", { has: page.locator(`#fleet-${fid.toString()}`) });
  await card.getByRole("button", { name: "Disband" }).click();
  await ordered(
    page,
    `Fleet #${fid.toString()} disbanded. Its ships are on Planet #${colony.toString()}'s roster.`,
  );
  expect((await planet(colony)).roster.map((s) => s.ship_id)).toContain(ships.cargo);

  // Assemble the Cargo Vessel at the colony with 0.5 t and transport it home.
  const carried = await load(colony, 500_000);
  await page.goto(`/planets/${colony.toString()}?mission=true`);
  await tick(stepper, `Cargo Vessel #${ships.cargo.toString()}`);
  await stepper.getByRole("textbox", { name: "Ingots (t)" }).fill(tonnes(carried));
  await stepper.getByRole("button", { name: "Next" }).click();
  await expect(stepper.getByRole("radio", { name: /Transport/ })).toBeChecked();
  await stepper.getByRole("textbox", { name: "Target planet #" }).fill(home.toString());
  await stepper.getByRole("button", { name: "Next" }).click();
  await stepper.getByRole("button", { name: "Send" }).click();
  await ordered(page, new RegExp(`Fleet #\\d+ left for Planet #${home.toString()} \\(Transport\\)\\.`));
  const back = await waitFor(
    "the transport arriving home",
    async () =>
      (await fleets()).find(
        (f) =>
          f.state === "stationed" &&
          f.at_planet_id === home &&
          f.ships.some((s) => s.ship_id === ships.cargo),
      ),
    60,
  );
  expect(back.cargo, "the transport unloaded at home").toEqual({ iron_ore_g: 0, iron_ingot_g: 0 });

  // Closing the stepper returned to the colony's panel; Esc closes it back to the grid.
  await expect(page.getByRole("dialog", { name: `#${colony.toString()}` })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page).toHaveURL(/\/planets$/);

  // Logout forgets the login: the next login shows dex's form again.
  await page.getByText("Log out").click();
  await expect(page.getByRole("heading", { name: "Imperial Access" })).toBeVisible();
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page.getByPlaceholder("email address")).toBeVisible();

  await refusesATamperedCallback(browser);
});

/** A callback whose state this browser never issued logs nobody in. */
async function refusesATamperedCallback(browser: Browser) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto("/callback?code=forged&state=forged");
  await expect(page.getByRole("heading", { name: "Access denied" })).toBeVisible();
  await context.close();
}
