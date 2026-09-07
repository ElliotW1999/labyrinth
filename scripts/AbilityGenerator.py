#!/usr/bin/env python3
"""AbilityGenerator — add abilities from a CSV into the Labyrinth Rust codebase.

Patches:
  - `src/components.rs` AbilityId enum (`hero_generator:ability_enum`)
  - `src/components.rs` GeneratedAbilityDef table (`ability_generator:defs`)
  - README notes
  - `data/ability_pseudos/<Id>.pseudo.txt` for later implementation

Ability gameplay remains stubbed (`abilities.rs` `_ => {}`) until pseudocode is
converted into real effect code.

Ability types: passive, untargeted, unit_target, target_area, target_point, toggle.

Usage:
  python3 scripts/AbilityGenerator.py data/abilities.example.csv
  python3 scripts/AbilityGenerator.py data/abilities.example.csv --dry-run
"""

from __future__ import annotations

import argparse
import csv
import re
import sys
from dataclasses import dataclass
from pathlib import Path


REQUIRED_COLUMNS = (
    "Ability_name",
    "ability_type",
    "pseudocode",
)

ABILITY_TYPES = {
    "passive": "Passive",
    "untargeted": "Untargeted",
    "unit_target": "UnitTarget",
    "unit": "UnitTarget",
    "target_area": "TargetArea",
    "area": "TargetArea",
    "aoe": "TargetArea",
    "target_point": "TargetPoint",
    "point": "TargetPoint",
    "ground": "TargetPoint",
    "toggle": "Toggle",
}

DAMAGE_TYPES = {
    "": "Magical",
    "magic": "Magical",
    "magical": "Magical",
    "physical": "Physical",
    "phys": "Physical",
}


@dataclass
class AbilityRow:
    name: str
    rust_id: str
    ability_type: str  # Rust AbilityType variant
    is_ultimate: bool
    max_rank: int
    cast_point: float
    cast_backswing: float
    mana_cost_base: float
    mana_cost_per_level: float
    damage_base: float
    damage_per_level: float
    cast_range_base: float
    cast_range_per_level: float
    aoe_radius_base: float
    aoe_radius_per_level: float
    cooldown_base: float
    cooldown_per_level: float
    cooldown_min: float
    damage_type: str
    pseudocode: str


def to_pascal_case(name: str) -> str:
    cleaned = re.sub(r"[^0-9A-Za-z]+", " ", name).strip()
    if not cleaned:
        raise ValueError(f"Cannot derive Rust identifier from empty name {name!r}")
    parts = cleaned.split()
    ident = "".join(p[:1].upper() + p[1:] for p in parts)
    if ident[0].isdigit():
        ident = f"Ability{ident}"
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


def parse_float(row: dict[str, str], key: str, default: float) -> float:
    raw = (row.get(key) or "").strip()
    if not raw:
        return default
    return float(raw)


def parse_int(row: dict[str, str], key: str, default: int) -> int:
    raw = (row.get(key) or "").strip()
    if not raw:
        return default
    return int(float(raw))


def f32(value: float) -> str:
    if float(value).is_integer():
        return f"{value:.1f}"
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    if "." not in text:
        text += ".0"
    return text


def rust_escape(text: str) -> str:
    return (
        text.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "")
    )


def region_body(text: str, prefix: str, name: str) -> str:
    match = re.search(
        rf"[ \t]*// <{prefix}:{name}>\n(?P<body>.*?)[ \t]*// </{prefix}:{name}>",
        text,
        re.DOTALL,
    )
    if not match:
        raise SystemExit(f"Missing {prefix}:{name} marker region")
    return match.group("body")


def replace_region(text: str, prefix: str, name: str, new_body: str) -> str:
    pattern = re.compile(
        rf"([ \t]*// <{prefix}:{name}>\n)(.*?)([ \t]*// </{prefix}:{name}>)",
        re.DOTALL,
    )

    def _sub(match: re.Match[str]) -> str:
        body = new_body
        if body and not body.endswith("\n"):
            body += "\n"
        return f"{match.group(1)}{body}{match.group(3)}"

    updated, count = pattern.subn(_sub, text, count=1)
    if count != 1:
        raise SystemExit(f"Failed to update marker region {prefix}:{name}")
    return updated


def append_before_end_marker(text: str, prefix: str, name: str, addition: str) -> str:
    body = region_body(text, prefix, name)
    if body and not body.endswith("\n"):
        body += "\n"
    if not addition.endswith("\n"):
        addition += "\n"
    return replace_region(text, prefix, name, body + addition)


def extract_existing_ability_ids(components_rs: str) -> set[str]:
    block = region_body(components_rs, "hero_generator", "ability_enum")
    return set(re.findall(r"^\s*([A-Z][A-Za-z0-9]*)\s*,\s*$", block, re.MULTILINE))


