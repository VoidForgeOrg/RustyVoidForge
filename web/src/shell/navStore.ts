import { create } from "zustand";
import { persist } from "zustand/middleware";

interface NavState {
  desktopOpen: boolean;
  setDesktopOpen: (open: boolean) => void;
}

// Only the desktop pane's state persists; the phone drawer always starts closed.
export const useNavStore = create<NavState>()(
  persist(
    (set) => ({
      desktopOpen: true,
      setDesktopOpen: (desktopOpen) => {
        set({ desktopOpen });
      },
    }),
    { name: "voidforge.nav" },
  ),
);
