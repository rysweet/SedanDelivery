---
title: Graph Visualizer User Guide
description: Complete guide to the SedanDelivery Graph Visualizer SPA
type: howto
updated: 2026-02-27
---

# Graph Visualizer User Guide

This guide covers every part of the Graph Visualizer UI. It assumes the
application is running at `http://localhost:5173`. For setup instructions, see
[Getting Started](./getting-started.md).

## Contents

- [Interface Overview](#interface-overview)
- [Managing Graphs](#managing-graphs)
- [Navigating the Canvas](#navigating-the-canvas)
- [Creating Nodes and Edges](#creating-nodes-and-edges)
- [Editing Nodes and Edges](#editing-nodes-and-edges)
- [Expanding Neighborhoods](#expanding-neighborhoods)
- [Searching the Graph](#searching-the-graph)
- [Legend Panel](#legend-panel)
- [Inspector Panel](#inspector-panel)
- [Cypher Editor](#cypher-editor)
- [Schema View](#schema-view)
- [Exporting and Importing](#exporting-and-importing)
- [Keyboard Shortcuts](#keyboard-shortcuts)

## Interface Overview

```
+--Toolbar (top)-----------------------------------------------+
|  [Graph Selector v]  | [Select] [Pan]  | [+Node] [+Edge]     |
|  [Undo] [Redo] [Del] | [Layout v] [Fit]| [Query] [Schema]    |
|  [Export] [Import]   |                 |                      |
+--Search Bar--------------------------------------------------+
|  [Search by label or property...                         ]   |
+--Legend------+----------Canvas-----------+--Inspector--------+
|              |                           |                   |
|  Node Labels |   WebGL graph canvas      |  Selected node /  |
|  (color +    |                           |  edge properties  |
|   visibility)|                           |                   |
|              |   Context menu appears    |  Editable fields  |
|              |   on right-click          |                   |
+--------------+---------------------------+-------------------+
|  Status bar: [graph name]  [N nodes]  [E edges]  [zoom%]    |
+--------------------------------------------------------------+
```

The left Legend Panel, right Inspector Panel, and bottom Cypher/Schema sheet can
each be collapsed to expand the canvas area.

## Managing Graphs

### Switching graphs

The graph selector dropdown at the left of the toolbar lists all graphs in the
engine. Clicking a name loads that graph. The canvas and all panels update
immediately.

### Creating a graph

Click the **+** icon beside the graph selector. Enter a name in the dialog.
Valid characters: letters, digits, underscore, dot, hyphen (max 64 characters).

After creation, the new graph is selected automatically and the canvas is empty.

### Deleting a graph

Open the graph selector dropdown and click the trash icon beside the graph name.
A confirmation dialog appears before the delete is executed. Deletion is
permanent — the graph and all its nodes and edges are removed from cypher-core.

## Navigating the Canvas

The canvas renders using WebGL via Sigma.js and handles large graphs (100,000+
nodes) at interactive frame rates.

### Pan and zoom

| Action | Effect |
|---|---|
| Click and drag on empty canvas | Pan |
| Scroll wheel | Zoom in / out |
| Pinch (trackpad) | Zoom in / out |
| Double-click on canvas | Zoom in, centering on the click point |

### Fit view

Press **F** or click the **Fit** button in the toolbar to fit all nodes inside
the viewport. This is useful after adding many nodes or after applying a layout.

### Select mode vs. pan mode

The toolbar has two cursor modes:

- **Select** (arrow icon) — click nodes and edges to select them. The Inspector
  Panel opens automatically.
- **Pan** (hand icon) — click and drag pans the canvas without accidentally
  selecting elements.

Switch modes with the toolbar buttons or by pressing the corresponding shortcut.

### Layout algorithms

Three layout algorithms are available from the **Layout** dropdown:

| Algorithm | When to use |
|---|---|
| Force Atlas 2 | General-purpose. Runs continuously and separates clusters. Stop it once the graph stabilizes (click the stop button in the toolbar). |
| Circular | Arranges nodes in a circle. Fast and predictable for small graphs. |
| Random | Places nodes randomly. A starting point for manual positioning. |

Force Atlas 2 runs asynchronously. The toolbar shows a spinning indicator while
it is active. Click the stop button to freeze the current positions.

## Creating Nodes and Edges

### Creating a node

Click **+ Node** in the toolbar (or right-click on an empty area of the canvas
and choose **Add Node Here**). The dialog has two fields:

- **Labels** — space-separated list of label names, e.g. `Driver Vehicle`
- **Properties** — JSON object, e.g. `{"name": "Ana Lopez", "license": "WA-8821"}`

Labels and properties are both optional. Click **Create** to add the node to the
graph. It appears on the canvas and the Legend Panel updates with any new labels.

### Creating an edge

Click **+ Edge** in the toolbar. The dialog shows:

- **Source** — dropdown of all nodes in the current graph (searchable)
- **Target** — dropdown of all nodes
- **Type** — relationship type string, e.g. `DRIVES`
- **Properties** — optional JSON object

Alternatively, right-click a node on the canvas and choose **Add Edge From Here**
to pre-populate the source.

## Editing Nodes and Edges

### Via the Inspector Panel

Click any node or edge on the canvas to select it. The Inspector Panel on the
right shows all labels and properties as editable fields.

- Edit property values inline and press **Enter** or click outside the field to
  save.
- Add a new property by clicking **+ Add Property** at the bottom of the panel.
- Remove a property by clicking the **×** icon beside it.
- For nodes, add or remove labels using the label chip list at the top of the
  panel.

Changes are sent to gql-api immediately when you confirm. There is no "unsaved
state" in the Inspector — each edit is a PUT request.

### Via context menu

Right-click a node or edge and choose **Edit** to open a full edit dialog.
The dialog is equivalent to the create dialog, but pre-populated with the
current values.

### Deleting nodes and edges

Select one or more nodes/edges and press **Delete** (or **Backspace**). A
confirmation is shown for destructive batch deletes.

Alternatively, right-click and choose **Delete**. Deleting a node also deletes
all edges incident to it.

Undo is available via **Ctrl+Z** for a limited history depth.

## Expanding Neighborhoods

Right-click any node and select **Expand Neighborhood**. The BFF calls
`GET /api/graphs/{name}/neighbors/{nodeId}` and merges the returned subgraph
(connected nodes and edges) into the canvas.

Nodes already present in the canvas are not duplicated; only new nodes are
added. This lets you iteratively explore a graph starting from a focal node
without loading the entire dataset.

Double-clicking a node also triggers neighborhood expansion.

## Searching the Graph

The search bar below the toolbar accepts free-text queries. Matching is
performed client-side against all loaded nodes:

- Node labels: type part of a label name, e.g. `Ware`
- Property values: type part of a string property value, e.g. `Seattle`

Matching nodes are highlighted on the canvas and listed in a dropdown. Clicking
a result centers the view on that node and selects it.

Search is case-insensitive. Results update as you type. Press **Escape** or
click elsewhere to dismiss.

## Legend Panel

The Legend Panel on the left lists every distinct node label in the current
graph with a color swatch. Colors are assigned deterministically from the label
name using a fixed palette, so the same label always gets the same color across
sessions.

### Toggling label visibility

Click the eye icon beside a label to hide or show all nodes with that label on
the canvas. Hidden nodes are removed from the WebGL scene (not from the data).
The **Hide All / Show All** button at the top of the panel operates on all labels
at once.

This is useful for isolating a subset of a large graph — for example, hiding
`Driver` nodes to focus on `Route` and `Warehouse` relationships.

### Collapsing the panel

Click the **‹** icon at the top-right of the Legend Panel to collapse it and
reclaim horizontal space. Click the **›** icon on the collapsed edge to restore.

## Inspector Panel

The Inspector Panel on the right shows properties of the currently selected node
or edge.

**Node view**

- Labels displayed as chips at the top. Each chip has a color matching the
  Legend.
- All properties listed as key/value rows with inline editing.
- Node ID shown (read-only) at the bottom.

**Edge view**

- Relationship type shown at the top.
- Source and target node IDs shown as links — click to select that node.
- Properties listed with inline editing.
- Edge ID shown (read-only).

When nothing is selected, the panel shows a placeholder prompt.

## Cypher Editor

Click **Query** in the toolbar to open the Cypher editor as a bottom sheet (272px
height). The editor uses Monaco (VS Code's editor engine) with:

- Cypher keyword highlighting (MATCH, WHERE, RETURN, CREATE, MERGE, SET, DELETE,
  DETACH DELETE, WITH, UNWIND, UNION, ORDER BY, LIMIT, SKIP, DISTINCT, and all
  standard aggregation functions)
- Auto-complete for keywords
- Line numbers and bracket matching

### Running a query

Type a read-only Cypher statement and press **Ctrl+Enter** (or **Cmd+Enter** on
macOS), or click the **Run** button:

```cypher
MATCH (w:Warehouse)-[:SUPPLIES]->(r:Route)
RETURN w.name AS warehouse, count(r) AS routes
ORDER BY routes DESC
LIMIT 10
```

Results appear in a table below the editor showing column names and rows. The
elapsed time is shown in the status bar of the editor.

### Running a mutation

The editor detects write clauses (`CREATE`, `MERGE`, `DELETE`, `DETACH DELETE`,
`SET`, `REMOVE`) and routes the request to the mutate endpoint. The result shows
the mutation statistics:

```cypher
CREATE (d:Driver {name: 'Paulo Salave\'a', region: 'PNW'})
```

After a successful mutation, the canvas data is refetched automatically so new
nodes appear without a manual refresh.

### Clearing the editor

Click the **×** button to clear the editor content. The result table is also
cleared.

### Closing the editor

Click **Query** in the toolbar again, or press **Escape** with focus outside the
editor.

## Schema View

Click **Schema** in the toolbar to open the schema view as a bottom sheet. The
schema view calls `GET /api/graphs/{name}/schema` and displays:

- **Node labels** with counts, sorted alphabetically
- **Relationship types** with counts, sorted alphabetically

This is a read-only summary. Use it to understand the shape of a graph before
writing Cypher queries.

The schema view and Cypher editor cannot both be open simultaneously — opening
one closes the other.

## Exporting and Importing

### Exporting

Click **Export** in the toolbar. A dialog shows a preview of the JSON payload
and a **Download** button. The file is named `{graph-name}-export.json`.

The export format:

```json
{
  "nodes": [
    {
      "id": "1",
      "labels": ["Warehouse"],
      "properties": { "name": "Seattle-HQ" }
    }
  ],
  "edges": [
    {
      "id": "1",
      "type": "SUPPLIES",
      "source": "1",
      "target": "2",
      "properties": {}
    }
  ]
}
```

### Importing

Click **Import** in the toolbar. Paste or upload a JSON file in the export
format. Click **Import** to submit. The dialog shows `nodes_created` and
`edges_created` on success.

The target graph must already exist. The import creates new nodes and edges and
assigns new internal IDs — it does not deduplicate against existing data. If you
are reimporting an export, delete the old data first or import into a fresh graph.

## Keyboard Shortcuts

Shortcuts are active when keyboard focus is not in a text input, textarea, or
the Monaco editor (which handles its own shortcuts).

| Key | Action |
|---|---|
| `F` | Fit all nodes in view |
| `Delete` or `Backspace` | Delete selected node(s) or edge(s) |
| `Escape` | Deselect all / close context menu / close dialogs |
| `Ctrl+Z` (or `Cmd+Z`) | Undo last action |
| `Ctrl+Shift+Z` (or `Cmd+Shift+Z`) | Redo |

Inside the Cypher editor:

| Key | Action |
|---|---|
| `Ctrl+Enter` (or `Cmd+Enter`) | Execute query or mutation |

In dialogs and the graph selector:

| Key | Action |
|---|---|
| `Enter` | Confirm / submit |
| `Escape` | Cancel / close |
