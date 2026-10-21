import { Button } from "@heroui/react";
import type { ReactNode } from "react";
import { type Context, failure, type Fill } from "../copy/messages";
import { failures } from "../copy/voices";

/** A framed notice: an in-universe heading over plain copy, with the code beside it. */
export function Notice({
  heading,
  copy,
  code,
  children,
}: {
  heading: string;
  copy: string;
  code?: string | null;
  children?: ReactNode;
}) {
  return (
    <section role="alert" className="frame flex max-w-xl flex-col gap-3 border border-separator bg-surface p-5">
      <h2 className="text-lg text-danger">{heading}</h2>
      <p>
        {copy}
        {code && <span className="ml-2 font-mono text-muted">({code})</span>}
      </p>
      {children}
    </section>
  );
}

/** A failed call, in §6.1's words, with Try again when trying again can help. */
export function Failure({
  error,
  context = "page",
  fill,
  onRetry,
}: {
  error: unknown;
  context?: Context;
  fill?: Fill;
  onRetry?: () => void;
}) {
  const message = failure(error, context, fill);
  return (
    <Notice heading={context === "page" ? failures.read : failures.order} copy={message.copy} code={message.code}>
      {message.kind === "retry" && onRetry && (
        <Button className="self-start" variant="secondary" onPress={onRetry}>
          Try again
        </Button>
      )}
    </Notice>
  );
}
