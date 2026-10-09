// Боковая панель: запросы проекта (папки → файлы → запросы и сценарии) и переменные окружения.
import * as vscode from "vscode";
import type { Item, Var } from "./protocol";
import type { Server } from "./server";

export type Node =
  | { kind: "folder"; name: string; path: string; children: Node[] }
  | { kind: "file"; name: string; path: string; uri: string; children: Node[] }
  | { kind: "item"; item: Item };

function label(item: Item): string {
  return item.name ?? `${item.method} ${item.target}`;
}

/** Дерево из плоского списка: путь `users/create.outry` → папка `users` → файл → элементы. */
function tree(items: Item[]): Node[] {
  const top: Node[] = [];
  const folders = new Map<string, Node[]>();
  const files = new Map<string, Node[]>();
  const folder = (path: string): Node[] => {
    if (!path) {
      return top;
    }
    let children = folders.get(path);
    if (!children) {
      children = [];
      folders.set(path, children);
      const cut = path.lastIndexOf("/");
      folder(path.slice(0, Math.max(cut, 0))).push({ kind: "folder", name: path.slice(cut + 1), path, children });
    }
    return children;
  };
  for (const item of items) {
    let children = files.get(item.path);
    if (!children) {
      children = [];
      files.set(item.path, children);
      const cut = item.path.lastIndexOf("/");
      const name = item.path.slice(cut + 1);
      folder(item.path.slice(0, Math.max(cut, 0))).push({ kind: "file", name, path: item.path, uri: item.uri, children });
    }
    children.push({ kind: "item", item });
  }
  const sort = (nodes: Node[]) => {
    nodes.sort((a, b) => {
      if (a.kind === "item" || b.kind === "item") {
        return 0;
      }
      if (a.kind !== b.kind) {
        return a.kind === "folder" ? -1 : 1;
      }
      return a.name.localeCompare(b.name);
    });
    for (const n of nodes) {
      if (n.kind !== "item") {
        sort(n.children);
      }
    }
  };
  sort(top);
  return top;
}

export class RequestsView implements vscode.TreeDataProvider<Node> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  private roots: Node[] = [];

  constructor(server: Server) {
    server.onDidChange(() => {
      this.roots = tree(server.state?.items ?? []);
      this.changed.fire();
    });
  }

  getChildren(node?: Node): Node[] {
    if (!node) {
      return this.roots;
    }
    return node.kind === "item" ? [] : node.children;
  }

  getTreeItem(node: Node): vscode.TreeItem {
    if (node.kind !== "item") {
      const t = new vscode.TreeItem(node.name, vscode.TreeItemCollapsibleState.Expanded);
      t.contextValue = node.kind;
      t.id = `${node.kind}:${node.path}`;
      if (node.kind === "file") {
        t.resourceUri = vscode.Uri.parse(node.uri);
        t.collapsibleState = vscode.TreeItemCollapsibleState.Collapsed;
      } else {
        t.iconPath = vscode.ThemeIcon.Folder;
      }
      return t;
    }
    const { item } = node;
    const t = new vscode.TreeItem(label(item), vscode.TreeItemCollapsibleState.None);
    t.id = `item:${item.path}:${item.line}`;
    t.contextValue = item.flow ? "flow" : "request";
    t.description = item.flow ? "flow" : item.name ? `${item.method} ${item.target}` : undefined;
    t.tooltip = `${item.path}:${item.line}`;
    t.iconPath = new vscode.ThemeIcon(item.flow ? "run-all" : "arrow-swap", new vscode.ThemeColor(methodColor(item)));
    const at = new vscode.Position(item.line - 1, 0);
    t.command = {
      command: "vscode.open",
      title: "Open",
      arguments: [vscode.Uri.parse(item.uri), { selection: new vscode.Range(at, at) }],
    };
    return t;
  }
}

function methodColor(item: Item): string {
  if (item.flow) {
    return "charts.orange";
  }
  switch (item.method) {
    case "GET":
      return "charts.green";
    case "POST":
      return "charts.yellow";
    case "PUT":
      return "charts.blue";
    case "PATCH":
      return "charts.purple";
    case "DELETE":
      return "charts.red";
    default:
      return "foreground";
  }
}

const SOURCES: Record<string, string> = {
  override: "--var",
  saved: "saved by `save`",
  process_env: "OUTRY_* environment variable",
  env: "env.toml",
  secret: "system keychain",
  dynamic: "dynamic",
};

export class VariablesView implements vscode.TreeDataProvider<Var> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;

  constructor(private readonly server: Server) {
    server.onDidChange(() => this.changed.fire());
  }

  getChildren(node?: Var): Var[] {
    return node ? [] : (this.server.state?.vars ?? []);
  }

  getTreeItem(v: Var): vscode.TreeItem {
    const t = new vscode.TreeItem(v.name);
    t.description = v.value ?? "missing";
    const source = v.source ? SOURCES[v.source] ?? v.source : "not set";
    t.tooltip = new vscode.MarkdownString(`\`${v.name}\` — ${source}${v.secret ? " (secret, masked)" : ""}`);
    t.iconPath =
      v.value === null
        ? new vscode.ThemeIcon("warning", new vscode.ThemeColor("problemsWarningIcon.foreground"))
        : new vscode.ThemeIcon(v.secret ? "key" : v.source === "saved" ? "save" : "symbol-variable");
    return t;
  }
}
