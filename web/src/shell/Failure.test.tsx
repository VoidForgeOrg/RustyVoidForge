import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { ApiError } from "../api/client";
import { Failure } from "./Failure";

afterEach(cleanup);

test("an order whose outcome is unknown is unconfirmed, not refused", () => {
  render(<Failure error={new ApiError({ kind: "unreachable" })} context="command" onRetry={() => undefined} />);
  expect(screen.getByRole("heading", { name: "Order unconfirmed" })).toBeInTheDocument();
  cleanup();
  render(
    <Failure
      error={new ApiError({ kind: "refused", status: 404, code: "unknown_fleet", retryAfter: null })}
      context="command"
    />,
  );
  expect(screen.getByRole("heading", { name: "Order refused" })).toBeInTheDocument();
});
