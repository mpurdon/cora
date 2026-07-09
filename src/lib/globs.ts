/** Minimal glob matching for review-ignore patterns.
 *  Supports `*` (within a segment), `**` (across segments), and bare
 *  filenames matching at any depth. */
export function globToRegex(glob: string): RegExp {
  let pattern = "";
  let i = 0;
  while (i < glob.length) {
    if (glob.startsWith("**/", i)) {
      pattern += "(.*/)?";
      i += 3;
    } else if (glob.startsWith("**", i)) {
      pattern += ".*";
      i += 2;
    } else if (glob[i] === "*") {
      pattern += "[^/]*";
      i += 1;
    } else {
      pattern += glob[i].replace(/[.+^${}()|[\]\\]/, "\\$&");
      i += 1;
    }
  }
  // A bare filename or extension pattern matches at any depth.
  if (!glob.includes("/")) {
    pattern = `(.*/)?${pattern}`;
  }
  return new RegExp(`^${pattern}$`);
}

export function matchesAny(path: string, globs: string[]): boolean {
  return globs.some((g) => {
    const trimmed = g.trim();
    if (!trimmed) return false;
    try {
      return globToRegex(trimmed).test(path);
    } catch {
      return false;
    }
  });
}

/** Reading-order score: interfaces/source first, config later, tests last.
 *  Lower = review sooner. */
export function reviewOrderScore(path: string): number {
  const p = path.toLowerCase();
  let score = 0;
  if (/\.(test|spec)\.|__tests__|_test\.(go|py|rs)|\/tests?\//.test(p)) score += 30;
  if (/\.(json|ya?ml|toml|ini|cfg|conf|env)$|\/config\//.test(p)) score += 20;
  if (/\.(md|txt|rst)$/.test(p)) score += 25;
  if (/\.(css|scss|less)$/.test(p)) score += 10;
  // Interface-ish files float to the very top.
  if (/(types?|interface|schema|api|contract|proto)[^/]*$/.test(p)) score -= 5;
  return score;
}
