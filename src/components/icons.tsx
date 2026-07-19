/** 16px stroke icons for icon-btns. Unicode glyphs (⟳ ▶ ⋯) render at wildly
 *  different visual sizes per font; fixed-viewBox SVGs keep every icon the
 *  same optical weight. */

function Svg({ children, ...rest }: React.SVGProps<SVGSVGElement>) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      {children}
    </svg>
  );
}

export function IconRefresh(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9" />
      <path d="M13.5 1.5v3h-3" />
    </Svg>
  );
}

export function IconExternal(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M6.5 3.5H3.5a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V9.5" />
      <path d="M9.5 2.5h4v4" />
      <path d="M13.5 2.5 7.5 8.5" />
    </Svg>
  );
}

export function IconChat(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M13.5 2.5h-11a1 1 0 0 0-1 1v7a1 1 0 0 0 1 1H5v2.8l3.4-2.8h5.1a1 1 0 0 0 1-1v-7a1 1 0 0 0-1-1Z" />
    </Svg>
  );
}

export function IconClipboard(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <rect x="3" y="3.5" width="10" height="11" rx="1.5" />
      <rect x="5.5" y="1.75" width="5" height="3.25" rx="1" />
      <path d="M5.75 8.25h4.5" />
      <path d="M5.75 10.75h3" />
    </Svg>
  );
}

export function IconEllipsis(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="3.25" cy="8" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="12.75" cy="8" r="1.1" fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function IconLink(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M6.5 9.5a3.2 3.2 0 0 0 4.6.3l2.2-2.2a3.2 3.2 0 1 0-4.5-4.5l-1 1" />
      <path d="M9.5 6.5a3.2 3.2 0 0 0-4.6-.3L2.7 8.4a3.2 3.2 0 1 0 4.5 4.5l1-1" />
    </Svg>
  );
}

export function IconCheck(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="m3 8.5 3.4 3.4L13 5" />
    </Svg>
  );
}

export function IconArrowUp(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M8 13V3.5" />
      <path d="m3.75 7.25 4.25-4.25 4.25 4.25" />
    </Svg>
  );
}

/* -- C4 node kinds --------------------------------------------------------- */

export function IconUser(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="8" cy="5" r="2.8" />
      <path d="M2.8 13.8a5.2 5.2 0 0 1 10.4 0" />
    </Svg>
  );
}

export function IconGlobe(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="8" cy="8" r="6.2" />
      <path d="M1.8 8h12.4M8 1.8c-3.4 3.6-3.4 8.8 0 12.4M8 1.8c3.4 3.6 3.4 8.8 0 12.4" />
    </Svg>
  );
}

export function IconSystem(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <rect x="1.8" y="3" width="12.4" height="10" rx="1.5" />
      <rect x="4.5" y="6" width="7" height="4" rx="1" />
    </Svg>
  );
}

export function IconBox(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M8 1.8 14 5v6l-6 3.2L2 11V5Z" />
      <path d="M2 5l6 3.2L14 5M8 8.2v6" />
    </Svg>
  );
}

export function IconComponent(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <rect x="2" y="2" width="5" height="5" rx="1" />
      <rect x="9" y="2" width="5" height="5" rx="1" />
      <rect x="2" y="9" width="5" height="5" rx="1" />
      <rect x="9" y="9" width="5" height="5" rx="1" />
    </Svg>
  );
}

export function IconCode(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="m5 4.5-3.5 3.5L5 11.5M11 4.5 14.5 8 11 11.5M9.2 3 6.8 13" />
    </Svg>
  );
}

export function IconDatabase(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <ellipse cx="8" cy="3.6" rx="5.6" ry="2.1" />
      <path d="M2.4 3.6v8.8c0 1.16 2.5 2.1 5.6 2.1s5.6-.94 5.6-2.1V3.6" />
      <path d="M2.4 8c0 1.16 2.5 2.1 5.6 2.1s5.6-.94 5.6-2.1" />
    </Svg>
  );
}

export function IconQueue(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M2 4.5h8M2 8h8M2 11.5h8" />
      <path d="m11.5 5.5 2.5 2.5-2.5 2.5" />
    </Svg>
  );
}

export function IconSparkle(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M8 1.8 9.6 6.4 14.2 8 9.6 9.6 8 14.2 6.4 9.6 1.8 8 6.4 6.4Z" />
      <path d="M12.9 1.9v2.6M11.6 3.2h2.6" />
    </Svg>
  );
}

/* -- activity-feed verbs (octicon-style git glyphs) ------------------------ */

export function IconGitCommit(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="8" cy="8" r="2.6" />
      <path d="M1.5 8h3.9M10.6 8h3.9" />
    </Svg>
  );
}

export function IconGitMerge(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="4" cy="3" r="1.7" />
      <circle cx="4" cy="13" r="1.7" />
      <circle cx="12" cy="8" r="1.7" />
      <path d="M4 4.7v6.6" />
      <path d="M4 5c.5 2.6 2.6 3 6.3 3" />
    </Svg>
  );
}

export function IconGitPr(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="4" cy="3" r="1.7" />
      <circle cx="4" cy="13" r="1.7" />
      <circle cx="12" cy="13" r="1.7" />
      <path d="M4 4.7v6.6" />
      <path d="M8.5 3H10a2 2 0 0 1 2 2v6.3" />
    </Svg>
  );
}

export function IconEye(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M1.5 8S4 3.75 8 3.75 14.5 8 14.5 8 12 12.25 8 12.25 1.5 8 1.5 8Z" />
      <circle cx="8" cy="8" r="1.9" />
    </Svg>
  );
}

export function IconCheckCircle(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="8" cy="8" r="6.25" />
      <path d="m5.25 8.2 1.9 1.9 3.6-4" />
    </Svg>
  );
}

export function IconXCircle(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <circle cx="8" cy="8" r="6.25" />
      <path d="m5.75 5.75 4.5 4.5M10.25 5.75l-4.5 4.5" />
    </Svg>
  );
}

export function IconAlertTriangle(props: React.SVGProps<SVGSVGElement>) {
  return (
    <Svg {...props}>
      <path d="M8 2.2 14.8 14H1.2Z" />
      <path d="M8 6.5v3.4" />
      <circle cx="8" cy="12.1" r="0.4" fill="currentColor" stroke="none" />
    </Svg>
  );
}
