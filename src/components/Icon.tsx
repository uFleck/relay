import type { ReactNode } from "react";

/* Phosphor-style 24px line icons, inline so neither window needs the network. Stroke follows currentColor. */

const paths = {
  sun: <><circle cx="12" cy="12" r="4" /><path d="M12 2.5v2M12 19.5v2M4.6 4.6l1.4 1.4M18 18l1.4 1.4M2.5 12h2M19.5 12h2M4.6 19.4 6 18M18 6l1.4-1.4" /></>,
  tray: <><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><path d="M3.5 14h4.2l1.6 2.5h5.4l1.6-2.5h4.2" /></>,
  folders: <><path d="M3.5 7.5v10a1.5 1.5 0 0 0 1.5 1.5h12" /><path d="M6.5 5.5a1 1 0 0 1 1-1h3.5l2 2h6.5a1 1 0 0 1 1 1V15a1 1 0 0 1-1 1h-12a1 1 0 0 1-1-1z" /></>,
  robot: <><rect x="4.5" y="8" width="15" height="11" rx="2.5" /><path d="M12 8V4.5M9 13h.01M15 13h.01M9.5 16h5" /><circle cx="12" cy="4" r=".8" /></>,
  review: <><rect x="4" y="4" width="16" height="16" rx="2.5" /><path d="m8.5 12.2 2.4 2.4 4.6-5" /></>,
  gear: <><circle cx="12" cy="12" r="3" /><path d="M12 3.5v2.2M12 18.3v2.2M3.5 12h2.2M18.3 12h2.2M6 6l1.6 1.6M16.4 16.4 18 18M6 18l1.6-1.6M16.4 7.6 18 6" /></>,
  search: <><circle cx="11" cy="11" r="6" /><path d="m20 20-4.5-4.5" /></>,
  pr: <><circle cx="6.5" cy="6" r="2" /><circle cx="6.5" cy="18" r="2" /><circle cx="17.5" cy="18" r="2" /><path d="M6.5 8v8M17.5 16V9.5a2.5 2.5 0 0 0-2.5-2.5h-4M12.5 5 10.5 7l2 2" /></>,
  branch: <><circle cx="6.5" cy="5.5" r="2" /><circle cx="6.5" cy="18.5" r="2" /><circle cx="17.5" cy="7.5" r="2" /><path d="M6.5 7.5v9M17.5 9.5c0 4-11 3-11 7" /></>,
  thread: <><path d="M4.5 6a1.5 1.5 0 0 1 1.5-1.5h12A1.5 1.5 0 0 1 19.5 6v9a1.5 1.5 0 0 1-1.5 1.5H9l-4.5 3.5z" /><path d="M8.5 9h7M8.5 12h4.5" /></>,
  clickup: <path d="m5 15.5 3.3-2.6c2.2 2.8 5.2 2.8 7.4 0l3.3 2.6c-3.9 4.6-10.1 4.6-14 0zM7.5 10.5 12 6.5l4.5 4" />,
  github: <path d="M9 19.5c-3.5 1-3.5-1.8-5-2.2M15 21.5V18c0-1 .1-1.4-.5-2 2.8-.3 5.5-1.4 5.5-6a4.6 4.6 0 0 0-1.3-3.2 4.2 4.2 0 0 0-.1-3.2s-1.1-.3-3.5 1.3a12 12 0 0 0-6.2 0C6.5 3.3 5.4 3.6 5.4 3.6a4.2 4.2 0 0 0-.1 3.2A4.6 4.6 0 0 0 4 10c0 4.6 2.7 5.7 5.5 6-.6.6-.6 1.2-.5 2v3.5" />,
  refresh: <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4v4h-4" />,
  ext: <path d="M14 4.5h5.5V10M19.5 4.5 11 13M17 14v4.5a1 1 0 0 1-1 1H5.5a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1H10" />,
  x: <path d="M6 6l12 12M18 6 6 18" />,
  min: <path d="M6 12h12" />,
  max: <rect x="6" y="6" width="12" height="12" rx="1" />,
  restore: <><rect x="5.5" y="8.5" width="10" height="10" rx="1" /><path d="M8.5 8.5V6.5a1 1 0 0 1 1-1h8a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1h-2" /></>,
  diff: <path d="M8 4v7M4.5 7.5h7M4.5 17h7M15 4.5h4.5V20H15" />,
  dots: <><circle cx="6" cy="12" r=".9" /><circle cx="12" cy="12" r=".9" /><circle cx="18" cy="12" r=".9" /></>,
  check: <path d="m5 12.5 4.5 4.5L19 7.5" />,
  pencil: <path d="M15.5 4.5 19.5 8.5 9 19H5v-4z" />,
  slack: <><rect x="10" y="3.5" width="3.6" height="9" rx="1.8" /><rect x="10.4" y="11.5" width="3.6" height="9" rx="1.8" transform="rotate(90 12.2 16)" /></>,
  relay: <><circle cx="6" cy="17" r="2.6" /><circle cx="18" cy="7" r="2.6" /><path d="M8.6 17h3.4a3 3 0 0 0 3-3v-4a3 3 0 0 1 .4-1.5" /></>,
  pin: <path d="M9 3.5h6M10 3.5v6l-3.5 4h11L14 9.5v-6M12 13.5v7" />,
  moon: <path d="M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10z" />,
  clock: <><circle cx="12" cy="12" r="8" /><path d="M12 7.5V12l3 2" /></>,
  download: <path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19.5h14" />,
  grip: <><circle cx="9" cy="7" r=".8" /><circle cx="15" cy="7" r=".8" /><circle cx="9" cy="12" r=".8" /><circle cx="15" cy="12" r=".8" /><circle cx="9" cy="17" r=".8" /><circle cx="15" cy="17" r=".8" /></>,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof paths;

export function Icon({ name, small, className }: { name: IconName; small?: boolean; className?: string }) {
  return (
    <svg className={`i${small ? " s" : ""}${className ? ` ${className}` : ""}`} viewBox="0 0 24 24" aria-hidden="true">
      {paths[name]}
    </svg>
  );
}

/** The Relay mark: rounded square, a wire between two nodes, the far node lit. */
export function Mark({ size = 24 }: { size?: number }) {
  return (
    <svg viewBox="0 0 28 28" width={size} height={size} aria-hidden="true">
      <rect x="1" y="1" width="26" height="26" rx="7" fill="var(--accent-soft)" stroke="var(--accent)" strokeWidth="1.4" />
      <path d="M8.5 19h4a3 3 0 0 0 3-3v-4a3 3 0 0 1 3-3h1" fill="none" stroke="var(--accent)" strokeWidth="2" strokeLinecap="round" />
      <circle cx="8" cy="19" r="2.3" fill="var(--accent)" />
      <circle cx="20" cy="9" r="2.3" fill="var(--glow)" />
    </svg>
  );
}