def extract_existing_ability_names(components_rs: str) -> set[str]:
    names: set[str] = set()
    display = region_body(components_rs, "hero_generator", "ability_display_name")
    names.update(re.findall(r'=>\s*"([^"]+)"', display))
    # Also names from generated defs.
    if "// <ability_generator:defs>" in components_rs:
        defs = region_body(components_rs, "ability_generator", "defs")
        names.update(re.findall(r'display_name:\s*"([^"]+)"', defs))
    return {n.casefold() for n in names}


def defaults_for_type(ability_type: str, is_ultimate: bool) -> dict[str, float | int]:
    if ability_type == "Passive":
        return {
            "cast_point": 0.0,
            "cast_backswing": 0.0,
            "mana_cost_base": 0.0,
            "mana_cost_per_level": 0.0,
            "damage_base": 0.0,
            "damage_per_level": 0.0,
            "cast_range_base": 0.0,
            "cast_range_per_level": 0.0,
            "aoe_radius_base": 0.0,
            "aoe_radius_per_level": 0.0,
            "cooldown_base": 0.0,
            "cooldown_per_level": 0.0,
            "cooldown_min": 0.0,
            "max_rank": 4 if is_ultimate else 7,
        }
    if is_ultimate:
        return {
            "cast_point": 0.35,
            "cast_backswing": 0.45,
            "mana_cost_base": 100.0,
            "mana_cost_per_level": 20.0,
            "damage_base": 160.0,
            "damage_per_level": 55.0,
            "cast_range_base": 9.0,
            "cast_range_per_level": 0.5,
            "aoe_radius_base": 4.5,
            "aoe_radius_per_level": 0.4,
            "cooldown_base": 50.0,
            "cooldown_per_level": -4.0,
            "cooldown_min": 30.0,
            "max_rank": 4,
        }
    base = {
        "cast_point": 0.2,
        "cast_backswing": 0.25,
        "mana_cost_base": 40.0,
        "mana_cost_per_level": 6.0,
        "damage_base": 70.0,
        "damage_per_level": 25.0,
        "cast_range_base": 8.0,
        "cast_range_per_level": 0.5,
        "aoe_radius_base": 0.0,
        "aoe_radius_per_level": 0.0,
        "cooldown_base": 8.0,
        "cooldown_per_level": -0.35,
        "cooldown_min": 4.0,
        "max_rank": 7,
    }
    if ability_type == "UnitTarget":
        base.update(
            {
                "cast_range_base": 10.0,
                "damage_base": 90.0,
                "damage_per_level": 30.0,
                "aoe_radius_base": 0.0,
            }
        )
    elif ability_type == "TargetArea":
        base.update(
            {
                "aoe_radius_base": 3.0,
                "aoe_radius_per_level": 0.25,
                "damage_base": 100.0,
            }
        )
    elif ability_type == "TargetPoint":
        base.update({"aoe_radius_base": 0.75, "cast_range_base": 8.5})
    elif ability_type == "Toggle":
        base.update(
            {
                "cooldown_base": 1.0,
                "cooldown_per_level": 0.0,
                "cooldown_min": 0.5,
                "mana_cost_base": 10.0,
                "mana_cost_per_level": 2.0,
                "damage_base": 0.0,
            }
        )
    elif ability_type == "Untargeted":
        base.update({"cast_range_base": 0.0, "aoe_radius_base": 5.0})
    return base


