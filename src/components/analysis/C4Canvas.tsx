import { useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  EdgeLabelRenderer,
  getBezierPath,
  Handle,
  MiniMap,
  Panel,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import ELK, { type ElkNode, type ElkExtendedEdge } from "elkjs/lib/elk.bundled.js";
import "@xyflow/react/dist/style.css";
import type { C4Graph } from "../../bindings/C4Graph";
import type { C4Node } from "../../bindings/C4Node";
import {
  IconBox,
  IconCode,
  IconComponent,
  IconDatabase,
  IconGlobe,
  IconQueue,
  IconSystem,
  IconUser,
} from "../icons";

const elk = new ELK();

const LEAF_W = 224;
const LEAF_H = 88;

const KIND_LABEL: Record<string, string> = {
  person: "person",
  "external-system": "external system",
  system: "system",
  container: "container",
  component: "component",
  code: "code",
  "data-store": "data store",
  queue: "queue",
};

const KIND_ICON: Record<string, React.ReactElement> = {
  person: <IconUser />,
  "external-system": <IconGlobe />,
  system: <IconSystem />,
  container: <IconBox />,
  component: <IconComponent />,
  code: <IconCode />,
  "data-store": <IconDatabase />,
  queue: <IconQueue />,
};

/** Edge stroke follows the same change palette as the node stripes. */
const CHANGE_STROKE: Record<string, string> = {
  added: "var(--ok)",
  modified: "var(--warn)",
  removed: "var(--bad)",
  affected: "var(--text)",
};

/** Long labels turn the canvas into spaghetti — clamp, keep the protocol
 *  only when there's room for it. Models are also prompted to stay terse;
 *  this is the backstop for graphs generated before that rule. */
function edgeLabel(label: string, protocol: string | null): string {
  const base = label.length > 28 ? `${label.slice(0, 27).trimEnd()}…` : label;
  return protocol && base.length + protocol.length <= 34 ? `${base} (${protocol})` : base;
}

type C4FlowNode = Node<{ c4: C4Node; highlighted: boolean; isGroup: boolean }>;

/** Handle sides per role — ELK's routes draw the real paths; these only
 *  anchor the bezier fallback, where back-edges attach on the near side. */
const HANDLE_DEFS = (["t", "s"] as const).flatMap((role) =>
  (
    [
      ["l", Position.Left],
      ["r", Position.Right],
    ] as const
  ).map(([side, position]) => ({ id: `${role}-${side}`, role, position })),
);

function C4NodeCard({ data }: NodeProps<C4FlowNode>) {
  const { c4, highlighted, isGroup } = data;
  const classes = [
    "c4-node",
    `change-${c4.change}`,
    `kind-${c4.kind}`,
    highlighted ? "highlighted" : "",
    isGroup ? "c4-group" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={classes}>
      {HANDLE_DEFS.map((h) => (
        <Handle
          key={h.id}
          id={h.id}
          type={h.role === "t" ? "target" : "source"}
          position={h.position}
          className="c4-handle"
        />
      ))}
      <div className="c4-kind">
        {KIND_ICON[c4.kind] && <span className="c4-kind-icon">{KIND_ICON[c4.kind]}</span>}
        {KIND_LABEL[c4.kind] ?? c4.kind}
        {c4.technology ? ` · ${c4.technology}` : ""}
        {c4.change !== "unchanged" && <span className={`c4-change-tag ${c4.change}`}>{c4.change}</span>}
      </div>
      <div className="c4-name">{c4.name}</div>
      {!isGroup && c4.description && <div className="c4-desc">{c4.description}</div>}
    </div>
  );
}

const nodeTypes = { c4: C4NodeCard };

type Pt = { x: number; y: number };

/** Orthogonal polyline with rounded corners. */
function roundedPath(pts: Pt[], r = 10): string {
  let d = `M ${pts[0].x} ${pts[0].y}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const prev = pts[i - 1];
    const p = pts[i];
    const next = pts[i + 1];
    const v1 = { x: p.x - prev.x, y: p.y - prev.y };
    const v2 = { x: next.x - p.x, y: next.y - p.y };
    const l1 = Math.hypot(v1.x, v1.y) || 1;
    const l2 = Math.hypot(v2.x, v2.y) || 1;
    const r1 = Math.min(r, l1 / 2);
    const r2 = Math.min(r, l2 / 2);
    const a = { x: p.x - (v1.x / l1) * r1, y: p.y - (v1.y / l1) * r1 };
    const b = { x: p.x + (v2.x / l2) * r2, y: p.y + (v2.y / l2) * r2 };
    d += ` L ${a.x} ${a.y} Q ${p.x} ${p.y} ${b.x} ${b.y}`;
  }
  const last = pts[pts.length - 1];
  d += ` L ${last.x} ${last.y}`;
  return d;
}

/** Edge that follows ELK's obstacle-avoiding orthogonal route instead of a
 *  straight bezier through whatever sits between the endpoints. Falls back
 *  to a bezier when no route came back. */
function ElkEdge(props: EdgeProps) {
  const pts = ((props.data as { points?: Pt[] } | undefined)?.points ?? []) as Pt[];
  let path: string;
  let lx: number;
  let ly: number;
  if (pts.length >= 2) {
    path = roundedPath(pts);
    // Label on the longest segment — that's where the eye follows the line.
    let bi = 0;
    let best = 0;
    for (let i = 0; i < pts.length - 1; i++) {
      const len = Math.hypot(pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
      if (len > best) {
        best = len;
        bi = i;
      }
    }
    lx = (pts[bi].x + pts[bi + 1].x) / 2;
    ly = (pts[bi].y + pts[bi + 1].y) / 2;
  } else {
    const [p, labelX, labelY] = getBezierPath(props);
    path = p;
    lx = labelX;
    ly = labelY;
  }
  return (
    <>
      <BaseEdge id={props.id} path={path} style={props.style} markerEnd={props.markerEnd} />
      {props.label && (
        <EdgeLabelRenderer>
          <div
            className="c4-edge-label mono"
            style={{ transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)` }}
          >
            {props.label}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}

