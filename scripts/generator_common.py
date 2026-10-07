"""Shared CSV parsing, validation, and Rust emission for AbilityGenerator / ItemGenerator.

Both generators describe gameplay consequences with the same effect-row schema:

    <owner>_id,effect_index,trigger,effect_target,effect_type,effect_id,value,
    damage_type,duration,radius,amount,scaling_type,scaling_value

and translate each row into one `AbilityEffectEntry { trigger, target, effect }`
(see `src/abilities/definition.rs`). Abilities and items only differ in which
triggers/targets are meaningful for them.
"""

from __future__ import annotations

import csv
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

EFFECT_COLUMNS = (
    "effect_index",
    "trigger",
    "effect_target",
    "effect_type",
    "effect_id",
    "value",
    "damage_type",
    "duration",
    "radius",
    "amount",
    "scaling_type",
    "scaling_value",
)

ID_PATTERN = re.compile(r"^[a-z][a-z0-9_]*$")
CUSTOM_TRIGGER_PATTERN = re.compile(r"^on_[a-z0-9_]+$")

RUST_KEYWORDS = {
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait",
    "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do",
    "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
}

ABILITY_TRIGGERS = {
    "on_cast": "OnCast",
    "on_projectile_hit": "OnProjectileHit",
    "on_unit_contact": "OnUnitContact",
    "on_impact": "OnImpact",
    "on_channel_tick": "OnChannelTick",
    "on_channel_end": "OnChannelEnd",
    "on_expire": "OnExpire",
}

ITEM_TRIGGERS = {
    "passive": "Passive",
    "on_use": "OnUse",
    "on_attack": "OnAttack",
    "on_attack_hit": "OnAttackHit",
    "on_damage_taken": "OnDamageTaken",
    "on_kill": "OnKill",
    "on_death": "OnDeath",
}

EFFECT_TARGETS = {
    "caster",
    "owner",
    "cast_target",
    "trigger_unit",
    "attacker",
    "affected_units",
    "units_in_radius",
}

EFFECT_TYPES = {
    "damage",
    "heal",
    "apply_status",
    "remove_status",
    "dispel",
    "movement",
    "stat_modifier",
    "custom",
}

DAMAGE_TYPES = {"physical": "Physical", "magical": "Magical", "pure": "Pure"}

# effect_id → (StatusSpec expression, runtime status id, is_buff)
STATUSES = {
    "stun": ("StatusSpec::Stun", "stun", False),
    "root": ("StatusSpec::Root", "root", False),
    "silence": ("StatusSpec::Silence", "silence", False),
    "disarm": ("StatusSpec::Disarm", "disarm", False),
    "slow": (None, "slow", False),
    "phased": ("StatusSpec::Phased", "phased", True),
    "forceful": ("StatusSpec::Forceful", "forceful", True),
    "debuff_immunity": ("StatusSpec::DebuffImmunity", "spell_immunity", True),
    "spell_immunity": ("StatusSpec::DebuffImmunity", "spell_immunity", True),
}

MOVEMENTS = {"knockback": "Knockback", "pull": "Pull"}
DEFAULT_MOVEMENT_DURATION = 0.3

# Timed stat modifiers map onto `StatusSpec::Modifier` fields.
TIMED_STATS = {
    "armor": "armor",
    "magic_resist": "magic_resist",
    "move_speed": "move_speed",
    "attack_damage": "attack_damage",
}

# Permanent (item passive) stats map onto `PassiveStat`.
PASSIVE_STATS = {
    "max_health": "PassiveStat::MaxHealth",
    "health": "PassiveStat::MaxHealth",
    "health_regen": "PassiveStat::HealthRegen",
    "max_mana": "PassiveStat::MaxMana",
    "mana": "PassiveStat::MaxMana",
    "mana_regen": "PassiveStat::ManaRegen",
    "attack_damage": "PassiveStat::AttackDamage",
    "attack_speed": "PassiveStat::AttackSpeed",
    "armor": "PassiveStat::Armor",
    "magic_resist": "PassiveStat::MagicResist",
    "move_speed": "PassiveStat::MoveSpeed",
    "strength": "PassiveStat::Attribute(Attribute::Strength)",
    "agility": "PassiveStat::Attribute(Attribute::Agility)",
    "intelligence": "PassiveStat::Attribute(Attribute::Intelligence)",
}

