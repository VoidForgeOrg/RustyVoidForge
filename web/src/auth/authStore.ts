import type { User } from "oidc-client-ts";
import { create } from "zustand";

// Mirrors the UserManager's stored user for rendering; auth.ts is its only writer. `lostOrder` is
// set when the login ended on an order, which therefore wasn't sent.
export const useAuth = create<{ user: User | null; lostOrder: boolean }>(() => ({
  user: null,
  lostOrder: false,
}));
