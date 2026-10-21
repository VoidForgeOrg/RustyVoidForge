import type { User } from "oidc-client-ts";
import { create } from "zustand";

// Mirrors the UserManager's stored user for rendering; auth.ts is its only writer.
export const useAuth = create<{ user: User | null }>(() => ({ user: null }));
