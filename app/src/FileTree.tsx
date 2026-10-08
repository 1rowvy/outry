import { useMemo, type MouseEvent } from "react";

interface Node {
  name: string;
  path: string;
  children: Node[];
  isFile: boolean;
}

function buildTree(files: string[]): Node[] {
  const root: Node = { name: "", path: "", children: [], isFile: false };
  for (const file of files) {
    let cur = root;
    const parts = file.split("/");
    parts.forEach((part, i) => {
      const isFile = i === parts.length - 1;
      let next = cur.children.find((c) => c.name === part && c.isFile === isFile);
      if (!next) {
        next = { name: part, path: parts.slice(0, i + 1).join("/"), children: [], isFile };
        cur.children.push(next);
      }
      cur = next;
    });
  }
  const sort = (nodes: Node[]) => {
    nodes.sort((a, b) => Number(a.isFile) - Number(b.isFile) || a.name.localeCompare(b.name));
    nodes.forEach((n) => sort(n.children));
  };
  sort(root.children);
  return root.children;
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
  selected: string | null;
  onSelect: (path: string) => void;
  onMenu: OnMenu;
}

export function FileTree(props: Props) {
  const tree = useMemo(() => buildTree(props.files), [props.files]);
  if (props.files.length === 0) {
    return <p className="muted pad">No *.http files</p>;
  }
  return <ul className="tree">{tree.map((n) => <TreeNode key={n.path} node={n} {...props} />)}</ul>;
}

function TreeNode(props: Omit<Props, "files"> & { node: Node }) {
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
          {node.name.replace(/\.http$/, "")}
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
