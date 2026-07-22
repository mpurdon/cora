import { useEffect, useState } from "react";
import type { ModelRate } from "../bindings/ModelRate";
import type { UsageGroup } from "../bindings/UsageGroup";
import type { UsageStats } from "../bindings/UsageStats";
import type { UsageTotals } from "../bindings/UsageTotals";
import { ipc } from "../lib/ipc";
import { formatTokens } from "./analysis/TraceSteps";

/** Money at the scale this actually lands: dollars for totals, fractions of a
 *  cent for a single request. Never rounds a real cost to "$0.00". */
function usd(n: number): string {
  if (n === 0) return "$0";
  if (n >= 100) return `$${Math.round(n).toLocaleString()}`;
  if (n >= 1) return `$${n.toFixed(2)}`;
  if (n >= 0.01) return `$${n.toFixed(3)}`;
  return `<$0.01`;
}

const tokensOf = (t: UsageTotals) => t.inputTokens + t.outputTokens;

const EMPTY_TOTALS: UsageTotals = {
  requests: 0,
  inputTokens: 0,
  outputTokens: 0,
  cacheRead: 0,
  cacheWrite: 0,
  cost: 0,
  unpricedRequests: 0,
};

/** Daily spend. One series, so one hue and no legend — the heading names it.
 *  Bars are anchored to the baseline with rounded data-ends and a 2px gap;
 *  only the peak is labelled, with the rest on hover. */
function DailySpend({ days }: { days: UsageGroup[] }) {
  const [hover, setHover] = useState<number | null>(null);
  if (days.length === 0) return null;
  // Only days with activity are stored. Plotting those back-to-back would put
  // a quiet fortnight and a busy one side by side at equal width, so refill
  // the calendar and let the empty days read as empty.
  const byKey = new Map(days.map((d) => [d.key, d]));
  const today = new Date();
  const recent: { key: string; label: string; totals: UsageTotals }[] = [];
  for (let back = 29; back >= 0; back--) {
    const d = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate() - back));
    const key = d.toISOString().slice(0, 10);
    recent.push(byKey.get(key) ?? { key, label: key, totals: EMPTY_TOTALS });
  }
  const peak = Math.max(...recent.map((d) => d.totals.cost));
  const peakIndex = recent.findIndex((d) => d.totals.cost === peak);
  const shown = hover != null ? recent[hover] : recent[peakIndex];

  return (
    <section className="usage-block">
      <div className="usage-block-head">
        <span className="eyebrow">Spend per day</span>
        <span className="usage-caption mono">
          {shown && shown.totals.requests > 0
            ? `${shown.label} · ${usd(shown.totals.cost)} · ${shown.totals.requests} requests`
            : shown
              ? `${shown.label} · nothing recorded`
              : ""}
        </span>
      </div>
      <div className="usage-bars" onMouseLeave={() => setHover(null)}>
        {recent.map((d, i) => {
          // A day with spend always gets a visible stub; a day with none gets
          // nothing, so quiet days are legible as quiet.
          const pct =
            d.totals.requests === 0 ? 0 : peak > 0 ? Math.max(3, (d.totals.cost / peak) * 100) : 3;
          return (
            <div
              key={d.key}
              className={`usage-bar-slot${hover === i ? " on" : ""}`}
              onMouseEnter={() => setHover(i)}
              title={`${d.label} · ${d.totals.requests === 0 ? "nothing" : usd(d.totals.cost)}`}
            >
              <span className="usage-bar" style={{ height: `${pct}%` }} />
            </div>
          );
        })}
      </div>
      <div className="usage-axis mono">
        <span>{recent[0]?.label}</span>
        <span>{recent[recent.length - 1]?.label}</span>
      </div>
    </section>
  );
}

function Tile({ value, label, hint }: { value: string; label: string; hint?: string }) {
  return (
    <div className="usage-tile">
      <span className="usage-tile-value mono">{value}</span>
      <span className="eyebrow">{label}</span>
      {hint && <span className="usage-tile-hint">{hint}</span>}
    </div>
  );
}

