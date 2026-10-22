import { Table } from "@heroui/react";
import type { components } from "../api/schema";
import { clock, player, thousands } from "../format";
import { useLeaderboard, useMe } from "../game/queries";
import { Read } from "../shell/Failure";

export function Leaderboard() {
  const board = useLeaderboard();
  const me = useMe().data;
  if (!me) return null;
  return (
    <Read query={board} loading="Loading standings">
      {(view) => <Standings view={view} me={me.player_id} />}
    </Read>
  );
}

function Standings({ view, me }: { view: components["schemas"]["Leaderboard"]; me: string }) {
  const asOf = Date.parse(view.as_of);
  return (
    <>
      <p className="mb-3 text-sm text-muted">As of {clock(view.as_of, asOf)}</p>
      <Table>
        <Table.ScrollContainer>
          <Table.Content aria-label="Imperial standings">
            <Table.Header>
              <Table.Column isRowHeader>Rank</Table.Column>
              <Table.Column>Player</Table.Column>
              <Table.Column>Score</Table.Column>
            </Table.Header>
            <Table.Body>
              {view.entries.map((e) => (
                <Table.Row key={e.player_id} className={e.player_id === me ? "text-accent" : ""}>
                  <Table.Cell>{e.rank}</Table.Cell>
                  <Table.Cell>{player(e.player_id, me)}</Table.Cell>
                  <Table.Cell>{thousands(e.score)}</Table.Cell>
                </Table.Row>
              ))}
            </Table.Body>
          </Table.Content>
        </Table.ScrollContainer>
      </Table>
    </>
  );
}
