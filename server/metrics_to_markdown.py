#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
from typing import Iterable, List, Tuple


def format_labels(labels: Iterable[Tuple[str, str]]) -> str:
    items = [f"{k}={v}" for k, v in labels]
    return ", ".join(items) if items else "—"


def render_section(kind: str, entries: List[dict]) -> str:
    if not entries:
        return ""
    title = kind.capitalize()
    lines = [
        f"## {title} Metrics",
        "",
        "| Metric | Labels | Value |",
        "| --- | --- | --- |",
    ]
    for entry in entries:
        metric = entry.get("metric", "")
        labels = entry.get("labels", [])
        value = entry.get("value", "")
        if isinstance(value, list):
            value = ", ".join(str(v) for v in value)
        lines.append(
            f"| `{metric}` | {format_labels(labels)} | `{value}` |",
        )
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert metrics.json into a Markdown summary."
    )
    parser.add_argument("metrics", type=Path, help="Path to metrics.json file")
    parser.add_argument("output", type=Path, help="Path to write Markdown to")
    parser.add_argument(
        "--block-number",
        type=str,
        default=None,
        help="Optional block number for the document title",
    )
    args = parser.parse_args()

    data = json.loads(args.metrics.read_text())
    sections = []
    for kind in ("gauge", "counter", "histogram"):
        entries = data.get(kind, [])
        if entries:
            sections.append(render_section(kind, entries))

    title_block = args.block_number or ""
    header = f"# Metrics {title_block}".strip()
    body = "\n\n".join(section for section in sections if section)
    if not body:
        body = "_No metrics available._"
    contents = f"{header}\n\n{body}\n"
    args.output.write_text(contents)


if __name__ == "__main__":
    main()
