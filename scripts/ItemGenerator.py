#!/usr/bin/env python3
"""ItemGenerator — add shop items from a CSV into the Labyrinth Rust codebase.

Patches marked regions in `src/items.rs` (and documents new items in README).

Each row may define:
  - up to 5 passives (`passive_1` … `passive_5`) as `stat=value`
  - an optional active (`has_active`, `active_cooldown`, `active_pseudocode`)
  - optional recipe `components` (semicolon-separated existing item names)

Active gameplay is stubbed: the hotkey starts the cooldown and leaves a
pseudocode comment for a later pass to turn into real effect code.

Usage:
  python3 scripts/ItemGenerator.py data/items.example.csv
  python3 scripts/ItemGenerator.py data/items.example.csv --dry-run
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path


REQUIRED_COLUMNS = ("Item_name", "cost")

PASSIVE_ALIASES = {
    "max_health": "max_health",
    "health": "max_health",
    "hp": "max_health",
    "max_mana": "max_mana",
    "mana": "max_mana",
    "mp": "max_mana",
    "mana_regen": "mana_regen",
    "mp_regen": "mana_regen",
    "attack_damage": "attack_damage",
    "damage": "attack_damage",
    "ad": "attack_damage",
    "armor": "armor",
    "magic_resist": "magic_resist",
    "mr": "magic_resist",
    "move_speed": "move_speed",
    "ms": "move_speed",
    "attack_speed_flat": "attack_speed_flat",
    "attack_speed": "attack_speed_flat",
    "as": "attack_speed_flat",
}

PASSIVE_LABELS = {
    "max_health": "HP",
    "max_mana": "Mana",
    "mana_regen": "Mana Regen",
    "attack_damage": "Attack Damage",
    "armor": "Armor",
    "magic_resist": "Magic Resist",
    "move_speed": "Move Speed",
    "attack_speed_flat": "Attack Speed",
}


@dataclass
class PassiveBonus:
    stat: str
    value: float


@dataclass
class ItemRow:
    name: str
    rust_id: str
    cost: int
    short_label: str
    description: str
    passives: list[PassiveBonus] = field(default_factory=list)
    has_active: bool = False
    active_cooldown: float | None = None
    active_pseudocode: str = ""
    component_names: list[str] = field(default_factory=list)
    component_ids: list[str] = field(default_factory=list)
    color: tuple[float, float, float] = (0.55, 0.55, 0.6)


def to_pascal_case(name: str) -> str:
    cleaned = re.sub(r"[^0-9A-Za-z]+", " ", name).strip()
    if not cleaned:
        raise ValueError(f"Cannot derive Rust identifier from empty name {name!r}")
    parts = cleaned.split()
    ident = "".join(p[:1].upper() + p[1:] for p in parts)
    if ident[0].isdigit():
        ident = f"Item{ident}"
    return ident


def parse_bool(value: str, default: bool = False) -> bool:
    raw = (value or "").strip().lower()
    if not raw:
        return default
    if raw in {"1", "true", "yes", "y"}:
        return True
    if raw in {"0", "false", "no", "n"}:
        return False
    raise ValueError(f"Invalid boolean: {value!r}")


def parse_float(value: str, default: float | None = None) -> float | None:
    raw = (value or "").strip()
    if not raw:
        return default
    return float(raw)


def f32(value: float) -> str:
    if float(value).is_integer():
        return f"{value:.1f}"
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    if "." not in text:
        text += ".0"
    return text


def short_label_from_name(name: str) -> str:
    parts = re.findall(r"[A-Za-z0-9]+", name)
    if not parts:
        return "IT"
    if len(parts) == 1:
        return parts[0][:2].upper()
    return "".join(p[0] for p in parts[:3]).upper()


def color_from_name(name: str) -> tuple[float, float, float]:
    digest = hashlib.sha256(name.encode("utf-8")).digest()
    # Keep mid-range colors so HUD icons stay readable.
    r = 0.25 + (digest[0] / 255.0) * 0.6
    g = 0.25 + (digest[1] / 255.0) * 0.6
    b = 0.25 + (digest[2] / 255.0) * 0.6
    return (r, g, b)


def parse_passive_cell(cell: str, row_no: int) -> PassiveBonus | None:
    raw = (cell or "").strip()
    if not raw:
        return None
    if "=" in raw:
        key, value = raw.split("=", 1)
    elif ":" in raw:
        key, value = raw.split(":", 1)
    else:
        raise SystemExit(
            f"Row {row_no}: passive must look like `stat=value` (got {raw!r})"
        )
    key = key.strip().lower()
    stat = PASSIVE_ALIASES.get(key)
    if not stat:
        raise SystemExit(
            f"Row {row_no}: unknown passive stat {key!r}. "
            f"Supported: {', '.join(sorted(set(PASSIVE_ALIASES.values())))}"
        )
    return PassiveBonus(stat=stat, value=float(value.strip()))


def parse_components(cell: str) -> list[str]:
    raw = (cell or "").strip()
    if not raw:
        return []
    parts = re.split(r"[;|]", raw)
    return [p.strip() for p in parts if p.strip()]


def build_description(
    passives: list[PassiveBonus],
    has_active: bool,
    active_pseudocode: str,
    explicit: str,
) -> str:
    if explicit.strip():
        return explicit.strip()
    bits: list[str] = []
    for bonus in passives:
        label = PASSIVE_LABELS.get(bonus.stat, bonus.stat)
        value = bonus.value
        if float(value).is_integer():
            bits.append(f"+{int(value)} {label}")
        else:
            bits.append(f"+{value:g} {label}")
    if has_active:
        active = active_pseudocode.strip() or "Active effect (stub)"
        bits.append(f"Active: {active}")
    return ". ".join(bits) if bits else "Generated item"


def merge_passives(passives: list[PassiveBonus]) -> dict[str, float]:
    merged: dict[str, float] = {}
    for bonus in passives:
        merged[bonus.stat] = merged.get(bonus.stat, 0.0) + bonus.value
    return merged


def region_body(text: str, name: str) -> str:
    match = re.search(
        rf"[ \t]*// <item_generator:{name}>\n(?P<body>.*?)[ \t]*// </item_generator:{name}>",
        text,
        re.DOTALL,
    )
    if not match:
        raise SystemExit(f"Missing item_generator marker region: {name}")
    return match.group("body")


def replace_region(text: str, name: str, new_body: str) -> str:
    pattern = re.compile(
        rf"([ \t]*// <item_generator:{name}>\n)(.*?)([ \t]*// </item_generator:{name}>)",
        re.DOTALL,
    )

    def _sub(match: re.Match[str]) -> str:
        body = new_body
        if body and not body.endswith("\n"):
            body += "\n"
        return f"{match.group(1)}{body}{match.group(3)}"

    updated, count = pattern.subn(_sub, text, count=1)
    if count != 1:
        raise SystemExit(f"Failed to update marker region: {name}")
    return updated


def append_before_end_marker(text: str, name: str, addition: str) -> str:
    body = region_body(text, name)
    if body and not body.endswith("\n"):
        body += "\n"
    if not addition.endswith("\n"):
        addition += "\n"
    return replace_region(text, name, body + addition)


def extract_existing_item_names(items_rs: str) -> set[str]:
    block = region_body(items_rs, "item_name")
    return {n.casefold() for n in re.findall(r'=>\s*"([^"]+)"', block)}


def extract_existing_item_ids(items_rs: str) -> dict[str, str]:
    """Map casefolded display name / rust id -> rust id."""
    names = region_body(items_rs, "item_name")
    mapping: dict[str, str] = {}
    for rust_id, display in re.findall(
        r"ItemId::([A-Za-z0-9]+)\s*=>\s*\"([^\"]+)\"", names
    ):
        mapping[display.casefold()] = rust_id
        mapping[rust_id.casefold()] = rust_id
    for rust_id in re.findall(
        r"ItemId::([A-Za-z0-9]+)", region_body(items_rs, "item_all")
    ):
        mapping[rust_id.casefold()] = rust_id
    return mapping


def rust_escape(text: str) -> str:
    return (
        text.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "")
    )


def passives_arm(item: ItemRow) -> str:
    merged = merge_passives(item.passives)
    if not merged:
        return f"            ItemId::{item.rust_id} => ItemPassives::default(),\n"
    lines = [f"            ItemId::{item.rust_id} => ItemPassives {{"]
    for stat, value in merged.items():
        lines.append(f"                {stat}: {f32(value)},")
    lines.append("                ..default()")
    lines.append("            },")
    return "\n".join(lines) + "\n"


def load_csv(path: Path) -> list[ItemRow]:
    with path.open(newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        if not reader.fieldnames:
            raise SystemExit(f"CSV has no header: {path}")
        fields = {f.strip(): f for f in reader.fieldnames if f}
        missing = [c for c in REQUIRED_COLUMNS if c not in fields]
        if missing:
            raise SystemExit(
                "CSV missing required columns: "
                + ", ".join(missing)
                + f"\nFound: {', '.join(reader.fieldnames)}"
            )

        rows: list[ItemRow] = []
        for i, raw in enumerate(reader, start=2):
            row = {k.strip(): (v or "").strip() for k, v in raw.items() if k}
            name = row.get("Item_name", "").strip()
            if not name or name.startswith("#"):
                continue

            passives: list[PassiveBonus] = []
            for n in range(1, 6):
                bonus = parse_passive_cell(row.get(f"passive_{n}", ""), i)
                if bonus:
                    passives.append(bonus)

            active_pseudocode = row.get("active_pseudocode", "")
            has_active = parse_bool(
                row.get("has_active", ""),
                default=bool(active_pseudocode.strip()),
            )
            active_cooldown = parse_float(row.get("active_cooldown", ""), None)
            if has_active and active_cooldown is None:
                active_cooldown = 30.0
            if has_active and not active_pseudocode.strip():
                raise SystemExit(
                    f"Row {i}: has_active requires active_pseudocode "
                    f"(used later to implement the effect)"
                )

            color = color_from_name(name)
            cr = parse_float(row.get("color_r", ""), None)
            cg = parse_float(row.get("color_g", ""), None)
            cb = parse_float(row.get("color_b", ""), None)
            if cr is not None and cg is not None and cb is not None:
                color = (cr, cg, cb)

            short = row.get("short_label") or short_label_from_name(name)
            description = build_description(
                passives, has_active, active_pseudocode, row.get("description", "")
            )

            rows.append(
                ItemRow(
                    name=name,
                    rust_id=to_pascal_case(name),
                    cost=int(float(row["cost"])),
                    short_label=short[:4],
                    description=description,
                    passives=passives,
                    has_active=has_active,
                    active_cooldown=active_cooldown,
                    active_pseudocode=active_pseudocode.strip(),
                    component_names=parse_components(row.get("components", "")),
                    color=color,
                )
            )
        return rows


def resolve_components(
    items: list[ItemRow], known: dict[str, str]
) -> None:
    pending = {item.name.casefold(): item.rust_id for item in items}
    pending.update({item.rust_id.casefold(): item.rust_id for item in items})
    lookup = dict(known)
    lookup.update(pending)

    for item in items:
        resolved: list[str] = []
        for comp in item.component_names:
            key = comp.casefold()
            rust_id = lookup.get(key) or lookup.get(to_pascal_case(comp).casefold())
            if not rust_id:
                raise SystemExit(
                    f"Item {item.name!r}: unknown component {comp!r}. "
                    "Components must name existing items or other rows in this CSV."
                )
            if rust_id == item.rust_id:
                raise SystemExit(f"Item {item.name!r}: cannot list itself as a component")
            resolved.append(rust_id)
        if len(resolved) > 5:
            raise SystemExit(
                f"Item {item.name!r}: at most 5 recipe components supported "
                f"(got {len(resolved)})"
            )
        item.component_ids = resolved


def apply_items(items_rs: str, readme: str, items: list[ItemRow]) -> tuple[str, str]:
    for item in items:
        items_rs = append_before_end_marker(
            items_rs,
            "item_enum",
            f"    {item.rust_id},\n",
        )
        items_rs = append_before_end_marker(
            items_rs,
            "item_all",
            f"            ItemId::{item.rust_id},\n",
        )
        items_rs = append_before_end_marker(
            items_rs,
            "item_name",
            f'            ItemId::{item.rust_id} => "{rust_escape(item.name)}",\n',
        )
        items_rs = append_before_end_marker(
            items_rs,
            "item_short_label",
            f'            ItemId::{item.rust_id} => "{rust_escape(item.short_label)}",\n',
        )
        items_rs = append_before_end_marker(
            items_rs,
            "item_cost",
            f"            ItemId::{item.rust_id} => {item.cost},\n",
        )
        items_rs = append_before_end_marker(
            items_rs,
            "item_description",
            f'            ItemId::{item.rust_id} => "{rust_escape(item.description)}",\n',
        )
        r, g, b = item.color
        items_rs = append_before_end_marker(
            items_rs,
            "item_color",
            (
                f"            ItemId::{item.rust_id} => "
                f"Color::srgb({f32(r)}, {f32(g)}, {f32(b)}),\n"
            ),
        )
        items_rs = append_before_end_marker(
            items_rs,
            "item_passives",
            passives_arm(item),
        )

        if item.has_active:
            assert item.active_cooldown is not None
            items_rs = append_before_end_marker(
                items_rs,
                "active_cooldown",
                (
                    f"            ItemId::{item.rust_id} => "
                    f"Some({f32(item.active_cooldown)}),\n"
                ),
            )
            items_rs = append_before_end_marker(
                items_rs,
                "active_pseudocode",
                (
                    f'            ItemId::{item.rust_id} => '
                    f'Some("{rust_escape(item.active_pseudocode)}"),\n'
                ),
            )
            pseudo = rust_escape(item.active_pseudocode)
            items_rs = append_before_end_marker(
                items_rs,
                "active_arms",
                (
                    f"        ItemId::{item.rust_id} => {{\n"
                    f"            // ACTIVE_PSEUDOCODE: {pseudo}\n"
                    f"            // TODO: implement active from ItemGenerator "
                    f"active_pseudocode\n"
                    f"        }}\n"
                ),
            )

        if item.component_ids:
            joined = ", ".join(f"ItemId::{c}" for c in item.component_ids)
            items_rs = append_before_end_marker(
                items_rs,
                "recipe_components",
                f"            ItemId::{item.rust_id} => &[{joined}],\n",
            )

    readme = update_readme(readme, items)
    return items_rs, readme


def update_readme(readme: str, items: list[ItemRow]) -> str:
    # Append generated item notes under the Items bullet list.
    table = re.search(
        r"(### Items\n\n)(?P<body>(?:- .*\n)+)",
        readme,
    )
    if not table:
        print("warning: README Items section not found; skipping README update", file=sys.stderr)
        return readme

    lines = []
    for item in items:
        passive_n = len(item.passives)
        active = "active stub" if item.has_active else "passive-only"
        comps = (
            f"; components: {', '.join(item.component_names)}"
            if item.component_names
            else ""
        )
        lines.append(
            f"- **{item.name}** ({item.cost}g) — {passive_n} passive(s), {active}{comps}\n"
        )

    # Avoid duplicating if names already mentioned.
    body = table.group("body")
    additions = []
    for line in lines:
        name = line.split("**")[1]
        if name.casefold() in body.casefold():
            continue
        additions.append(line)
    if not additions:
        return readme
    new_body = body
    if not new_body.endswith("\n"):
        new_body += "\n"
    new_body += "".join(additions)
    return readme[: table.start("body")] + new_body + readme[table.end("body") :]


def write_pseudocode_files(repo: Path, items: list[ItemRow]) -> None:
    out_dir = repo / "data" / "item_actives"
    out_dir.mkdir(parents=True, exist_ok=True)
    for item in items:
        if not item.has_active:
            continue
        path = out_dir / f"{item.rust_id}.pseudo.txt"
        path.write_text(
            (
                f"Item: {item.name}\n"
                f"Rust id: ItemId::{item.rust_id}\n"
                f"Cooldown: {item.active_cooldown}\n"
                f"Pseudocode:\n{item.active_pseudocode}\n"
            ),
            encoding="utf-8",
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path, help="CSV file of items to add")
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Repository root (default: parent of scripts/)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print planned changes without writing files",
    )
    args = parser.parse_args()

    repo = args.repo_root.resolve()
    items_path = repo / "src" / "items.rs"
    readme_path = repo / "README.md"
    for path in (items_path, readme_path):
        if not path.exists():
            raise SystemExit(f"Expected file missing: {path}")

    rows = load_csv(args.csv)
    if not rows:
        print("No item rows found in CSV.")
        return 0

    items_rs = items_path.read_text(encoding="utf-8")
    readme = readme_path.read_text(encoding="utf-8")

    existing_names = extract_existing_item_names(items_rs)
    known_ids = extract_existing_item_ids(items_rs)

    to_add: list[ItemRow] = []
    for row in rows:
        if row.name.casefold() in existing_names:
            print(f"skip: item name already used: {row.name}")
            continue
        if row.rust_id.casefold() in known_ids:
            print(
                f"skip: Rust ItemId::{row.rust_id} already exists "
                f"(from name {row.name!r})"
            )
            continue
        if len(row.passives) > 5:
            raise SystemExit(f"{row.name}: at most 5 passives allowed")
        to_add.append(row)
        existing_names.add(row.name.casefold())
        known_ids[row.name.casefold()] = row.rust_id
        known_ids[row.rust_id.casefold()] = row.rust_id

    if not to_add:
        print("Nothing to add.")
        return 0

    resolve_components(to_add, known_ids)

    print("Will add:")
    for item in to_add:
        passive_bits = ", ".join(f"{p.stat}={p.value:g}" for p in item.passives) or "none"
        active = (
            f"active cd={item.active_cooldown} pseudo={item.active_pseudocode!r}"
            if item.has_active
            else "no active"
        )
        comps = ", ".join(item.component_ids) if item.component_ids else "none"
        print(
            f"  - {item.name} ({item.rust_id}) cost={item.cost} "
            f"passives=[{passive_bits}] {active}; components=[{comps}]"
        )

    new_items, new_readme = apply_items(items_rs, readme, to_add)

    if args.dry_run:
        print("Dry run — no files written.")
        return 0

    items_path.write_text(new_items, encoding="utf-8")
    readme_path.write_text(new_readme, encoding="utf-8")
    write_pseudocode_files(repo, to_add)
    print(f"Updated {items_path.relative_to(repo)}")
    print(f"Updated {readme_path.relative_to(repo)}")
    print("Wrote active pseudocode under data/item_actives/ (when applicable).")
    print("Active effects are stubs — implement from active_pseudocode later.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
