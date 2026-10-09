import { useMemo, useState, type MouseEvent } from "react";

interface Node {
  name: string;
  path: string;
  children: Node[];
  isFile: boolean;
  method: string;
  /** Имена запросов и сценариев файла (`Login`) — подпись вместо имени файла */
  names: string[];
  /** Подпись файла: имена запросов, иначе имя без префикса метода (`get-by-id` → `by-id`) */
  label: string;
}

const METHOD_ORDER = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const SHORT: Record<string, string> = { DELETE: "DEL", PATCH: "PTCH", OPTIONS: "OPT", FLOW: "FLOW" };
/** Фильтр показывается, когда файлов больше этого */
const FILTER_FROM = 8;

function buildTree(files: string[], methods: Record<string, string>, names: Record<string, string[]>): Node[] {
  const root: Node = { name: "", path: "", children: [], isFile: false, method: "", names: [], label: "" };
  for (const file of files) {
    let cur = root;
    const parts = file.split("/");
    parts.forEach((part, i) => {
      const isFile = i === parts.length - 1;
      let next = cur.children.find((c) => c.name === part && c.isFile === isFile);
      if (!next) {
        const method = isFile ? (methods[file] ?? "GET") : "";
        const own = isFile ? (names[file] ?? []) : [];
        next = { name: part, path: parts.slice(0, i + 1).join("/"), children: [], isFile, method, names: own, label: "" };
        cur.children.push(next);
      }
      cur = next;
    });
  }
  return compact(root.children, "");
}

/** `users/get.routy` → `users`, `users/get-by-id.routy` → `by-id`, `users/create.http` → `create`. */
function fileLabel(node: Node, parent: string): string {
  if (node.names.length) return node.names.join(", ");
  const base = node.name.replace(/\.(http|routy)$/, "");
  const m = node.method.toLowerCase();
  if (base === m) return parent || base;
  if (base.startsWith(m + "-")) return base.slice(m.length + 1);
  return base;
}

/**
 * Сжимает дерево: цепочка единственных папок — одна строка (`api / v1`),
 * папка с единственным файлом — сам файл (`auth/login/post.http` → `POST login`).
 */
function compact(nodes: Node[], parent: string): Node[] {
  const out = nodes.map((n): Node => {
    if (n.isFile) return { ...n, label: fileLabel(n, parent) };
    let dir = n;
    let name = n.name;
    while (dir.children.length === 1 && !dir.children[0].isFile) {
      dir = dir.children[0];
      name += " / " + dir.name;
    }
    if (dir.children.length === 1) {
      const file = dir.children[0];
      if (file.names.length) return { ...file, label: fileLabel(file, "") };
      const own = fileLabel(file, "");
      const label = own === file.method.toLowerCase() ? name : `${name} / ${own}`;
      return { ...file, label };
    }
    return { ...dir, name, children: compact(dir.children, dir.name) };
  });
  const rank = (m: string) => {
    const i = METHOD_ORDER.indexOf(m);
    return i < 0 ? METHOD_ORDER.length : i;
  };
  return out.sort(
    (a, b) =>
      Number(a.isFile) - Number(b.isFile) ||
      (a.isFile ? a.label : a.name).localeCompare(b.isFile ? b.label : b.name) ||
      rank(a.method) - rank(b.method),
  );
}

export interface TreeTarget {
  path: string;
  isFile: boolean;
  /** Файлов внутри (для каталога) */
  count: number;
}

export type OnMenu = (e: MouseEvent, target: TreeTarget) => void;

function countFiles(node: Node): number {
  return node.isFile ? 1 : node.children.reduce((n, c) => n + countFiles(c), 0);
}

interface Props {
  files: string[];
  methods: Record<string, string>;
  names: Record<string, string[]>;
  selected: string | null;
  onSelect: (path: string) => void;
  onMenu: OnMenu;
}

export function FileTree(props: Props) {
  const [filter, setFilter] = useState("");
  const filtering = filter.trim() !== "";
  const shown = useMemo(() => {
    const terms = filter.toLowerCase().split(/\s+/).filter(Boolean);
    return props.files.filter((f) => {
      const hay = `${props.methods[f] ?? ""} ${f} ${(props.names[f] ?? []).join(" ")}`.toLowerCase();
      return terms.every((t) => hay.includes(t));
    });
  }, [props.files, props.methods, props.names, filter]);
  const tree = useMemo(() => buildTree(shown, props.methods, props.names), [shown, props.methods, props.names]);
  if (props.files.length === 0) {
    return <p className="muted pad">No *.routy or *.http files</p>;
  }
  return (
    <>
      {(props.files.length > FILTER_FROM || filter) && (
        <input
          className="tree-filter"
          type="search"
          placeholder="Filter"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && setFilter("")}
        />
      )}
      {tree.length === 0 ? (
        <p className="muted pad">Nothing matches</p>
      ) : (
        <ul className="tree">
          {tree.map((n) => (
            <TreeNode key={n.path + (filtering ? ":f" : "")} node={n} {...props} />
          ))}
        </ul>
      )}
    </>
  );
}

function TreeNode(props: Omit<Props, "files" | "methods" | "names"> & { node: Node }) {
  const { node } = props;
  if (node.isFile) {
    return (
      <li>
        <button
          className={"tree-file" + (props.selected === node.path ? " active" : "")}
          onClick={() => props.onSelect(node.path)}
          onContextMenu={(e) => props.onMenu(e, { path: node.path, isFile: true, count: 1 })}
          title={node.path}
        >
          <span className={"tree-method m-" + node.method.toLowerCase()}>{SHORT[node.method] ?? node.method}</span>
          <span className="ellipsis">{node.label}</span>
        </button>
      </li>
    );
  }
  return (
    <li>
      <details open>
        <summary onContextMenu={(e) => props.onMenu(e, { path: node.path, isFile: false, count: countFiles(node) })}>
          <span className="ellipsis">{node.name}</span>
          <span className="muted">{countFiles(node)}</span>
        </summary>
        <ul className="tree">
          {node.children.map((c) => (
            <TreeNode key={c.path} {...props} node={c} />
          ))}
        </ul>
      </details>
    </li>
  );
}