SCALING_TYPES = {
    "strength": "EffectScaling::CasterAttribute(Attribute::Strength, {v})",
    "agility": "EffectScaling::CasterAttribute(Attribute::Agility, {v})",
    "intelligence": "EffectScaling::CasterAttribute(Attribute::Intelligence, {v})",
    "attack_damage": "EffectScaling::CasterAttackDamage({v})",
    "target_health_below": "EffectScaling::TargetHealthBelow({v})",
}


# --------------------------------------------------------------------------- errors


@dataclass
class Issue:
    file: str
    row: int | None
    field: str | None
    value: str | None
    message: str

    def __str__(self) -> str:
        where = self.file
        if self.row is not None:
            where += f":{self.row}"
        detail = ""
        if self.field:
            detail += f" [{self.field}"
            if self.value is not None:
                detail += f"={self.value!r}"
            detail += "]"
        return f"{where}{detail}: {self.message}"


@dataclass
class Report:
    errors: list[Issue] = field(default_factory=list)
    warnings: list[Issue] = field(default_factory=list)

    def error(self, file: str, row: int | None, fld: str | None, value: str | None, message: str) -> None:
        self.errors.append(Issue(file, row, fld, value, message))

    def warn(self, file: str, row: int | None, fld: str | None, value: str | None, message: str) -> None:
        self.warnings.append(Issue(file, row, fld, value, message))

    def print(self) -> None:
        for issue in self.warnings:
            print(f"warning: {issue}", file=sys.stderr)
        for issue in self.errors:
            print(f"error: {issue}", file=sys.stderr)

    def fail_if_errors(self) -> None:
        if self.errors:
            self.print()
            raise SystemExit(f"{len(self.errors)} validation error(s); nothing was written.")


# --------------------------------------------------------------------------- csv rows


@dataclass
class Row:
    """One CSV data row with location info for error messages."""

    file: str
    line: int
    cells: dict[str, str]

    def get(self, name: str) -> str:
        return self.cells.get(name, "").strip()