function GroupTable({
  title,
  groups,
  limit,
}: {
  title: string;
  groups: UsageGroup[];
  limit?: number;
}) {
  const rows = limit ? groups.slice(0, limit) : groups;
  if (rows.length === 0) return null;
  return (
    <section className="usage-block">
      <span className="eyebrow">{title}</span>
      <table className="usage-table">
        <thead>
          <tr>
            <th></th>
            <th className="num">requests</th>
            <th className="num">tokens</th>
            <th className="num">cost</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((g) => (
            <tr key={g.key}>
              <td className="usage-label">{g.label}</td>
              <td className="num mono">{g.totals.requests}</td>
              <td className="num mono">{formatTokens(tokensOf(g.totals))}</td>
              <td className="num mono">
                {g.totals.unpricedRequests === g.totals.requests ? "—" : usd(g.totals.cost)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** Rates are per model id. An inference-profile ARN names no model, so its
 *  price can only come from here — the table lists whatever the data has
 *  actually seen rather than asking you to name models up front. */
function Rates({ rates, onSaved }: { rates: ModelRate[]; onSaved: () => void }) {
  const [draft, setDraft] = useState<Record<string, { input: string; output: string }>>({});
  const [error, setError] = useState<string | null>(null);
  if (rates.length === 0) return null;

  const save = async (model: string) => {
    const d = draft[model];
    if (!d) return;
    setError(null);
    try {
      await ipc.setModelPrice(model, Number(d.input) || 0, Number(d.output) || 0);
      setDraft((prev) => {
        const next = { ...prev };
        delete next[model];
        return next;
      });
      onSaved();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <section className="usage-block">
      <span className="eyebrow">Rates — dollars per million tokens</span>
      <p className="usage-note">
        Inference-profile ARNs name no model, so Cora reads the mapping back out of the{" "}
        <span className="mono">env</span> block in your <span className="mono">~/.claude/*.json</span>{" "}
        settings — whatever <span className="mono">ANTHROPIC_DEFAULT_&lt;FAMILY&gt;_MODEL</span>{" "}
        points at gets that family's published rate. Anything you type here wins over that. Cache
        reads bill at a tenth of the input rate and cache writes at 1.25×, so both follow from it.
        Costs are recomputed every time this page loads — correcting a rate reprices all history.
      </p>
      {error && <div className="settings-error">{error}</div>}
      <table className="usage-table">
        <tbody>
          {rates.map((r) => {
            const d = draft[r.model] ?? {
              input: r.inputPerMtok ? String(r.inputPerMtok) : "",
              output: r.outputPerMtok ? String(r.outputPerMtok) : "",
            };
            const dirty = !!draft[r.model];
            return (
              <tr key={r.model}>
                <td className="usage-label mono usage-model">
                  {r.model}
                  {r.source === "unknown" && (
                    <span className="usage-flag">no rate — spend not counted</span>
                  )}
                  {r.source === "claude-settings" && (
                    <span className="usage-flag muted">identified as {r.note}</span>
                  )}
                  {r.source === "published" && <span className="usage-flag muted">{r.note}</span>}
                  {r.source === "yours" && <span className="usage-flag muted">your rate</span>}
                </td>
                <td className="num">
                  <input
                    className="usage-rate"
                    inputMode="decimal"
                    placeholder="in"
                    value={d.input}
                    onChange={(e) =>
                      setDraft((p) => ({ ...p, [r.model]: { ...d, input: e.target.value } }))
                    }
                  />
                </td>
                <td className="num">
                  <input
                    className="usage-rate"
                    inputMode="decimal"
                    placeholder="out"
                    value={d.output}
                    onChange={(e) =>
                      setDraft((p) => ({ ...p, [r.model]: { ...d, output: e.target.value } }))
                    }
                  />
                </td>
                <td className="num">
                  <button className="action-btn" disabled={!dirty} onClick={() => void save(r.model)}>
                    Save
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}

/** Long-term token and cost history for this org's Bedrock usage. */
export function UsageView() {
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    void ipc
      .getUsageStats()
      .then(setStats)
      .catch((e) => setError(String(e)));

  useEffect(load, []);

  if (error) return <div className="settings-error">{error}</div>;
  if (!stats) return <div className="drawer-empty">reading usage…</div>;
  if (stats.allTime.requests === 0) {
    return (
      <div className="placeholder">
        <p>No model requests recorded yet.</p>
        <p className="usage-note">
          Every analysis, code pass and assistant turn is recorded from here on — run an analysis
          and this fills in.
        </p>
      </div>
    );
  }

  const all = stats.allTime;
  const unpriced = all.unpricedRequests;
  const since = stats.since.slice(0, 10);

  return (
    <div className="usage-view">
      <p className="pane-intro">
        Every Bedrock request this org has made since {since}. Tokens are measured; costs are
        computed from the rates below.
      </p>

      {unpriced > 0 && (
        <div className="usage-warn">
          {unpriced} of {all.requests} requests use a model with no rate — their tokens are counted
          but their cost is not. Set a rate under <strong>Rates</strong> below.
        </div>
      )}

      <div className="usage-tiles">
        <Tile value={usd(all.cost)} label="all-time spend" hint={`${all.requests} requests`} />
        <Tile
          value={usd(stats.last30Days.cost)}
          label="last 30 days"
          hint={`${stats.last30Days.requests} requests`}
        />
        <Tile
          value={usd(stats.costPerPr)}
          label="average per PR"
          hint={`${formatTokens(stats.tokensPerPr)} tokens · ${stats.prs} PRs`}
        />
        <Tile
          value={usd(stats.byPr[0]?.totals.cost ?? 0)}
          label="most expensive PR"
          hint={stats.byPr[0]?.label}
        />
        <Tile
          value={usd(stats.cacheSavings)}
          label="saved by prompt caching"
          hint={`${formatTokens(all.cacheRead)} tokens read from cache`}
        />
        <Tile
          value={formatTokens(all.inputTokens + all.outputTokens)}
          label="tokens all time"
          hint={`${formatTokens(all.inputTokens)} in / ${formatTokens(all.outputTokens)} out`}
        />
      </div>

      <DailySpend days={stats.byDay} />
      <GroupTable title="Most expensive PRs" groups={stats.byPr} limit={10} />
      <GroupTable title="By kind of work" groups={stats.byKind} />
      <GroupTable title="By model" groups={stats.byModel} />
      <Rates rates={stats.rates} onSaved={load} />
    </div>
  );
}
