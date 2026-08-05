/** Flags you can pin onto an activity row. Shared by the callout's feed menu
 *  and the main window's flagged rail section, so the vocabulary — and the
 *  order they're offered and displayed in — has one owner. */
export const FLAG_LABEL: Record<string, string> = {
  "must-review": "must review",
  "follow-up": "follow up with author",
};

/** Display order, most urgent first. */
export const FLAG_ORDER = Object.keys(FLAG_LABEL);

export function flagRank(flag: string): number {
  const i = FLAG_ORDER.indexOf(flag);
  return i === -1 ? FLAG_ORDER.length : i;
}