def read_csv(path: Path, required: tuple[str, ...], report: Report, legacy_hint: str = "") -> list[Row]:
    display = str(path)
    if not path.exists():
        report.error(display, None, None, None, "file not found")
        return []
    with path.open(newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        headers = [h.strip() for h in (reader.fieldnames or []) if h]
        missing = [c for c in required if c not in headers]
        if missing:
            message = f"missing required column(s): {', '.join(missing)}; expected headers: {','.join(required)}"
            if legacy_hint and legacy_hint in headers:
                message += " (this looks like the legacy generator format; see README)"
            report.error(display, 1, None, None, message)
            return []
        rows = []
        for line, raw in enumerate(reader, start=2):
            cells = {(k or "").strip(): (v or "").strip() for k, v in raw.items() if k}
            if not any(cells.values()) or next(iter(cells.values()), "").startswith("#"):
                continue
            rows.append(Row(display, line, cells))
        return rows


# --------------------------------------------------------------------------- values


def parse_number(
    row: Row,
    name: str,
    report: Report,
    *,
    levels: bool = False,
    integer: bool = False,
    minimum: float | None = None,
) -> list[float] | None:
    """Blank → None. `levels` allows `75|150|225|300`; returns one value per level."""
    raw = row.get(name)
    if not raw:
        return None
    parts = raw.split("|")
    if len(parts) > 1 and not levels:
        report.error(row.file, row.line, name, raw, "level-dependent values are not supported here")
        return None
    values: list[float] = []
    for part in parts:
        text = part.strip()
        if not text:
            report.error(row.file, row.line, name, raw, "empty level value")
            return None
        try:
            value = float(text)
        except ValueError:
            report.error(row.file, row.line, name, raw, f"invalid number {text!r}")
            return None
        if value != value or value in (float("inf"), float("-inf")):
            report.error(row.file, row.line, name, raw, "number must be finite")
            return None
        if integer and not value.is_integer():
            report.error(row.file, row.line, name, raw, "expected an integer")
            return None
        if minimum is not None and value < minimum:
            report.error(row.file, row.line, name, raw, f"must be >= {minimum:g}")
            return None
        values.append(value)
    return values


def parse_scalar(row: Row, name: str, report: Report, **kwargs) -> float | None:
    values = parse_number(row, name, report, **kwargs)
    return values[0] if values else None


def parse_bool(row: Row, name: str, report: Report) -> bool | None:
    raw = row.get(name).lower()
    if not raw:
        return None
    if raw in {"true", "1", "yes"}:
        return True
    if raw in {"false", "0", "no"}:
        return False
    report.error(row.file, row.line, name, row.get(name), "expected true or false")
    return None


def parse_enum(row: Row, name: str, allowed, report: Report, *, required: bool = True, lower: bool = True) -> str | None:
    raw = row.get(name)
    key = raw.lower() if lower else raw
    if not key:
        if required:
            report.error(row.file, row.line, name, raw, f"required; expected one of: {', '.join(allowed)}")
        return None
    if key not in allowed:
        report.error(row.file, row.line, name, raw, f"unknown value; expected one of: {', '.join(allowed)}")
        return None
    return key


def f32(value: float) -> str:
    if float(value).is_integer():
        return f"{value:.1f}"
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    return text if "." in text else text + ".0"


def rank_value(values: list[float] | None) -> str:
    """Prefer the existing linear representation; fall back to a per-level table."""
    if not values:
        return "RankValue::ZERO"
    if len(values) == 1 or all(v == values[0] for v in values):
        return f"RankValue::fixed({f32(values[0])})"
    step = values[1] - values[0]
    if all(abs((values[i + 1] - values[i]) - step) < 1e-6 for i in range(len(values) - 1)):
        return f"RankValue::from_level_1({f32(values[0])}, {f32(step)})"
    return "RankValue::levels(&[" + ", ".join(f32(v) for v in values) + "])"


def rust_str(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "")
    return f'"{escaped}"'


def to_pascal_case(ident: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in ident.split("_") if part)


def rust_fn_name(ident: str) -> str:
    return f"r#{ident}" if ident in RUST_KEYWORDS else ident


def check_id(row: Row, column: str, report: Report) -> str | None:
    raw = row.get(column)
    if not raw:
        report.error(row.file, row.line, column, raw, "required")
        return None
    if not ID_PATTERN.match(raw):
        report.error(row.file, row.line, column, raw, "ids must be snake_case: a-z, 0-9 and _ (starting with a letter)")
        return None
    return raw


# --------------------------------------------------------------------------- effects


@dataclass
class EffectRow:
    row: Row
    owner: str
    index: int
    trigger: str
    target: str
    effect_type: str


def parse_effect_rows(rows: list[Row], owner_column: str, owners: set[str], report: Report) -> dict[str, list[EffectRow]]:
    """Common checks: owner exists, effect_index unique per owner, enums known."""
    by_owner: dict[str, list[EffectRow]] = {}
    seen: dict[tuple[str, int], int] = {}
    for row in rows:
        owner = check_id(row, owner_column, report)
        index = parse_scalar(row, "effect_index", report, integer=True, minimum=0)
        if row.get("effect_index") == "":
            report.error(row.file, row.line, "effect_index", "", "required")
        trigger = row.get("trigger").lower()
        if not trigger:
            report.error(row.file, row.line, "trigger", "", "required")
        elif trigger not in ABILITY_TRIGGERS and trigger not in ITEM_TRIGGERS and not CUSTOM_TRIGGER_PATTERN.match(trigger):
            known = ", ".join([*ABILITY_TRIGGERS, *ITEM_TRIGGERS])
            report.error(row.file, row.line, "trigger", row.get("trigger"), f"unknown trigger; expected one of: {known}, or a custom on_<name> trigger")
            trigger = ""
        target = parse_enum(row, "effect_target", sorted(EFFECT_TARGETS), report)
        effect_type = parse_enum(row, "effect_type", sorted(EFFECT_TYPES), report)
        if owner is None or index is None or not trigger or target is None or effect_type is None:
            continue
        if owner not in owners:
            report.error(row.file, row.line, owner_column, owner, f"no {owner_column.removesuffix('_id')} with this id")
            continue
        key = (owner, int(index))
        if key in seen:
            report.error(row.file, row.line, "effect_index", row.get("effect_index"), f"duplicate effect_index for {owner!r} (first used on line {seen[key]})")
            continue
        seen[key] = row.line
        by_owner.setdefault(owner, []).append(EffectRow(row, owner, int(index), trigger, target, effect_type))
    for effects in by_owner.values():
        effects.sort(key=lambda e: e.index)
    return by_owner


@dataclass
class EffectContext:
    """What the owning ability/item allows, for static validation and defaults."""

    kind: str  # "ability" or "item"
    owner_id: str
    #: Number of values a level-dependent field must have (None = levels not allowed).
    levels: int | None
    #: `units_in_radius` center for each trigger.
    center_for: dict[str, str]
    #: Default `units_in_radius` team for abilities (None → derive from effect polarity).
    team: str | None = None
    #: Ability aoe_radius > 0 (lets units_in_radius fall back to `AreaRadius::Ability`).
    has_aoe: bool = False
    #: Static reasons a trigger/target combination can never resolve (filled by the generator).
    invalid_combo: dict[tuple[str, str], str] = field(default_factory=dict)
    #: Triggers that can never fire for this owner, with the reason.
    invalid_trigger: dict[str, str] = field(default_factory=dict)


def _levels(e: EffectRow, name: str, ctx: EffectContext, report: Report, **kwargs) -> list[float] | None:
    values = parse_number(e.row, name, report, levels=ctx.levels is not None, **kwargs)
    if values and len(values) > 1 and ctx.levels is not None and len(values) != ctx.levels:
        report.error(e.row.file, e.row.line, name, e.row.get(name), f"has {len(values)} levels but {e.owner!r} has {ctx.levels}")
        return None
    return values


def _unused(e: EffectRow, report: Report, *names: str) -> None:
    for name in names:
        if e.row.get(name):
            report.warn(e.row.file, e.row.line, name, e.row.get(name), f"ignored for effect_type {e.effect_type}")


def _require(e: EffectRow, name: str, report: Report, why: str) -> bool:
    if e.row.get(name):
        return True
    report.error(e.row.file, e.row.line, name, "", f"required: {why}")
    return False


def effect_to_rust(e: EffectRow, ctx: EffectContext, report: Report) -> tuple[str, str, str] | None:
    """Translate one effect row into Rust `(trigger, target, effect)` expressions, or None on error."""
    row = e.row
    errors_before = len(report.errors)

    # --- trigger
    if e.trigger in ABILITY_TRIGGERS:
        if ctx.kind == "item":
            report.error(row.file, row.line, "trigger", e.trigger, f"ability trigger; items use: {', '.join(ITEM_TRIGGERS)}")
            return None
        trigger = f"AbilityTrigger::{ABILITY_TRIGGERS[e.trigger]}"
    elif e.trigger in ITEM_TRIGGERS:
        if ctx.kind == "ability":
            report.error(row.file, row.line, "trigger", e.trigger, f"item trigger; abilities use: {', '.join(ABILITY_TRIGGERS)} or a custom on_<name>")
            return None
        trigger = f"AbilityTrigger::{ITEM_TRIGGERS[e.trigger]}"
    else:
        trigger = f'AbilityTrigger::Custom("{e.trigger}")'
    if e.trigger in ctx.invalid_trigger:
        report.error(row.file, row.line, "trigger", e.trigger, ctx.invalid_trigger[e.trigger])
        return None

    # --- effect
    value = _levels(e, "value", ctx, report)
    duration = _levels(e, "duration", ctx, report, minimum=0)
    amount = _levels(e, "amount", ctx, report)
    radius = _levels(e, "radius", ctx, report, minimum=0)
    scaling_value = parse_scalar(row, "scaling_value", report)
    scaling_type = parse_enum(row, "scaling_type", sorted(SCALING_TYPES), report, required=False)
    if len(report.errors) != errors_before:
        return None
    if scaling_type and scaling_value is None:
        report.error(row.file, row.line, "scaling_value", "", f"required when scaling_type is {scaling_type}")
        return None
    if scaling_value is not None and not scaling_type:
        report.error(row.file, row.line, "scaling_type", "", "required when scaling_value is set")
        return None
    scaling = "EffectScaling::None"
    if scaling_type:
        scaling = SCALING_TYPES[scaling_type].format(v=f32(scaling_value))

    def no_scaling() -> bool:
        if scaling_type:
            report.error(row.file, row.line, "scaling_type", scaling_type, f"scaling is only supported by damage and heal effects")
            return False
        return True

    effect_id = row.get("effect_id").lower()
    beneficial = False
    t = e.effect_type
    if t == "damage":
        ok = _require(e, "value", report, "damage amount")
        ok &= _require(e, "damage_type", report, "physical, magical, or pure")
        damage_type = parse_enum(row, "damage_type", list(DAMAGE_TYPES), report, required=False)
        if not ok or damage_type is None:
            return None
        _unused(e, report, "effect_id", "duration", "amount")
        effect = (
            f"AbilityEffect::Damage {{ amount: {rank_value(value)}, "
            f"damage_type: DamageType::{DAMAGE_TYPES[damage_type]}, scaling: {scaling} }}"
        )
    elif t == "heal":
        beneficial = True
        if not _require(e, "value", report, "heal amount"):
            return None
        _unused(e, report, "damage_type", "duration", "amount")
        if effect_id in ("", "health"):
            effect = f"AbilityEffect::Heal {{ amount: {rank_value(value)}, scaling: {scaling} }}"
        elif effect_id == "mana":
            if not no_scaling():
                return None
            effect = f"AbilityEffect::RestoreMana {{ amount: {rank_value(value)} }}"
        else:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "heal effect_id must be blank, health, or mana")
            return None
    elif t == "apply_status":
        if not _require(e, "effect_id", report, f"status to apply ({', '.join(STATUSES)})"):
            return None
        if effect_id not in STATUSES:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"unknown status; expected one of: {', '.join(STATUSES)}")
            return None
        if not _require(e, "duration", report, "status duration in seconds") or not no_scaling():
            return None
        spec, _, beneficial = STATUSES[effect_id]
        if effect_id == "slow":
            percent = amount or value
            if not percent:
                report.error(row.file, row.line, "amount", "", "required: slow percentage (e.g. 30)")
                return None
            if any(not 0 <= p <= 100 for p in percent):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), "slow percentage must be within 0..100")
                return None
            spec = f"StatusSpec::Slow {{ percent: {rank_value(percent)} }}"
            if amount:
                _unused(e, report, "value")
        else:
            _unused(e, report, "value", "amount")
        _unused(e, report, "damage_type")
        effect = f"AbilityEffect::ApplyStatus {{ status: {spec}, duration: {rank_value(duration)} }}"
    elif t in ("remove_status", "dispel"):
        if not _require(e, "effect_id", report, "buff, debuff, or a status id") or not no_scaling():
            return None
        _unused(e, report, "value", "damage_type", "duration", "amount")
        if effect_id in ("buff", "debuff"):
            beneficial = effect_id == "debuff"
            effect = f"AbilityEffect::Dispel {{ kind: StatusKind::{effect_id.capitalize()} }}"
        elif ID_PATTERN.match(effect_id):
            status_id = STATUSES[effect_id][1] if effect_id in STATUSES else effect_id
            beneficial = effect_id in STATUSES and not STATUSES[effect_id][2]
            effect = f'AbilityEffect::RemoveStatus {{ id: "{status_id}" }}'
        else:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "expected buff, debuff, or a snake_case status id")
            return None
    elif t == "movement":
        if not _require(e, "effect_id", report, f"movement kind ({', '.join(MOVEMENTS)})") or not no_scaling():
            return None
        if effect_id not in MOVEMENTS:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"unknown movement; expected one of: {', '.join(MOVEMENTS)}")
            return None
        if effect_id == "knockback" and not _require(e, "value", report, "knockback distance"):
            return None
        _unused(e, report, "damage_type", "amount")
        distance = f"Some({rank_value(value)})" if value else "None"
        effect = (
            f"AbilityEffect::Displace {{ kind: DisplacementKind::{MOVEMENTS[effect_id]}, "
            f"distance: {distance}, duration: {rank_value(duration or [DEFAULT_MOVEMENT_DURATION])} }}"
        )
    elif t == "stat_modifier":
        if not _require(e, "effect_id", report, "stat to modify") or not _require(e, "value", report, "stat bonus") or not no_scaling():
            return None
        _unused(e, report, "damage_type", "amount")
        if e.trigger == "passive":
            if effect_id not in PASSIVE_STATS:
                report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"unknown passive stat; expected one of: {', '.join(PASSIVE_STATS)}")
                return None
            if duration:
                report.error(row.file, row.line, "duration", row.get("duration"), "passive stat bonuses last while the item is owned; leave blank")
                return None
            beneficial = True
            effect = f"AbilityEffect::PassiveStat {{ stat: {PASSIVE_STATS[effect_id]}, amount: {rank_value(value)} }}"
        else:
            if effect_id not in TIMED_STATS:
                report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"timed stat modifiers support: {', '.join(TIMED_STATS)}")
                return None
            if not _require(e, "duration", report, "timed stat modifiers need a duration (only item passives are permanent)"):
                return None
            beneficial = all(v >= 0 for v in value)
            kind = "Buff" if beneficial else "Debuff"
            fields = {name: "RankValue::ZERO" for name in TIMED_STATS.values()}
            fields[TIMED_STATS[effect_id]] = rank_value(value)
            body = ", ".join(f"{k}: {v}" for k, v in fields.items())
            effect = (
                f'AbilityEffect::ApplyStatus {{ status: StatusSpec::Modifier {{ id: "{ctx.owner_id}_{effect_id}", '
                f"kind: StatusKind::{kind}, {body} }}, duration: {rank_value(duration)} }}"
            )
    elif t == "custom":
        if not _require(e, "effect_id", report, "registered custom effect id"):
            return None
        if not ID_PATTERN.match(effect_id):
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "custom effect ids must be snake_case")
            return None
        if not no_scaling():
            return None
        _unused(e, report, "damage_type")
        effect = (
            f'AbilityEffect::Custom {{ id: "{effect_id}", value: {rank_value(value)}, '
            f"amount: {rank_value(amount)}, duration: {rank_value(duration)} }}"
        )
    else:  # pragma: no cover - parse_enum already rejected it
        return None

    if t not in ("movement",) and e.target != "units_in_radius" and row.get("radius"):
        report.warn(row.file, row.line, "radius", row.get("radius"), "only used by units_in_radius targets")

    # --- target
    combo = ctx.invalid_combo.get((e.trigger, e.target))
    if combo:
        report.error(row.file, row.line, "effect_target", e.target, combo)
        return None
    target_name = e.target
    if target_name in ("caster", "owner"):
        target = "EffectTarget::Caster"
    elif target_name == "cast_target":
        target = "EffectTarget::CastTarget"
    elif target_name == "trigger_unit":
        target = "EffectTarget::TriggerUnit"
    elif target_name == "affected_units":
        target = "EffectTarget::AffectedUnits"
    elif target_name == "attacker":
        if e.trigger == "on_damage_taken":
            target = "EffectTarget::TriggerUnit"
        elif e.trigger in ("on_attack", "on_attack_hit"):
            target = "EffectTarget::Caster"
        else:
            report.error(row.file, row.line, "effect_target", target_name, "attacker is only defined for on_attack, on_attack_hit, and on_damage_taken")
            return None
    else:  # units_in_radius
        if radius:
            radius_rs = f"AreaRadius::Fixed({rank_value(radius)})"
        elif ctx.has_aoe:
            radius_rs = "AreaRadius::Ability"
        else:
            where = "the ability has no aoe_radius" if ctx.kind == "ability" else "items have no aoe_radius"
            report.error(row.file, row.line, "radius", "", f"required for units_in_radius ({where})")
            return None
        team = ctx.team or ("Ally" if beneficial else "Enemy")
        center = ctx.center_for.get(e.trigger, "TriggerPoint")
        target = (
            f"EffectTarget::UnitsInRadius {{ center: AreaCenter::{center}, "
            f"radius: {radius_rs}, team: TargetTeam::{team} }}"
        )

    if len(report.errors) != errors_before:
        return None
    return trigger, target, effect


