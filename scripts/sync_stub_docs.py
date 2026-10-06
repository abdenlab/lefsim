"""Copy the extension's docstrings into its type stubs.

The Rust doc comments in crates/lefsim-py are the source of truth: pyo3 makes
them the runtime docstrings (what `help()` shows). This script writes each one
into python/lefsim/_lefsim.pyi, which editors and the API reference read,
replacing or inserting only docstrings and leaving signatures alone.

    uv run --project python python scripts/sync_stub_docs.py          # update
    uv run --project python python scripts/sync_stub_docs.py --check  # verify

`--check` changes nothing and fails if a docstring is out of date, a public
item is undocumented in Rust, or the runtime and the stubs disagree on which
public names exist. The Python tests run it.
"""

import ast
import inspect
import pathlib
import sys

import lefsim._lefsim as ext

STUB = pathlib.Path(__file__).resolve().parents[1] / "python" / "lefsim" / "_lefsim.pyi"
LINE_LENGTH = 120


def runtime_doc(qualname: str) -> "str | None":
    obj = ext
    for part in qualname.split("."):
        obj = inspect.getattr_static(obj, part)
    doc = getattr(obj, "__doc__", None)
    return inspect.cleandoc(doc) if doc and doc.strip() else None


def public(name: str) -> bool:
    return not name.startswith("_")


def docstring_lines(doc: str, indent: str) -> "list[str]":
    if '"""' in doc:
        raise ValueError(f'a docstring contains """: {doc[:60]!r}')
    prefix = "r" if "\\" in doc else ""
    lines = doc.split("\n")
    if len(lines) == 1 and len(indent) + len(prefix) + len(doc) + 6 <= LINE_LENGTH:
        return [f'{indent}{prefix}"""{doc}"""']
    body = [f"{indent}{line}" if line else "" for line in lines[1:]]
    return [f'{indent}{prefix}"""{lines[0]}', *body, f'{indent}"""']


def has_docstring(node: "ast.ClassDef | ast.FunctionDef") -> bool:
    first = node.body[0]
    return isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant) and isinstance(first.value.value, str)


def plan(source: str) -> "tuple[list[tuple[int, int, list[str]]], list[str]]":
    """Edits (0-based start line, end line exclusive, new lines) and problems."""
    tree = ast.parse(source)
    lines = source.split("\n")
    edits: "list[tuple[int, int, list[str]]]" = []
    problems: "list[str]" = []

    def sync(node: "ast.ClassDef | ast.FunctionDef", qualname: str) -> None:
        doc = runtime_doc(qualname)
        current = ast.get_docstring(node) if has_docstring(node) else None
        if doc is None:
            problems.append(f"{qualname}: no docstring in the Rust bindings")
            return
        if current == doc:
            return
        indent = " " * (node.col_offset + 4)
        new = docstring_lines(doc, indent)
        first = node.body[0]
        if has_docstring(node):
            edits.append((first.lineno - 1, first.end_lineno, new))
        elif isinstance(node, ast.ClassDef):
            start = min([first.lineno, *(d.lineno for d in getattr(first, "decorator_list", []))]) - 1
            edits.append((start, start, [*new, ""]))
        else:
            # A body of just `...`, on its own line or after the signature.
            line = first.lineno - 1
            text = lines[line]
            if text.strip() == "...":
                edits.append((line, line + 1, new))
            else:
                head = text[: first.col_offset].rstrip()
                edits.append((line, line + 1, [head, *new]))

    def sync_attribute(node: ast.AnnAssign, following: "list[ast.stmt]", qualname: str) -> None:
        # An attribute's docstring is the string literal on the line after it.
        doc = runtime_doc(qualname)
        nxt = following[0] if following else None
        has = isinstance(nxt, ast.Expr) and isinstance(nxt.value, ast.Constant) and isinstance(nxt.value.value, str)
        current = inspect.cleandoc(nxt.value.value) if has else None
        if doc is None:
            problems.append(f"{qualname}: no docstring in the Rust bindings")
            return
        if current == doc:
            return
        new = docstring_lines(doc, " " * node.col_offset)
        if has:
            edits.append((nxt.lineno - 1, nxt.end_lineno, new))
        else:
            edits.append((node.end_lineno, node.end_lineno, new))

    stub_names = set()
    for node in tree.body:
        if isinstance(node, (ast.ClassDef, ast.FunctionDef)) and public(node.name):
            stub_names.add(node.name)
            sync(node, node.name)
        if isinstance(node, ast.ClassDef):
            members = set()
            for i, member in enumerate(node.body):
                if isinstance(member, ast.FunctionDef):
                    members.add(member.name)
                    if public(member.name):
                        sync(member, f"{node.name}.{member.name}")
                elif isinstance(member, ast.AnnAssign) and isinstance(member.target, ast.Name):
                    members.add(member.target.id)
                    if public(member.target.id):
                        sync_attribute(member, node.body[i + 1 : i + 2], f"{node.name}.{member.target.id}")
            runtime_members = {n for n in vars(getattr(ext, node.name)) if public(n)}
            for name in sorted(runtime_members - members):
                problems.append(f"{node.name}.{name}: in the extension but not in the stubs")

    for name in sorted({n for n in vars(ext) if public(n)} - stub_names):
        problems.append(f"{name}: in the extension but not in the stubs")
    return edits, problems


def apply(source: str, edits: "list[tuple[int, int, list[str]]]") -> str:
    lines = source.split("\n")
    for start, end, new in sorted(edits, reverse=True):
        lines[start:end] = new
    return "\n".join(lines)


def main() -> int:
    check = "--check" in sys.argv[1:]
    source = STUB.read_text()
    edits, problems = plan(source)
    for problem in problems:
        print(problem, file=sys.stderr)
    if check:
        if edits:
            print(
                f"{len(edits)} docstring(s) in {STUB.name} are out of date; "
                "run scripts/sync_stub_docs.py (or `make stubs`)",
                file=sys.stderr,
            )
        return 1 if edits or problems else 0
    if edits:
        STUB.write_text(apply(source, edits))
        print(f"updated {len(edits)} docstring(s) in {STUB.name}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
