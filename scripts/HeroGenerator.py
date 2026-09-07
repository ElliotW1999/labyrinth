#!/usr/bin/env python3
"""HeroGenerator — add heroes from a CSV into the Labyrinth Rust codebase.

Reads rows describing hero kits and patches marked regions in:
  - src/heroes.rs   (HeroId enum, roster, blurbs, defs)
  - src/components.rs (AbilityId stubs + display names + ultimates)
  - README.md       (heroes table)

Ability implementations are left as Instant no-op stubs (castable, no effect).

Usage:
  python3 scripts/HeroGenerator.py data/heroes.csv
  python3 scripts/HeroGenerator.py data/heroes.csv --dry-run
  python3 scripts/HeroGenerator.py data/heroes.csv --repo-root .
"""

from __future__ import annotations

import argparse
import csv
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REQUIRED_COLUMNS = (
    "Hero_name",
    "is_melee",
    "Ability_Q_Name",
    "Ability_W_Name",
    "Ability_E_Name",
    "Ability_R_Name",
    "base_STR",
    "base_AGI",
    "base_INT",
    "STR_per_level",
    "AGI_per_level",
    "INT_per_level",
)

README_HEROES_TABLE = re.compile(
    r"(### Heroes\n\n\| Hero \| Primary \| Kit \|\n\| --- \| --- \| --- \|\n)"
    r"(?P<rows>(?:\|.*\|\n)+)",
    re.MULTILINE,
)


@dataclass
class HeroRow:
    name: str
    rust_id: str
    is_melee: bool
    ability_q: str
    ability_w: str
    ability_e: str
    ability_r: str
    ability_q_id: str
    ability_w_id: str
    ability_e_id: str
    ability_r_id: str
    base_str: float
    base_agi: float
    base_int: float
    str_per_level: float
    agi_per_level: float
    int_per_level: float
    primary: str
    blurb: str
    base_health: float
    base_health_regen: float
    base_mana: float
    base_mana_regen: float
    attack_damage: float
    attack_range: float
    base_attack_time: float
    attack_point: float
    attack_backswing: float
    armor: float
    magic_resist: float
    move_speed: float


def to_pascal_case(name: str) -> str:
    cleaned = re.sub(r"[^0-9A-Za-z]+", " ", name).strip()
    if not cleaned:
        raise ValueError(f"Cannot derive Rust identifier from empty name {name!r}")
    parts = cleaned.split()
    ident = "".join(p[:1].upper() + p[1:] for p in parts)
    if ident[0].isdigit():
        ident = f"Hero{ident}"
    return ident


def parse_bool(value: str) -> bool:
    v = value.strip().lower()
    if v in {"1", "true", "yes", "y", "melee"}:
        return True
    if v in {"0", "false", "no", "n", "ranged"}:
        return False
    raise ValueError(f"Invalid boolean value: {value!r}")


def parse_float(row: dict[str, str], key: str, default: float | None = None) -> float:
    raw = (row.get(key) or "").strip()
    if not raw:
        if default is None:
            raise ValueError(f"Missing required numeric column {key}")
        return default
    return float(raw)


def infer_primary(base_str: float, base_agi: float, base_int: float, explicit: str) -> str:
    if explicit:
        key = explicit.strip().upper()
        if key in {"STR", "STRENGTH", "S"}:
            return "STR"
        if key in {"AGI", "AGILITY", "A"}:
            return "AGI"
        if key in {"INT", "INTELLIGENCE", "I"}:
            return "INT"
        raise ValueError(f"Unknown primary attribute: {explicit!r}")
    best = max(
        (("STR", base_str), ("AGI", base_agi), ("INT", base_int)),
        key=lambda pair: pair[1],
    )
    return best[0]


def default_blurb(hero: str, primary: str, q: str, w: str, e: str, r: str) -> str:
    label = {"STR": "Str", "AGI": "Agi", "INT": "Int"}[primary]
    return f"{label} hero — {q}, {w}, {e}, {r}"


def unique_ability_id(
    display_name: str,
    hero_rust_id: str,
    existing: set[str],
    pending: set[str],
) -> str:
    base = to_pascal_case(display_name)
    if base not in existing and base not in pending:
        return base
    prefixed = f"{hero_rust_id}{base}"
    if prefixed not in existing and prefixed not in pending:
        return prefixed
    n = 2
    while True:
        candidate = f"{prefixed}{n}"
        if candidate not in existing and candidate not in pending:
            return candidate
        n += 1


