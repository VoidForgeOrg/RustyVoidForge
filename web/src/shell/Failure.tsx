import { Button, Spinner } from "@heroui/react";
import type { ReactNode } from "react";
import { type Context, failure, type Fill } from "../copy/messages";
import { failures } from "../copy/voices";

/** A framed notice: an in-universe heading over plain copy, with the code beside it. */
export function Notice({
  heading,
  copy,
  code,
  strip = false,
  children,
}: {
  heading: string;
  copy: string;
  code?: string | null;
  strip?: boolean;
  children?: ReactNode;
}) {
  return (
    <section
      role="alert"
      className={
        strip
          ? "frame mb-3 flex flex-wrap items-center gap-x-3 gap-y-2 border border-separator bg-surface px-4 py-2"
          : "frame flex max-w-xl flex-col gap-3 border border-separator bg-surface p-5"
      }
    >
      <h2 className={strip ? "text-danger" : "text-lg text-danger"}>{heading}</h2>
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
  strip = false,
  onRetry,
}: {
  error: unknown;
  context?: Context;
  fill?: Fill;
  strip?: boolean;
  onRetry?: () => void;
}) {
  const message = failure(error, context, fill);
  return (
    <Notice
      heading={context === "page" ? failures.read : failures.order}
      copy={message.copy}
      code={message.code}
      strip={strip}
    >
      {message.kind === "retry" && onRetry && (
        <Button className="self-start" size={strip ? "sm" : "md"} variant="secondary" onPress={onRetry}>
          Try again
        </Button>
      )}
    </Notice>
  );
}

interface Query<T> {
  data: T | undefined;
  error: unknown;
  isError: boolean;
  refetch: () => Promise<unknown>;
}

/** A failed refetch as a strip above the view it leaves standing. */
export function Stale({ query, fill = {} }: { query: Query<unknown>; fill?: Fill }) {
  return query.isError && <Failure error={query.error} fill={fill} strip onRetry={() => void query.refetch()} />;
}

/** A read's view. Data once shown stays shown; the full failure is only for a read with none yet. */
export function Read<T>({
  query,
  fill = {},
  loading,
  children,
}: {
  query: Query<T>;
  fill?: Fill;
  loading: string;
  children: (data: T) => ReactNode;
}) {
  if (query.data === undefined) {
    return query.isError ? (
      <Failure error={query.error} fill={fill} onRetry={() => void query.refetch()} />
    ) : (
      <Spinner aria-label={loading} />
    );
  }
  return (
    <>
      <Stale query={query} fill={fill} />
      {children(query.data)}
    </>
  );
}
