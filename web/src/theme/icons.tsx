import type { ReactNode } from "react";

// One hand-drawn set on a 24 grid with one stroke. Icons are decoration: the word beside
// each one carries the meaning, so every icon is hidden from assistive tech.

function Icon({ children, size = 20 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

export const MarkIcon = () => (
  <Icon>
    <path d="M12 2l8.5 5v10L12 22l-8.5-5V7z" />
    <circle cx="12" cy="12" r="3" />
  </Icon>
);

export const MenuIcon = () => (
  <Icon>
    <path d="M4 6h16M4 12h16M4 18h16" />
  </Icon>
);

export const LogoutIcon = () => (
  <Icon>
    <path d="M14 4h5v16h-5M10 8l-4 4 4 4M6 12h10" />
  </Icon>
);

export const PlanetsIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="6" />
    <path d="M3.5 15.5c-1.5 2.2 6 1.6 11-1.5s7.5-7.3 5.9-8.2" />
  </Icon>
);

export const FleetsIcon = () => (
  <Icon>
    <path d="M12 2.5l3.5 7.5-3.5 2-3.5-2z" />
    <path d="M8.5 10L5 17l7-3 7 3-3.5-7" />
    <path d="M12 14v7" />
  </Icon>
);

export const UniverseIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 3v3M12 18v3M3 12h3M18 12h3" />
    <circle cx="12" cy="12" r="2" />
  </Icon>
);

export const LeaderboardIcon = () => (
  <Icon>
    <path d="M4 20V13h4v7M10 20V8h4v12M16 20v-5h4v5" />
    <path d="M12 3l1 2 2 .3-1.5 1.4.4 2-1.9-1-1.9 1 .4-2L9 5.3 11 5z" />
  </Icon>
);

export const DrillIcon = () => (
  <Icon>
    <path d="M6 3h12l-2 5H8z" />
    <path d="M9 8l1.5 9L12 21l1.5-4L15 8" />
    <path d="M10 12h4" />
  </Icon>
);

export const RefineryIcon = () => (
  <Icon>
    <path d="M4 21V11l5 3V11l5 3V6h6v15z" />
    <path d="M16 6V3M19 6V4" />
  </Icon>
);

export const ShipyardIcon = () => (
  <Icon>
    <path d="M3 21h18M5 21V8l7-5 7 5v13" />
    <path d="M12 9l2.5 5h-5zM12 14v4" />
  </Icon>
);

export const ColonyShipIcon = () => (
  <Icon>
    <ellipse cx="12" cy="12" rx="8" ry="4" />
    <path d="M12 8V4M8 15l-2 4M16 15l2 4" />
    <circle cx="12" cy="12" r="1.5" />
  </Icon>
);

export const CargoVesselIcon = () => (
  <Icon>
    <path d="M3 10h14l4 2-4 2H3z" />
    <path d="M6 10V7h6v3M6 14v3h6v-3" />
  </Icon>
);

export const OreIcon = () => (
  <Icon>
    <path d="M4 15l4-8 6-2 6 6-3 8-9 1z" />
    <path d="M8 7l3 6 6-2M11 13l-3 7" />
  </Icon>
);

export const IngotIcon = () => (
  <Icon>
    <path d="M3 16l3-7h12l3 7z" />
    <path d="M6 9l2 7M18 9l-2 7" />
  </Icon>
);

export const EmptySlotIcon = () => (
  <Icon>
    <path d="M12 7v10M7 12h10" />
  </Icon>
);

export const WorkingIcon = () => (
  <Icon size={14}>
    <circle cx="12" cy="12" r="7" />
    <path d="M9 12l2 2 4-4" />
  </Icon>
);

export const ConstructingIcon = () => (
  <Icon size={14}>
    <path d="M4 20l8-16 8 16z" />
    <path d="M12 10v4M12 17v.5" />
  </Icon>
);

export const DemolishingIcon = () => (
  <Icon size={14}>
    <path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13" />
  </Icon>
);

export const HaltedIcon = () => (
  <Icon size={14}>
    <circle cx="12" cy="12" r="7" />
    <path d="M10 9v6M14 9v6" />
  </Icon>
);
