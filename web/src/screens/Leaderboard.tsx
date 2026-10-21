import { Spinner, Table } from "@heroui/react";
import { clock, player, thousands } from "../format";
import { useLeaderboard, useMe } from "../game/queries";
import { Failure } from "../shell/Failure";

export function Leaderboard() {
  const board = useLeaderboard();
  const me = useMe().data;
  if (board.isPending || !me) return <Spinner aria-label="Loading standings" />;
  if (board.isError) return <Failure error={board.error} onRetry={() => void board.refetch()} />;
  const asOf = Date.parse(board.data.as_of);
  return (
    <>
      <p className="mb-3 text-sm text-muted">As of {clock(board.data.as_of, asOf)}</p>
      <Table>
        <Table.ScrollContainer>
          <Table.Content aria-label="Imperial standings">
            <Table.Header>
              <Table.Column isRowHeader>Rank</Table.Column>
              <Table.Column>Player</Table.Column>
              <Table.Column>Score</Table.Column>
            </Table.Header>
            <Table.Body>
              {board.data.entries.map((e) => (
                <Table.Row key={e.player_id} className={e.player_id === me.player_id ? "text-accent" : ""}>
                  <Table.Cell>{e.rank}</Table.Cell>
                  <Table.Cell>{player(e.player_id, me.player_id)}</Table.Cell>
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
