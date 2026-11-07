#!/usr/bin/env python3
"""PreToolUse hook for documentation-only subagents (architect, reviewer): allow Write/Edit only on Markdown under docs/."""
import json
import os
import sys

data = json.load(sys.stdin)
path = (data.get("tool_input") or {}).get("file_path") or ""
root = os.path.realpath(os.environ.get("CLAUDE_PROJECT_DIR") or data.get("cwd") or os.getcwd())
target = os.path.realpath(os.path.join(root, path))
docs = os.path.join(root, "docs") + os.sep

if target.startswith(docs) and target.endswith(".md"):
    sys.exit(0)
print(f"this agent may only write Markdown under docs/ (blocked: {path})", file=sys.stderr)
sys.exit(2)