def load_csv(path: Path) -> list[HeroRow]:
    with path.open(newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        if reader.fieldnames is None:
            raise SystemExit(f"CSV has no header: {path}")
        fields = {f.strip(): f for f in reader.fieldnames if f}
        missing = [c for c in REQUIRED_COLUMNS if c not in fields]
        if missing:
            raise SystemExit(
                "CSV missing required columns: "
                + ", ".join(missing)
                + f"\nFound: {', '.join(reader.fieldnames)}"
            )

        rows: list[HeroRow] = []
        for i, raw in enumerate(reader, start=2):
            row = {k.strip(): (v or "").strip() for k, v in raw.items() if k}
            name = row.get("Hero_name", "").strip()
            if not name or name.startswith("#"):
                continue
            is_melee = parse_bool(row["is_melee"])
            base_str = parse_float(row, "base_STR")
            base_agi = parse_float(row, "base_AGI")
            base_int = parse_float(row, "base_INT")
            primary = infer_primary(base_str, base_agi, base_int, row.get("primary", ""))
            q = row["Ability_Q_Name"]
            w = row["Ability_W_Name"]
            e = row["Ability_E_Name"]
            r = row["Ability_R_Name"]
            for slot_name, ability in (("Q", q), ("W", w), ("E", e), ("R", r)):
                if not ability:
                    raise SystemExit(f"Row {i}: Ability_{slot_name}_Name is required")
            attack_range_default = 1.8 if is_melee else 9.5
            attack_damage_default = 55.0 if is_melee else 48.0
            blurb = row.get("blurb") or default_blurb(name, primary, q, w, e, r)
            rust_id = to_pascal_case(name)
            rows.append(
                HeroRow(
                    name=name,
                    rust_id=rust_id,
                    is_melee=is_melee,
                    ability_q=q,
                    ability_w=w,
                    ability_e=e,
                    ability_r=r,
                    ability_q_id="",  # filled later
                    ability_w_id="",
                    ability_e_id="",
                    ability_r_id="",
                    base_str=base_str,
                    base_agi=base_agi,
                    base_int=base_int,
                    str_per_level=parse_float(row, "STR_per_level"),
                    agi_per_level=parse_float(row, "AGI_per_level"),
                    int_per_level=parse_float(row, "INT_per_level"),
                    primary=primary,
                    blurb=blurb,
                    base_health=parse_float(row, "base_health", 340.0 if is_melee else 300.0),
                    base_health_regen=parse_float(
                        row, "base_health_regen", 0.5 if is_melee else 0.35
                    ),
                    base_mana=parse_float(row, "base_mana", 90.0 if primary != "INT" else 140.0),
                    base_mana_regen=parse_float(
                        row, "base_mana_regen", 0.8 if primary != "INT" else 1.4
                    ),
                    attack_damage=parse_float(row, "attack_damage", attack_damage_default),
                    attack_range=parse_float(row, "attack_range", attack_range_default),
                    base_attack_time=parse_float(row, "base_attack_time", 1.7),
                    attack_point=parse_float(row, "attack_point", 0.3),
                    attack_backswing=parse_float(row, "attack_backswing", 0.35),
                    armor=parse_float(row, "armor", 2.0 if is_melee else 0.8),
                    magic_resist=parse_float(
                        row, "magic_resist", 0.7 if primary != "INT" else 1.2
                    ),
                    move_speed=parse_float(row, "move_speed", 11.5),
                )
            )
        return rows


def extract_existing_hero_names(heroes_rs: str) -> set[str]:
    block = region_body(heroes_rs, "hero_name")
    names = set(re.findall(r'=>\s*"([^"]+)"', block))
    return {n.casefold() for n in names}


def extract_existing_ability_ids(components_rs: str) -> set[str]:
    block = region_body(components_rs, "ability_enum")
    return set(re.findall(r"^\s*([A-Z][A-Za-z0-9]*)\s*,\s*$", block, re.MULTILINE))


def region_body(text: str, name: str) -> str:
    match = re.search(
        rf"[ \t]*// <hero_generator:{name}>\n(?P<body>.*?)[ \t]*// </hero_generator:{name}>",
        text,
        re.DOTALL,
    )
    if not match:
        raise SystemExit(f"Missing hero_generator marker region: {name}")
    return match.group("body")


def replace_region(text: str, name: str, new_body: str) -> str:
    pattern = re.compile(
        rf"([ \t]*// <hero_generator:{name}>\n)(.*?)([ \t]*// </hero_generator:{name}>)",
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
    if not body.endswith("\n"):
        body += "\n"
    if not addition.endswith("\n"):
        addition += "\n"
    return replace_region(text, name, body + addition)


def f32(value: float) -> str:
    # Keep a stable Rust float literal.
    if float(value).is_integer():
        return f"{value:.1f}"
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    if "." not in text:
        text += ".0"
    return text


def hero_def_arm(hero: HeroRow) -> str:
    return f"""            HeroId::{hero.rust_id} => HeroDef {{
                id: self,
                attributes: HeroAttributes {{
                    strength: {f32(hero.base_str)},
                    agility: {f32(hero.base_agi)},
                    intelligence: {f32(hero.base_int)},
                    str_per_level: {f32(hero.str_per_level)},
                    agi_per_level: {f32(hero.agi_per_level)},
                    int_per_level: {f32(hero.int_per_level)},
                }},
                base_health: {f32(hero.base_health)},
                base_health_regen: {f32(hero.base_health_regen)},
                base_mana: {f32(hero.base_mana)},
                base_mana_regen: {f32(hero.base_mana_regen)},
                combat: hero_combat(
                    {f32(hero.attack_damage)},
                    {f32(hero.attack_range)},
                    {f32(hero.base_attack_time)},
                    {f32(hero.attack_point)},
                    {f32(hero.attack_backswing)},
                    crate::facing::HERO_TURN_RATE,
                    {f32(hero.armor)},
                    {f32(hero.magic_resist)},
                    {f32(hero.move_speed)},
                ),
                abilities: [
                    AbilityId::{hero.ability_q_id},
                    AbilityId::{hero.ability_w_id},
                    AbilityId::{hero.ability_e_id},
                    AbilityId::{hero.ability_r_id},
                ],
            }},
"""


def update_readme(readme: str, heroes: Iterable[HeroRow]) -> str:
    match = README_HEROES_TABLE.search(readme)
    if not match:
        print("warning: README heroes table not found; skipping README update", file=sys.stderr)
        return readme

    existing_rows = match.group("rows")
    existing_names = {
        line.split("|")[1].strip().casefold()
        for line in existing_rows.strip().splitlines()
        if line.strip().startswith("|")
    }
    additions = []
    for hero in heroes:
        if hero.name.casefold() in existing_names:
            continue
        primary = {"STR": "Strength", "AGI": "Agility", "INT": "Intelligence"}[hero.primary]
        kit = (
            f"{hero.ability_q}, {hero.ability_w}, {hero.ability_e}, {hero.ability_r} "
            f"(stubs — implement later)"
        )
        additions.append(f"| {hero.name} | {primary} | {kit} |\n")
    if not additions:
        return readme
    new_rows = existing_rows
    if not new_rows.endswith("\n"):
        new_rows += "\n"
    new_rows += "".join(additions)
    return readme[: match.start("rows")] + new_rows + readme[match.end("rows") :]


def assign_ability_ids(heroes: list[HeroRow], existing_ability_ids: set[str]) -> None:
    pending: set[str] = set()
    for hero in heroes:
        hero.ability_q_id = unique_ability_id(
            hero.ability_q, hero.rust_id, existing_ability_ids, pending
        )
        pending.add(hero.ability_q_id)
        hero.ability_w_id = unique_ability_id(
            hero.ability_w, hero.rust_id, existing_ability_ids, pending
        )
        pending.add(hero.ability_w_id)
        hero.ability_e_id = unique_ability_id(
            hero.ability_e, hero.rust_id, existing_ability_ids, pending
        )
        pending.add(hero.ability_e_id)
        hero.ability_r_id = unique_ability_id(
            hero.ability_r, hero.rust_id, existing_ability_ids, pending
        )
        pending.add(hero.ability_r_id)


def apply_heroes(
    heroes_rs: str,
    components_rs: str,
    readme: str,
    heroes: list[HeroRow],
) -> tuple[str, str, str]:
    for hero in heroes:
        heroes_rs = append_before_end_marker(
            heroes_rs,
            "hero_enum",
            f"    /// Generated hero — abilities are stubs until implemented.\n    {hero.rust_id},\n",
        )
        heroes_rs = append_before_end_marker(
            heroes_rs,
            "hero_all",
            f"            HeroId::{hero.rust_id},\n",
        )
        heroes_rs = append_before_end_marker(
            heroes_rs,
            "hero_name",
            f'            HeroId::{hero.rust_id} => "{hero.name}",\n',
        )
        blurb = hero.blurb.replace('"', '\\"')
        heroes_rs = append_before_end_marker(
            heroes_rs,
            "hero_blurb",
            f'            HeroId::{hero.rust_id} => "{blurb}",\n',
        )
        heroes_rs = append_before_end_marker(
            heroes_rs,
            "hero_primary",
            f'            HeroId::{hero.rust_id} => "{hero.primary}",\n',
        )
        heroes_rs = append_before_end_marker(
            heroes_rs,
            "hero_def",
            hero_def_arm(hero),
        )

        components_rs = append_before_end_marker(
            components_rs,
            "ability_enum",
            (
                f"    // {hero.name} (generated stubs)\n"
                f"    {hero.ability_q_id},\n"
                f"    {hero.ability_w_id},\n"
                f"    {hero.ability_e_id},\n"
                f"    {hero.ability_r_id},\n"
            ),
        )
        components_rs = append_before_end_marker(
            components_rs,
            "ability_display_name",
            (
                f'            AbilityId::{hero.ability_q_id} => "{hero.ability_q}",\n'
                f'            AbilityId::{hero.ability_w_id} => "{hero.ability_w}",\n'
                f'            AbilityId::{hero.ability_e_id} => "{hero.ability_e}",\n'
                f'            AbilityId::{hero.ability_r_id} => "{hero.ability_r}",\n'
            ),
        )
        # Extend ultimate list: `A | B | C` -> `A | B | C | New`
        ult_body = region_body(components_rs, "ability_ultimates").rstrip()
        ult_body = ult_body.rstrip() + f" | AbilityId::{hero.ability_r_id}\n"
        components_rs = replace_region(components_rs, "ability_ultimates", ult_body)

    readme = update_readme(readme, heroes)
    return heroes_rs, components_rs, readme


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path, help="CSV file of heroes to add")
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
    heroes_path = repo / "src" / "heroes.rs"
    components_path = repo / "src" / "components.rs"
    readme_path = repo / "README.md"

    for path in (heroes_path, components_path, readme_path):
        if not path.exists():
            raise SystemExit(f"Expected file missing: {path}")

    rows = load_csv(args.csv)
    if not rows:
        print("No hero rows found in CSV.")
        return 0

    heroes_rs = heroes_path.read_text(encoding="utf-8")
    components_rs = components_path.read_text(encoding="utf-8")
    readme = readme_path.read_text(encoding="utf-8")

    existing_names = extract_existing_hero_names(heroes_rs)
    existing_ability_ids = extract_existing_ability_ids(components_rs)
    existing_rust_ids = set(
        re.findall(
            r"HeroId::([A-Za-z0-9]+)",
            region_body(heroes_rs, "hero_all"),
        )
    )

    to_add: list[HeroRow] = []
    for row in rows:
        if row.name.casefold() in existing_names:
            print(f"skip: hero name already used: {row.name}")
            continue
        if row.rust_id in existing_rust_ids:
            print(
                f"skip: Rust HeroId::{row.rust_id} already exists "
                f"(from name {row.name!r})"
            )
            continue
        to_add.append(row)
        existing_names.add(row.name.casefold())
        existing_rust_ids.add(row.rust_id)

    if not to_add:
        print("Nothing to add.")
        return 0

    assign_ability_ids(to_add, existing_ability_ids)

    print("Will add:")
    for hero in to_add:
        print(
            f"  - {hero.name} ({hero.rust_id}) "
            f"[{hero.primary}] melee={hero.is_melee} "
            f"abilities={hero.ability_q_id}/{hero.ability_w_id}/"
            f"{hero.ability_e_id}/{hero.ability_r_id}"
        )

    new_heroes, new_components, new_readme = apply_heroes(
        heroes_rs, components_rs, readme, to_add
    )

    if args.dry_run:
        print("Dry run — no files written.")
        return 0

    heroes_path.write_text(new_heroes, encoding="utf-8")
    components_path.write_text(new_components, encoding="utf-8")
    readme_path.write_text(new_readme, encoding="utf-8")
    print(f"Updated {heroes_path.relative_to(repo)}")
    print(f"Updated {components_path.relative_to(repo)}")
    print(f"Updated {readme_path.relative_to(repo)}")
    print("Ability gameplay stubs only — implement casts in abilities.rs later.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