const edgeTypes = { elk: ElkEdge };

interface LaidOut {
  nodes: C4FlowNode[];
  edges: Edge[];
}

async function layout(graph: C4Graph, highlight: Set<string>): Promise<LaidOut> {
  const ids = new Set(graph.nodes.map((n) => n.id));
  const isParent = new Set(
    graph.nodes.map((n) => n.boundary).filter((b): b is string => !!b && ids.has(b)),
  );

  // Build the ELK hierarchy: children nested under their boundary node.
  const byParent = new Map<string | null, C4Node[]>();
  for (const n of graph.nodes) {
    const parent = n.boundary && ids.has(n.boundary) ? n.boundary : null;
    byParent.set(parent, [...(byParent.get(parent) ?? []), n]);
  }
  const toElk = (n: C4Node): ElkNode => {
    const children = byParent.get(n.id) ?? [];
    return {
      id: n.id,
      ...(children.length > 0
        ? {
            children: children.map(toElk),
            layoutOptions: { "elk.padding": "[top=52,left=18,bottom=18,right=18]" },
          }
        : { width: LEAF_W, height: LEAF_H }),
    };
  };
  const elkGraph: ElkNode = {
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "RIGHT",
      "elk.layered.spacing.nodeNodeBetweenLayers": "130",
      "elk.spacing.nodeNode": "48",
      "elk.hierarchyHandling": "INCLUDE_CHILDREN",
      "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
      // Route edges around nodes instead of through them, with all
      // coordinates in root space so nested endpoints line up.
      "elk.edgeRouting": "ORTHOGONAL",
      "elk.json.edgeCoords": "ROOT",
      "elk.spacing.edgeNode": "18",
      "elk.spacing.edgeEdge": "10",
    },
    children: (byParent.get(null) ?? []).map(toElk),
    edges: graph.edges.map(
      (e): ElkExtendedEdge => ({ id: e.id, sources: [e.source], targets: [e.target] }),
    ),
  };

  const laid = await elk.layout(elkGraph);

  // ELK's routed polyline per edge, in root coordinates.
  const routes = new Map<string, Pt[]>();
  for (const e of laid.edges ?? []) {
    const pts: Pt[] = [];
    for (const s of e.sections ?? []) {
      pts.push(s.startPoint, ...(s.bendPoints ?? []), s.endPoint);
    }
    if (pts.length >= 2) routes.set(e.id, pts);
  }

  const flowNodes: C4FlowNode[] = [];
  const walk = (elkNode: ElkNode, parentId?: string) => {
    for (const child of elkNode.children ?? []) {
      const c4 = graph.nodes.find((n) => n.id === child.id);
      if (!c4) continue;
      const group = isParent.has(c4.id);
      flowNodes.push({
        id: c4.id,
        type: "c4",
        position: { x: child.x ?? 0, y: child.y ?? 0 },
        ...(parentId ? { parentId, extent: "parent" as const } : {}),
        width: child.width ?? LEAF_W,
        height: child.height ?? LEAF_H,
        style: group ? { width: child.width, height: child.height } : undefined,
        data: { c4, highlighted: highlight.has(c4.id), isGroup: group },
        // Edges follow ELK's fixed routes; dragging nodes would strand them.
        draggable: false,
        connectable: false,
      });
      walk(child, c4.id);
    }
  };
  walk(laid);

  // Absolute node centers pick the fallback-bezier attachment side, so
  // back-edges leave and enter on the near side instead of sweeping around.
  const centerX = new Map<string, number>();
  for (const n of flowNodes) {
    const abs = absolutePosition(flowNodes, n.id);
    centerX.set(n.id, abs.x + (n.width ?? LEAF_W) / 2);
  }

  const edges: Edge[] = graph.edges.map((e) => {
    const changed = e.change !== "unchanged";
    const forward = (centerX.get(e.source) ?? 0) <= (centerX.get(e.target) ?? 0);
    return {
      id: e.id,
      type: "elk",
      data: { points: routes.get(e.id) },
      source: e.source,
      target: e.target,
      sourceHandle: forward ? "s-r" : "s-l",
      targetHandle: forward ? "t-l" : "t-r",
      label: edgeLabel(e.label, e.protocol),
      // One encoding, stated in the legend: dashes (animated) = the
      // interaction changed in this PR; width = crosses a boundary;
      // color follows the same change palette as the node stripes.
      animated: changed,
      className: [
        "c4-edge",
        e.crossesBoundary ? "crosses-boundary" : "",
        `change-${e.change}`,
        highlight.has(e.source) || highlight.has(e.target) ? "highlighted" : "",
      ]
        .filter(Boolean)
        .join(" "),
      style: {
        stroke: changed ? CHANGE_STROKE[e.change] ?? "var(--warn)" : "var(--line)",
        strokeWidth: e.crossesBoundary ? 2.25 : 1.25,
      },
    };
  });

  return { nodes: flowNodes, edges };
}