def load_csv(path: Path) -> list[AbilityRow]:
    with path.open(newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        if not reader.fieldnames:
            raise SystemExit(f"CSV has no header: {path}")
        fields = {f.strip() for f in reader.fieldnames if f}
        missing = [c for c in REQUIRED_COLUMNS if c not in fields]
        if missing:
            raise SystemExit(
                "CSV missing required columns: "
                + ", ".join(missing)
                + f"\nFound: {', '.join(reader.fieldnames)}"
            )

        rows: list[AbilityRow] = []
        for i, raw in enumerate(reader, start=2):
            row = {k.strip(): (v or "").strip() for k, v in raw.items() if k}
            name = row.get("Ability_name", "").strip()
            if not name or name.startswith("#"):
                continue

            type_key = row["ability_type"].strip().lower().replace("-", "_").replace(" ", "_")
            ability_type = ABILITY_TYPES.get(type_key)
            if not ability_type:
                raise SystemExit(
                    f"Row {i}: unknown ability_type {row['ability_type']!r}. "
                    f"Expected one of: passive, untargeted, unit_target, "
                    f"target_area, target_point, toggle"
                )

            is_ultimate = parse_bool(row.get("is_ultimate", ""), default=False)
            defaults = defaults_for_type(ability_type, is_ultimate)

            # cost_per_level aliases mana_cost_per_level
            if (row.get("mana_cost_per_level") or "").strip():
                mana_per = parse_float(row, "mana_cost_per_level", float(defaults["mana_cost_per_level"]))
            else:
                mana_per = parse_float(
                    row, "cost_per_level", float(defaults["mana_cost_per_level"])
                )

            dmg_key = (row.get("damage_type") or "").strip().lower()
            damage_type = DAMAGE_TYPES.get(dmg_key)
            if damage_type is None:
                raise SystemExit(
                    f"Row {i}: unknown damage_type {row.get('damage_type')!r} "
                    "(use magical or physical)"
                )

            pseudocode = row["pseudocode"].strip()
            if not pseudocode:
                raise SystemExit(f"Row {i}: pseudocode is required")

            rows.append(
                AbilityRow(
                    name=name,
                    rust_id=to_pascal_case(name),
                    ability_type=ability_type,
                    is_ultimate=is_ultimate,
                    max_rank=parse_int(row, "max_rank", int(defaults["max_rank"])),
                    cast_point=parse_float(row, "cast_point", float(defaults["cast_point"])),
                    cast_backswing=parse_float(
                        row, "cast_backswing", float(defaults["cast_backswing"])
                    ),
                    mana_cost_base=parse_float(
                        row, "mana_cost_base", float(defaults["mana_cost_base"])
                    ),
                    mana_cost_per_level=mana_per,
                    damage_base=parse_float(row, "damage_base", float(defaults["damage_base"])),
                    damage_per_level=parse_float(
                        row, "damage_per_level", float(defaults["damage_per_level"])
                    ),
                    cast_range_base=parse_float(
                        row, "cast_range_base", float(defaults["cast_range_base"])
                    ),
                    cast_range_per_level=parse_float(
                        row,
                        "cast_range_per_level",
                        float(defaults["cast_range_per_level"]),
                    ),
                    aoe_radius_base=parse_float(
                        row, "aoe_radius_base", float(defaults["aoe_radius_base"])
                    ),
                    aoe_radius_per_level=parse_float(
                        row,
                        "aoe_radius_per_level",
                        float(defaults["aoe_radius_per_level"]),
                    ),
                    cooldown_base=parse_float(
                        row, "cooldown_base", float(defaults["cooldown_base"])
                    ),
                    cooldown_per_level=parse_float(
                        row, "cooldown_per_level", float(defaults["cooldown_per_level"])
                    ),
                    cooldown_min=parse_float(
                        row, "cooldown_min", float(defaults["cooldown_min"])
                    ),
                    damage_type=damage_type,
                    pseudocode=pseudocode,
                )
            )
        return rows


def def_arm(ability: AbilityRow) -> str:
    return f"""            AbilityId::{ability.rust_id} => Some(&GeneratedAbilityDef {{
                display_name: "{rust_escape(ability.name)}",
                ability_type: AbilityType::{ability.ability_type},
                is_ultimate: {"true" if ability.is_ultimate else "false"},
                max_rank: {ability.max_rank},
                cast_point: {f32(ability.cast_point)},
                cast_backswing: {f32(ability.cast_backswing)},
                mana_cost_base: {f32(ability.mana_cost_base)},
                mana_cost_per_level: {f32(ability.mana_cost_per_level)},
                damage_base: {f32(ability.damage_base)},
                damage_per_level: {f32(ability.damage_per_level)},
                cast_range_base: {f32(ability.cast_range_base)},
                cast_range_per_level: {f32(ability.cast_range_per_level)},
                aoe_radius_base: {f32(ability.aoe_radius_base)},
                aoe_radius_per_level: {f32(ability.aoe_radius_per_level)},
                cooldown_base: {f32(ability.cooldown_base)},
                cooldown_per_level: {f32(ability.cooldown_per_level)},
                cooldown_min: {f32(ability.cooldown_min)},
                damage_type: DamageType::{ability.damage_type},
                pseudocode: "{rust_escape(ability.pseudocode)}",
            }}),
"""


def apply_abilities(components_rs: str, readme: str, abilities: list[AbilityRow]) -> tuple[str, str]:
    for ability in abilities:
        components_rs = append_before_end_marker(
            components_rs,
            "hero_generator",
            "ability_enum",
            f"    // AbilityGenerator: {ability.name}\n    {ability.rust_id},\n",
        )
        components_rs = append_before_end_marker(
            components_rs,
            "ability_generator",
            "defs",
            def_arm(ability),
        )
        if ability.is_ultimate:
            # Keep hero_generator ultimates list in sync for non-generated fallbacks.
            ult_body = region_body(components_rs, "hero_generator", "ability_ultimates").rstrip()
            token = f"AbilityId::{ability.rust_id}"
            if token not in ult_body:
                ult_body = ult_body.rstrip() + f" | {token}\n"
                components_rs = replace_region(
                    components_rs, "hero_generator", "ability_ultimates", ult_body
                )

    readme = update_readme(readme, abilities)
    return components_rs, readme


def update_readme(readme: str, abilities: list[AbilityRow]) -> str:
    table = re.search(
        r"(### Ability ranks\n\n)(?P<body>(?:- .*\n)+)",
        readme,
    )
    if not table:
        print(
            "warning: README Ability ranks section not found; skipping README update",
            file=sys.stderr,
        )
        return readme

    body = table.group("body")
    additions = []
    for ability in abilities:
        line = (
            f"- **{ability.name}** ({ability.ability_type}"
            f"{', ultimate' if ability.is_ultimate else ''}) — stub; "
            f"see `data/ability_pseudos/{ability.rust_id}.pseudo.txt`\n"
        )
        if ability.name.casefold() in body.casefold():
            continue
        additions.append(line)
    if not additions:
        return readme
    new_body = body if body.endswith("\n") else body + "\n"
    new_body += "".join(additions)
    return readme[: table.start("body")] + new_body + readme[table.end("body") :]


def write_pseudocode_files(repo: Path, abilities: list[AbilityRow]) -> None:
    out_dir = repo / "data" / "ability_pseudos"
    out_dir.mkdir(parents=True, exist_ok=True)
    for ability in abilities:
        path = out_dir / f"{ability.rust_id}.pseudo.txt"
        path.write_text(
            (
                f"Ability: {ability.name}\n"
                f"Rust id: AbilityId::{ability.rust_id}\n"
                f"Type: {ability.ability_type}\n"
                f"Ultimate: {ability.is_ultimate}\n"
                f"Max rank: {ability.max_rank}\n"
                f"Cast point / backswing: {ability.cast_point} / {ability.cast_backswing}\n"
                f"Mana: {ability.mana_cost_base} + {ability.mana_cost_per_level}/level\n"
                f"Damage ({ability.damage_type}): {ability.damage_base} + {ability.damage_per_level}/level\n"
                f"Cast range: {ability.cast_range_base} + {ability.cast_range_per_level}/level\n"
                f"AoE: {ability.aoe_radius_base} + {ability.aoe_radius_per_level}/level\n"
                f"Cooldown: {ability.cooldown_base} + {ability.cooldown_per_level}/level "
                f"(min {ability.cooldown_min})\n"
                f"Pseudocode:\n{ability.pseudocode}\n"
            ),
            encoding="utf-8",
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path, help="CSV file of abilities to add")
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
    components_path = repo / "src" / "components.rs"
    readme_path = repo / "README.md"
    for path in (components_path, readme_path):
        if not path.exists():
            raise SystemExit(f"Expected file missing: {path}")

    rows = load_csv(args.csv)
    if not rows:
        print("No ability rows found in CSV.")
        return 0

    components_rs = components_path.read_text(encoding="utf-8")
    readme = readme_path.read_text(encoding="utf-8")

    existing_ids = extract_existing_ability_ids(components_rs)
    existing_names = extract_existing_ability_names(components_rs)

    to_add: list[AbilityRow] = []
    for row in rows:
        if row.name.casefold() in existing_names:
            print(f"skip: ability name already used: {row.name}")
            continue
        if row.rust_id in existing_ids:
            print(
                f"skip: Rust AbilityId::{row.rust_id} already exists "
                f"(from name {row.name!r})"
            )
            continue
        to_add.append(row)
        existing_names.add(row.name.casefold())
        existing_ids.add(row.rust_id)

    if not to_add:
        print("Nothing to add.")
        return 0

    print("Will add:")
    for ability in to_add:
        print(
            f"  - {ability.name} ({ability.rust_id}) type={ability.ability_type} "
            f"ultimate={ability.is_ultimate} ranks={ability.max_rank} "
            f"cd={ability.cooldown_base}/{ability.cooldown_per_level} "
            f"mana={ability.mana_cost_base}/{ability.mana_cost_per_level}"
        )

    new_components, new_readme = apply_abilities(components_rs, readme, to_add)

    if args.dry_run:
        print("Dry run — no files written.")
        return 0

    components_path.write_text(new_components, encoding="utf-8")
    readme_path.write_text(new_readme, encoding="utf-8")
    write_pseudocode_files(repo, to_add)
    print(f"Updated {components_path.relative_to(repo)}")
    print(f"Updated {readme_path.relative_to(repo)}")
    print("Wrote pseudocode under data/ability_pseudos/")
    print("Cast effects are stubs — implement from pseudocode in abilities.rs later.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
