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