function Flow({
  laidOut,
  isDrillable,
  onDrill,
  onPeek,
}: {
  laidOut: LaidOut;
  isDrillable: (node: C4Node) => boolean;
  onDrill: (node: C4Node) => void;
  onPeek: (node: C4Node) => void;
}) {
  const { fitBounds } = useReactFlow();
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clicking navigates: drillable nodes dive a level; leaf nodes hand off
  // to the peek (which the parent gates to the code level).
  const activate = (node: Node) => {
    const { c4 } = node.data as C4FlowNode["data"];
    if (!isDrillable(c4)) {
      onPeek(c4);
      return;
    }
    // Zoom into the node first — the drill lands mid-animation so the
    // level change reads as diving in, not a hard cut.
    const abs = absolutePosition(laidOut.nodes, node.id);
    fitBounds(
      {
        x: abs.x,
        y: abs.y,
        width: node.width ?? LEAF_W,
        height: node.height ?? LEAF_H,
      },
      { duration: 380, padding: 0.4 },
    );
    setTimeout(() => onDrill(c4), 320);
  };

  return (
    <ReactFlow
      nodes={laidOut.nodes}
      edges={laidOut.edges}
      nodeTypes={nodeTypes}
      edgeTypes={edgeTypes}
      fitView
      // Never fit below readable size: long linear chains pan instead of
      // shrinking to dust, and tiny graphs don't blow up past 1:1.
      fitViewOptions={{ padding: 0.15, minZoom: 0.55, maxZoom: 1 }}
      minZoom={0.2}
      maxZoom={2}
      proOptions={{ hideAttribution: true }}
      onNodeClick={(_, node) => {
        // Delay so a double-click doesn't activate twice.
        if (clickTimer.current) clearTimeout(clickTimer.current);
        clickTimer.current = setTimeout(() => {
          clickTimer.current = null;
          activate(node);
        }, 260);
      }}
      onNodeDoubleClick={(_, node) => {
        if (clickTimer.current) {
          clearTimeout(clickTimer.current);
          clickTimer.current = null;
        }
        activate(node);
      }}
      colorMode="dark"
    >
      <Background variant={BackgroundVariant.Dots} gap={22} size={1} color="#2a3340" />
      <Panel position="bottom-left" className="c4-legend">
        <span>
          <i className="lg-line" /> existing dependency
        </span>
        <span>
          <i className="lg-line dashed" /> changed by this PR
        </span>
        <span>
          <i className="lg-line thick" /> crosses a boundary
        </span>
      </Panel>
      <MiniMap
        pannable
        zoomable
        className="c4-minimap"
        nodeColor={() => "#2a3340"}
        maskColor="rgba(14, 17, 22, 0.75)"
      />
    </ReactFlow>
  );
}

/** Child node positions are parent-relative; walk up for viewport coords. */
function absolutePosition(nodes: C4FlowNode[], id: string): { x: number; y: number } {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  let node = byId.get(id);
  let x = 0;
  let y = 0;
  while (node) {
    x += node.position.x;
    y += node.position.y;
    node = node.parentId ? byId.get(node.parentId) : undefined;
  }
  return { x, y };
}

export function C4Canvas({
  graph,
  highlightIds,
  isDrillable,
  onDrill,
  onPeek,
}: {
  graph: C4Graph;
  highlightIds: string[];
  isDrillable: (node: C4Node) => boolean;
  onDrill: (node: C4Node) => void;
  onPeek: (node: C4Node) => void;
}) {
  const highlight = useMemo(() => new Set(highlightIds), [highlightIds]);
  const [laidOut, setLaidOut] = useState<LaidOut | null>(null);

  useEffect(() => {
    let cancelled = false;
    void layout(graph, highlight).then((result) => {
      if (!cancelled) setLaidOut(result);
    });
    return () => {
      cancelled = true;
    };
  }, [graph, highlight]);

  if (!laidOut) {
    return <div className="canvas-loading">laying out…</div>;
  }

  return (
    <div className="c4-canvas">
      <ReactFlowProvider>
        <Flow laidOut={laidOut} isDrillable={isDrillable} onDrill={onDrill} onPeek={onPeek} />
      </ReactFlowProvider>
    </div>
  );
}
