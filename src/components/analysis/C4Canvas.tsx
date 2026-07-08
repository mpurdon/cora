import { useEffect, useMemo, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import ELK, { type ElkNode, type ElkExtendedEdge } from "elkjs/lib/elk.bundled.js";
import "@xyflow/react/dist/style.css";
import type { C4Graph } from "../../bindings/C4Graph";
import type { C4Node } from "../../bindings/C4Node";

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

type C4FlowNode = Node<{ c4: C4Node; highlighted: boolean; isGroup: boolean }>;

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
      <Handle type="target" position={Position.Left} className="c4-handle" />
      <div className="c4-kind">
        {KIND_LABEL[c4.kind] ?? c4.kind}
        {c4.technology ? ` · ${c4.technology}` : ""}
        {c4.change !== "unchanged" && <span className={`c4-change-tag ${c4.change}`}>{c4.change}</span>}
      </div>
      <div className="c4-name">{c4.name}</div>
      {!isGroup && c4.description && <div className="c4-desc">{c4.description}</div>}
      <Handle type="source" position={Position.Right} className="c4-handle" />
    </div>
  );
}

const nodeTypes = { c4: C4NodeCard };

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
      "elk.layered.spacing.nodeNodeBetweenLayers": "90",
      "elk.spacing.nodeNode": "40",
      "elk.hierarchyHandling": "INCLUDE_CHILDREN",
      "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
    },
    children: (byParent.get(null) ?? []).map(toElk),
    edges: graph.edges.map(
      (e): ElkExtendedEdge => ({ id: e.id, sources: [e.source], targets: [e.target] }),
    ),
  };

  const laid = await elk.layout(elkGraph);

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
        draggable: true,
        connectable: false,
      });
      walk(child, c4.id);
    }
  };
  walk(laid);

  const edges: Edge[] = graph.edges.map((e) => {
    const emphatic = e.crossesBoundary;
    return {
      id: e.id,
      source: e.source,
      target: e.target,
      label: e.label + (e.protocol ? ` (${e.protocol})` : ""),
      animated: emphatic && e.change !== "unchanged",
      className: [
        "c4-edge",
        emphatic ? "crosses-boundary" : "",
        `change-${e.change}`,
        highlight.has(e.source) || highlight.has(e.target) ? "highlighted" : "",
      ]
        .filter(Boolean)
        .join(" "),
      labelStyle: { fill: "var(--muted)", fontSize: 10, fontFamily: "var(--mono)" },
      labelBgStyle: { fill: "var(--ink-0)", fillOpacity: 0.9 },
      style: {
        stroke: emphatic ? "var(--chat)" : "var(--line)",
        strokeWidth: emphatic ? 2.25 : 1.25,
      },
    };
  });

  return { nodes: flowNodes, edges };
}

function Flow({
  laidOut,
  onNodeDoubleClick,
}: {
  laidOut: LaidOut;
  onNodeDoubleClick?: (node: C4Node) => void;
}) {
  const { fitBounds } = useReactFlow();
  return (
    <ReactFlow
      nodes={laidOut.nodes}
      edges={laidOut.edges}
      nodeTypes={nodeTypes}
      fitView
      fitViewOptions={{ padding: 0.15 }}
      minZoom={0.2}
      maxZoom={2}
      proOptions={{ hideAttribution: true }}
      onNodeDoubleClick={(_, node) => {
        if (!onNodeDoubleClick) return;
        const data = node.data as C4FlowNode["data"];
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
        setTimeout(() => onNodeDoubleClick(data.c4), 320);
      }}
      colorMode="dark"
    >
      <Background variant={BackgroundVariant.Dots} gap={22} size={1} color="#2a3340" />
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
  onNodeDoubleClick,
}: {
  graph: C4Graph;
  highlightIds: string[];
  onNodeDoubleClick?: (node: C4Node) => void;
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
        <Flow laidOut={laidOut} onNodeDoubleClick={onNodeDoubleClick} />
      </ReactFlowProvider>
    </div>
  );
}