# --------------------------------------------------------------------------- source patching


def region_body(text: str, prefix: str, name: str) -> str:
    match = re.search(rf"[ \t]*// <{prefix}:{name}>\n(?P<body>.*?)[ \t]*// </{prefix}:{name}>", text, re.DOTALL)
    if not match:
        raise SystemExit(f"Missing `// <{prefix}:{name}>` marker region")
    return match.group("body")


def replace_region(text: str, prefix: str, name: str, body: str) -> str:
    pattern = re.compile(rf"([ \t]*// <{prefix}:{name}>\n)(.*?)([ \t]*// </{prefix}:{name}>)", re.DOTALL)
    if body and not body.endswith("\n"):
        body += "\n"
    updated, count = pattern.subn(lambda m: m.group(1) + body + m.group(3), text, count=1)
    if count != 1:
        raise SystemExit(f"Failed to update marker region {prefix}:{name}")
    return updated


def upsert_line(text: str, prefix: str, name: str, key_pattern: str, line: str) -> str:
    """Replace the region line matching `key_pattern`, or append `line` to the region."""
    body = region_body(text, prefix, name)
    lines = body.splitlines(keepends=True)
    regex = re.compile(key_pattern)
    for i, existing in enumerate(lines):
        if regex.search(existing):
            lines[i] = line + "\n"
            return replace_region(text, prefix, name, "".join(lines))
    return replace_region(text, prefix, name, body + line + "\n")


def read_blocks(path: Path, tag: str) -> dict[str, str]:
    """`// <tag:id>` … `// </tag:id>` blocks from a previously generated file."""
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8")
    return {
        m.group(1): m.group(0)
        for m in re.finditer(rf"// <{tag}:([a-z0-9_]+)>\n.*?// </{tag}:\1>", text, re.DOTALL)
    }


def rustfmt(path: Path) -> None:
    exe = shutil.which("rustfmt")
    if not exe:
        print(f"note: rustfmt not found; {path} left unformatted", file=sys.stderr)
        return
    subprocess.run([exe, "--edition", "2024", str(path)], check=True)
