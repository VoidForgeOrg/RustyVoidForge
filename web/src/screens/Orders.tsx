import { AlertDialog, Button } from "@heroui/react";
import type { ReactNode } from "react";
import { type Order, fillOf, useOrder } from "../game/commands";
import { Failure } from "../shell/Failure";

type Variant = "primary" | "secondary" | "tertiary" | "danger-soft";

/** A row of order controls under a card's or drawer's details. */
export const Actions = ({ children }: { children: ReactNode }) => (
  <div className="mt-3 flex flex-wrap items-start gap-2 border-t border-separator pt-3">{children}</div>
);

/** An order's refusal in place, with Try again on the same key when the outcome is unknown. */
export function Refusal({ error, order, retry }: { error: unknown; order: Order; retry: () => void }) {
  return <Failure error={error} context="command" fill={fillOf(order)} onRetry={retry} />;
}

/** A button that sends one order; disabled while it's in flight, its refusal shown below it. */
export function OrderButton({
  order,
  children,
  variant = "secondary",
  onDone,
}: {
  order: Order;
  children: ReactNode;
  variant?: Variant;
  onDone?: () => void;
}) {
  const { send, pending, error } = useOrder();
  const go = async () => {
    if (await send(order)) onDone?.();
  };
  return (
    <>
      <Button size="sm" variant={variant} isPending={pending} onPress={() => void go()}>
        {children}
      </Button>
      {error && <Refusal error={error} order={order} retry={() => void go()} />}
    </>
  );
}

/** An action the view already shows the API would refuse: disabled, with the reason beside it. */
export function Unavailable({ children, reason }: { children: ReactNode; reason: string }) {
  return (
    <div className="flex flex-col gap-1">
      <Button size="sm" variant="danger-soft" isDisabled>
        {children}
      </Button>
      <p className="text-xs text-muted">{reason}</p>
    </div>
  );
}

/** One of the four irreversible orders: a question and its consequence before anything is sent. */
export function Confirm({
  order,
  label,
  question,
  consequence,
  keep = "Keep it",
}: {
  order: Order;
  label: string;
  question: string;
  consequence: string;
  keep?: string;
}) {
  const { send, pending, error, reset } = useOrder();
  return (
    <AlertDialog>
      <Button size="sm" variant="danger-soft" onPress={reset}>
        {label}
      </Button>
      <AlertDialog.Backdrop>
        <AlertDialog.Container>
          <AlertDialog.Dialog>
            {({ close }) => {
              const go = async () => {
                if (await send(order)) close();
              };
              return (
                <>
                  <AlertDialog.Header>
                    <AlertDialog.Heading>{question}</AlertDialog.Heading>
                  </AlertDialog.Header>
                  <AlertDialog.Body className="flex flex-col gap-3">
                    <p>{consequence}</p>
                    {error && <Refusal error={error} order={order} retry={() => void go()} />}
                  </AlertDialog.Body>
                  <AlertDialog.Footer>
                    <Button variant="tertiary" slot="close">
                      {keep}
                    </Button>
                    <Button variant="danger" isPending={pending} onPress={() => void go()}>
                      {label}
                    </Button>
                  </AlertDialog.Footer>
                </>
              );
            }}
          </AlertDialog.Dialog>
        </AlertDialog.Container>
      </AlertDialog.Backdrop>
    </AlertDialog>
  );
}
