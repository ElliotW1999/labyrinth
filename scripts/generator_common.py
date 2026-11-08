"""Shared CSV parsing, validation, and Rust emission for AbilityGenerator / ItemGenerator.

Both generators describe gameplay consequences with the same effect-row schema:

    <owner>_id,effect_index,trigger,effect_target,effect_type,effect_id,value,
    damage_type,duration,radius,amount,scaling_type,scaling_value

and translate each row into one `AbilityEffectEntry { trigger, target, effect }`
(see `src/abilities/definition.rs`). Abilities and items only differ in which
triggers/targets are meaningful for them.

Ability CSV `target_type=point` with non-zero `aoe_radius` and `projectile_type none`
maps to Rust `TargetType::Area` (ground-placed AoE); there is no separate CSV `area`
value unless added as an alias later.
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

# Reactive triggers (see `AbilityTrigger` in src/abilities/definition.rs). Parameterised
# ones are written `name:arg[:arg]` in the CSV; the base name keys every table below.
REACTIVE_PLAIN_TRIGGERS = {
    "on_damage_dealt": "OnDamageDealt",
    "on_damage_taken": "OnDamageTaken",
    "on_kill": "OnKill",
    "on_death": "OnDeath",
    "on_spell_cast": "OnSpellCast",
    "on_spell_hit": "OnSpellHit",
    "on_cooldown_ready": "OnCooldownReady",
    "on_move": "OnMove",
}
REACTIVE_PARAM_TRIGGERS = {
    "on_health_below": "OnHealthBelow",
    "on_status_applied": "OnStatusApplied",
    "on_status_removed": "OnStatusRemoved",
    "on_interval": "OnInterval",
    "on_enemy_enter": "OnEnemyEnter",
    "on_enemy_leave": "OnEnemyLeave",
    "while": "While",
}
REACTIVE_TRIGGER_NAMES = (*REACTIVE_PLAIN_TRIGGERS, *REACTIVE_PARAM_TRIGGERS)
# Reactive triggers that cannot name a cast target / have no trigger unit at all.
REACTIVE_NO_CAST_TARGET = tuple(t for t in REACTIVE_TRIGGER_NAMES if t != "on_spell_cast")
REACTIVE_NO_TRIGGER_UNIT = ("on_cooldown_ready", "on_interval", "on_move", "while")
# Damage- or status-caused triggers: effects under them run as reactions (they never start
# attacks and their damage / statuses fire no damage or status triggers).
REACTION_TRIGGERS = {
    "on_damage_dealt",
    "on_damage_taken",
    "on_health_below",
    "on_kill",
    "on_death",
    "on_status_applied",
    "on_status_removed",
}
# Effect types that re-fire the row's context under a custom `on_<name>` trigger.
GATE_TYPES = ("proc", "pseudo_proc", "counter", "accumulate")
# Every effect type that fires a custom trigger: the gates plus the Chain mechanic gate (one
# derived context per hop). Used for reachability and reaction propagation.
CUSTOM_TRIGGER_FEEDERS = (*GATE_TYPES, "chain")
# Triggers whose resolved target is the unit that caused them (the `attacker` target).
ATTACKER_TRIGGERS = ("on_damage_taken", "on_death")

ABILITY_TRIGGERS = {
    "on_cast": "OnCast",
    "on_projectile_hit": "OnProjectileHit",
    "on_unit_contact": "OnUnitContact",
    "on_movement_end": "OnMovementEnd",
    "on_impact": "OnImpact",
    "on_channel_tick": "OnChannelTick",
    "on_channel_end": "OnChannelEnd",
    "on_channel_interrupted": "OnChannelInterrupted",
    "on_expire": "OnExpire",
    "while_toggled": "WhileToggled",
    "passive": "Passive",
    "on_attack": "OnAttack",
    "on_attack_hit": "OnAttackHit",
    **REACTIVE_PLAIN_TRIGGERS,
    **REACTIVE_PARAM_TRIGGERS,
}

ITEM_TRIGGERS = {
    "passive": "Passive",
    "on_use": "OnUse",
    "on_unit_contact": "OnUnitContact",
    "on_movement_end": "OnMovementEnd",
    "on_attack": "OnAttack",
    "on_attack_hit": "OnAttackHit",
    "on_channel_end": "OnChannelEnd",
    "on_channel_interrupted": "OnChannelInterrupted",
    # Only with a projectile_type: the item launches the same projectile mechanic abilities use.
    "on_projectile_hit": "OnProjectileHit",
    "on_impact": "OnImpact",
    **REACTIVE_PLAIN_TRIGGERS,
    **REACTIVE_PARAM_TRIGGERS,
}

# Triggers that parse but that no game system emits yet; rows using them never fire.
NOT_EMITTED_TRIGGERS = {
    "on_channel_tick": "not emitted by the game yet; the effect will never fire",
    "on_expire": "not emitted by the game yet; the effect will never fire",
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
    "resource",
    "custom",
    # Gates: re-fire the row's context under a custom trigger (see AbilityEffect::Proc / Counter /
    # Accumulate). `pseudo_proc` is a proc with a pseudo-random distribution.
    "proc",
    "pseudo_proc",
    "counter",
    "accumulate",
    # Mechanic gate: jump from the start unit to nearby units, re-firing a custom trigger per hop.
    "chain",
    # Extra basic attacks (instant / double / multi-target attack).
    "perform_attack",
}

DAMAGE_TYPES = {"physical": "Physical", "magical": "Magical", "pure": "Pure"}

# effect_id → (StatusSpec expression, runtime status id, is_buff)
STATUSES = {
    "stun": ("StatusSpec::Stun", "stun", False),
    "root": ("StatusSpec::Root", "root", False),
    "silence": ("StatusSpec::Silence", "silence", False),
    "disarm": ("StatusSpec::Disarm", "disarm", False),
    "mute": ("StatusSpec::Mute", "mute", False),
    "sleep": ("StatusSpec::Sleep", "sleep", False),
    "invulnerable": ("StatusSpec::Invulnerable", "invulnerable", True),
    "hex": ("StatusSpec::Hex", "hex", False),
    "fear": ("StatusSpec::Fear", "fear", False),
    "slow": (None, "slow", False),
    "attack_speed_slow": (None, "attack_speed_slow", False),
    "turn_rate_slow": (None, "turn_rate_slow", False),
    "turn_rate_bonus": (None, "turn_rate_bonus", True),
    "flying": ("StatusSpec::Flying", "flying", True),
    "leash": (None, "leash", False),
    "tether": (None, "tether", False),
    "blind": (None, "blind", False),
    "evasion": (None, "evasion", True),
    "accuracy": (None, "accuracy", True),
    "true_strike": ("StatusSpec::TrueStrike", "true_strike", True),
    "phased": ("StatusSpec::Phased", "phased", True),
    "forceful": ("StatusSpec::Forceful", "forceful", True),
    "debuff_immunity": ("StatusSpec::DebuffImmunity", "spell_immunity", True),
    "spell_immunity": ("StatusSpec::DebuffImmunity", "spell_immunity", True),
    "shield": (None, "shield", True),
    "burn": (None, "burn", False),
    "heal_over_time": (None, "heal_over_time", True),
    "heal_reduction": (None, "heal_reduction", False),
    "heal_block": ("StatusSpec::HealBlock", "heal_block", False),
    "mana_shield": (None, "mana_shield", True),
}

# `effect_type=resource` ids → (kind, spent resource). Hostile kinds act on other units;
# the spend kinds act on the unit that uses them (caster/owner only).
RESOURCE_EFFECTS = {
    "health_drain": "drain_health",
    "mana_burn": "drain_mana",
    "mana_drain": "drain_mana",
    "spend_health": "spend",
    "spend_mana": "spend",
    "health_to_mana": "spend",
    "mana_to_health": "spend",
}
# Targets that resolve to the unit using the effect (see the `attacker` mapping below).
SELF_RESOURCE_TARGETS = ("caster", "owner")
# Mana-flow effects take their resource amount from `value`, ratios from `amount`.
RESOURCE_SPENT = {
    "spend_health": "Health",
    "health_to_mana": "Health",
    "spend_mana": "Mana",
    "mana_to_health": "Mana",
}
RESOURCE_CONVERTED_TO = {"health_to_mana": "Mana", "mana_to_health": "Health"}

# Activation targeting shared by abilities and active items.
TARGET_TYPES = ("unit", "point", "direction", "no_target", "self")
PROJECTILE_TYPES = ("none", "targeted", "skillshot", "skillshot_single")
# Skillshots fly the cast range; `skillshot_single` stops at the first enemy it strikes (a hook).
SKILLSHOT_TYPES = ("skillshot", "skillshot_single")

# Optional activation columns on items.csv (not part of ITEM_COLUMNS required tuple).
ITEM_ACTIVATION_OPTIONAL = (
    "charge_restore_time",
    "target_type",
    "target_team",
    "cast_range",
    "cast_time",
    "channel_time",
    "mana_cost",
    "projectile_type",
    "projectile_speed",
)
# Effect-polarity teams (`units_in_radius` defaults); "self" areas still mean allies.
TARGET_TEAMS = {"enemy": "Enemy", "ally": "Ally", "both": "Any", "self": "Ally"}
# Activation team rule (`TargetTeam` of the action).
ACTION_TEAMS = {**TARGET_TEAMS, "self": "SelfOnly"}

MOVEMENTS = {
    "knockback": "Knockback",
    "pull": "Pull",
    "pull_point": "PullToPoint",
    "dash": "Dash",
    "charge": "Charge",
    "drag": "Drag",
    "cyclone": "Cyclone",
    "teleport": None,
}
# Teleport sends every target to the cast's aim point, so only single-target selectors make sense.
TELEPORT_TARGETS = ("caster", "owner", "cast_target", "trigger_unit")
# Dash and charge move the unit that uses them.
SELF_MOVEMENT_TARGETS = ("caster", "owner")
DEFAULT_DASH_DURATION = 0.3
DEFAULT_MOVEMENT_DURATION = 0.3

# Timed stat modifiers map onto `StatusSpec::Modifier` fields.
TIMED_STATS = {
    "armor": "armor",
    "magic_resist": "magic_resist",
    "move_speed": "move_speed",
    "attack_damage": "attack_damage",
    "attack_speed_flat": "attack_speed_flat",
    "attack_speed_mult": "attack_speed_mult",
    "attack_range": "attack_range",
}

# Timed resource modifiers map onto `StatusSpec::ResourceModifier` (summed when read).
RESOURCE_STATS = {
    "lifesteal": "ResourceStat::Lifesteal",
    "spell_lifesteal": "ResourceStat::SpellLifesteal",
    "heal_amp": "ResourceStat::HealAmp",
    "heal_received_amp": "ResourceStat::HealReceivedAmp",
    "mana_cost_amp": "ResourceStat::ManaCostAmp",
    "health_regen": "ResourceStat::HealthRegen",
    "mana_regen": "ResourceStat::ManaRegen",
    "max_health": "ResourceStat::MaxHealth",
    "max_mana": "ResourceStat::MaxMana",
}
# Fractions (0.25 = 25%) share one range check; mana_cost_amp only has a floor (-1 = free).
FRACTION_STATS = (
    "lifesteal",
    "spell_lifesteal",
    "heal_amp",
    "heal_received_amp",
    "crit_chance",
    "evasion",
    "accuracy",
    "cleave",
    "splash",
)
# A negative cost amp is the beneficial direction.
NEGATIVE_IS_BENEFICIAL = ("mana_cost_amp",)

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
    "spell_amp": "PassiveStat::SpellAmp",
    "outgoing_physical_amp": "PassiveStat::OutgoingPhysicalAmp",
    "incoming_damage_amp": "PassiveStat::IncomingDamageAmp",
    "damage_block": "PassiveStat::DamageBlock",
    "damage_reflect": "PassiveStat::DamageReflect",
    "lifesteal": "PassiveStat::Lifesteal",
    "spell_lifesteal": "PassiveStat::SpellLifesteal",
    "heal_amp": "PassiveStat::HealAmp",
    "heal_received_amp": "PassiveStat::HealReceivedAmp",
    "mana_cost_amp": "PassiveStat::ManaCostAmp",
    # Attack modifiers. Chances and ratios are fractions (0.25 = 25%); crit_multiplier is a
    # bonus on top of the 2.0 base; attack_projectile_speed is in units per second.
    "crit_chance": "PassiveStat::CritChance",
    "crit_multiplier": "PassiveStat::CritMultiplier",
    "evasion": "PassiveStat::Evasion",
    "accuracy": "PassiveStat::Accuracy",
    "cleave": "PassiveStat::CleaveRatio",
    "splash": "PassiveStat::SplashRatio",
    "attack_range": "PassiveStat::AttackRange",
    "attack_projectile_speed": "PassiveStat::AttackProjectileSpeed",
}

SCALING_TYPES = {
    "strength": "EffectScaling::CasterAttribute(Attribute::Strength, {v})",
    "agility": "EffectScaling::CasterAttribute(Attribute::Agility, {v})",
    "intelligence": "EffectScaling::CasterAttribute(Attribute::Intelligence, {v})",
    "attack_damage": "EffectScaling::CasterAttackDamage({v})",
    "target_health_below": "EffectScaling::TargetHealthBelow({v})",
    "target_missing_health": "EffectScaling::TargetMissingHealth({v})",
    "target_max_health": "EffectScaling::TargetMaxHealth({v})",
    "target_current_health": "EffectScaling::TargetCurrentHealth({v})",
    "distance": "EffectScaling::DistanceFromCaster {{ falloff: {v} }}",
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
        def order(issue: Issue) -> tuple[str, int]:
            return issue.file, issue.row or 0

        for issue in sorted(self.warnings, key=order):
            print(f"warning: {issue}", file=sys.stderr)
        for issue in sorted(self.errors, key=order):
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


@dataclass
class ActivationSpec:
    """Shared activation / targeting fields for abilities and active items."""

    target_type: str
    target_team: str
    cast_range: list[float] | None = None
    cast_range_scalar: float | None = None
    aoe_radius: list[float] | None = None
    cast_time: float = 0.0
    channel_time: float | None = None
    max_charges: int | None = None
    charge_restore_time: float | None = None
    projectile_type: str = "none"
    projectile_speed: float | None = None

    def has_cast_range(self, kind: str) -> bool:
        if kind == "ability":
            return bool(self.cast_range)
        return self.cast_range_scalar is not None


def parse_activation_spec(
    row: Row,
    *,
    kind: str,
    has_active_effects: bool,
    report: Report,
    levels: bool = False,
) -> ActivationSpec | None:
    """Parse activation columns from an ability or item row."""
    present = [c for c in ITEM_ACTIVATION_OPTIONAL if row.get(c)]
    if kind == "item" and not has_active_effects:
        for column in present:
            report.warn(row.file, row.line, column, row.get(column), "ignored: the item has no on_use effects")
        return None

    target_type = parse_enum(row, "target_type", TARGET_TYPES, report, required=kind == "ability")
    default_team = "self" if target_type in ("no_target", "self") else "both"
    target_team = parse_enum(
        row,
        "target_team",
        sorted(TARGET_TEAMS),
        report,
        required=kind == "ability" or (kind == "item" and target_type == "unit"),
    )
    if kind == "item" and target_type is None:
        target_type = "no_target"
    if kind == "item" and target_team is None:
        target_team = default_team

    cast_range = parse_number(row, "cast_range", report, levels=levels, minimum=0)
    cast_range_scalar = parse_scalar(row, "cast_range", report, minimum=0) if kind == "item" else None
    aoe_radius = parse_number(row, "aoe_radius", report, levels=levels, minimum=0) if kind == "ability" else None
    cast_time = parse_scalar(row, "cast_time", report, minimum=0) or 0.0
    channel_time = parse_scalar(row, "channel_time", report, minimum=0)
    max_charges = parse_scalar(row, "max_charges", report, integer=True, minimum=1)
    restore = parse_scalar(row, "charge_restore_time", report, minimum=0)
    projectile = parse_enum(row, "projectile_type", PROJECTILE_TYPES, report, required=False) or "none"
    speed = parse_scalar(row, "projectile_speed", report, minimum=1)

    if kind == "ability" and None in (target_type, target_team):
        return None
    if kind == "item" and target_type is None:
        return None

    if kind == "item" and speed is not None and projectile == "none":
        report.error(row.file, row.line, "projectile_speed", row.get("projectile_speed"), "projectile_speed requires projectile_type targeted or skillshot")

    return ActivationSpec(
        target_type=target_type,
        target_team=target_team,
        cast_range=cast_range if kind == "ability" else None,
        cast_range_scalar=cast_range_scalar if kind == "item" else None,
        aoe_radius=aoe_radius,
        cast_time=cast_time,
        channel_time=channel_time,
        max_charges=int(max_charges) if max_charges else None,
        charge_restore_time=restore,
        projectile_type=projectile,
        projectile_speed=speed,
    )


def validate_activation(
    row: Row,
    spec: ActivationSpec,
    *,
    kind: str,
    ability_type: str | None,
    report: Report,
) -> None:
    """Cross-field activation rules shared by abilities and active items."""

    def err(fld: str, message: str) -> None:
        report.error(row.file, row.line, fld, row.get(fld), message)

    if kind == "item" and spec.charge_restore_time is not None:
        err("charge_restore_time", "item charges do not regenerate; leave charge_restore_time blank")
    elif spec.charge_restore_time is not None and spec.max_charges is None:
        err("charge_restore_time", "charge_restore_time requires max_charges")
    if spec.max_charges is not None and spec.charge_restore_time is None and kind == "ability":
        err("max_charges", "charge-based abilities need a charge_restore_time")

    if spec.projectile_speed is not None and spec.projectile_type == "none":
        err("projectile_speed", "projectile_speed requires projectile_type targeted, skillshot, or skillshot_single")
    if spec.projectile_type == "targeted" and spec.target_type != "unit":
        err("projectile_type", f"targeted projectiles home in on a unit; target_type is {spec.target_type}")
    if spec.projectile_type in SKILLSHOT_TYPES and spec.target_type not in ("point", "direction"):
        err("projectile_type", f"skillshots fly toward a point or direction; target_type is {spec.target_type}")

    if spec.target_team == "self" and spec.target_type not in ("self", "no_target"):
        err("target_team", f"target_team self requires target_type self or no_target (got {spec.target_type})")
    if spec.target_type == "self" and spec.target_team != "self":
        err("target_team", "target_type self requires target_team self")

    needs_range = spec.target_type in ("unit", "point", "direction")
    skip_range = kind == "ability" and ability_type == "passive"
    if needs_range and not skip_range and not spec.has_cast_range(kind):
        label = "abilities" if kind == "ability" else "items"
        err("cast_range", f"required for {spec.target_type}-targeted {label}")
    if spec.target_type in ("no_target", "self") and row.get("cast_range"):
        report.warn(row.file, row.line, "cast_range", row.get("cast_range"), f"ignored: {spec.target_type} has no range")

    if kind == "ability" and ability_type == "passive":
        if spec.target_type not in ("no_target", "self"):
            err("target_type", "passive abilities cannot be targeted; use no_target or self")
        if spec.projectile_type != "none":
            err("projectile_type", "passive abilities cannot launch projectiles")
        for fld in ("cooldown", "mana_cost", "cast_time", "channel_time", "max_charges"):
            if row.get(fld) and any(v != 0 for v in parse_number(row, fld, Report(), levels=True) or []):
                report.warn(row.file, row.line, fld, row.get(fld), "ignored: passive abilities are never cast")
    if kind == "ability" and ability_type == "toggle" and spec.target_type not in ("no_target", "self"):
        err("target_type", "toggle abilities use no_target or self")


def ability_rust_target_type(target_type: str, projectile: str, aoe_radius: list[float] | None) -> str:
    if target_type == "unit":
        return "Unit"
    if target_type == "point" and projectile == "none" and aoe_radius and any(aoe_radius):
        return "Area"
    if target_type == "point":
        return "Point"
    if target_type == "direction":
        return "Direction"
    if target_type == "self":
        return "SelfTarget"
    return "NoTarget"


def find_emitted_custom_triggers(repo_root: Path) -> set[str]:
    """Custom `on_*` trigger names that hand-written Rust (mechanics) may emit."""
    names: set[str] = set()
    for path in (repo_root / "src").rglob("*.rs"):
        if path.name in ("generated.rs", "item_catalog.rs"):
            continue
        names.update(re.findall(r'Custom\("(on_[a-z0-9_]+)"\)', path.read_text(encoding="utf-8")))
    return names


def warn_unreachable_custom_triggers(effects: list["EffectRow"], emitted: set[str] | None, report: Report) -> None:
    """A custom trigger only fires when a proc / counter row names it or Rust emits it."""
    fed = {e.row.get("effect_id").lower() for e in effects if e.effect_type in CUSTOM_TRIGGER_FEEDERS}
    for e in effects:
        if e.trigger in ABILITY_TRIGGERS or e.trigger in ITEM_TRIGGERS:
            continue
        if e.trigger in fed or (emitted is not None and e.trigger in emitted):
            continue
        report.warn(
            e.row.file,
            e.row.line,
            "trigger",
            e.trigger,
            "custom trigger is never fired: no proc / counter / accumulate / chain row names it and no Rust code emits it",
        )


def warn_unemitted_triggers(effects: list["EffectRow"], report: Report) -> None:
    """Warn on rows whose (valid) trigger no game system emits. Shared by both generators."""
    for e in effects:
        message = NOT_EMITTED_TRIGGERS.get(e.trigger)
        if message:
            report.warn(e.row.file, e.row.line, "trigger", e.trigger, message)


def find_registered_custom_effects(repo_root: Path) -> set[str]:
    registered: set[str] = set()
    src = repo_root / "src"
    if not src.is_dir():
        return registered
    pattern = re.compile(r'register_custom_effect\s*\(\s*"([a-z][a-z0-9_]*)"')
    for path in src.rglob("*.rs"):
        registered.update(pattern.findall(path.read_text(encoding="utf-8")))
    return registered


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
    #: Colon arguments of the trigger cell (`on_interval:5` -> `("5",)`); `trigger` is the base name.
    trigger_args: tuple[str, ...] = ()


def parse_effect_rows(rows: list[Row], owner_column: str, owners: set[str], report: Report) -> dict[str, list[EffectRow]]:
    """Common checks: owner exists, effect_index unique per owner, enums known."""
    by_owner: dict[str, list[EffectRow]] = {}
    seen: dict[tuple[str, int], int] = {}
    for row in rows:
        owner = check_id(row, owner_column, report)
        index = parse_scalar(row, "effect_index", report, integer=True, minimum=0)
        if row.get("effect_index") == "":
            report.error(row.file, row.line, "effect_index", "", "required")
        trigger, *trigger_args = row.get("trigger").lower().split(":")
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
        by_owner.setdefault(owner, []).append(
            EffectRow(row, owner, int(index), trigger, target, effect_type, tuple(trigger_args))
        )
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
    #: Known custom effect ids from the Rust codebase (optional warn when authoring custom rows).
    registered_custom: set[str] | None = None
    #: Activation `target_type` of the owner (None = unknown), for effects that need a cast target.
    target_type: str | None = None
    #: Every trigger used by the owner's rows (lets `proc` / `counter` check their target).
    owner_triggers: set[str] | None = None
    #: Triggers whose effects run as reactions (see `reaction_triggers`); None = just the built-ins.
    reaction: set[str] | None = None
    #: Custom triggers fed (directly or through gates) by a `chain` row (see `chain_fed_triggers`).
    chain_fed: set[str] | None = None


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


def _fraction_stat_ok(e: EffectRow, effect_id: str, value: list[float], report: Report) -> bool:
    """Range rules for the fraction-valued resource stats (amps, lifesteal, cost)."""
    row = e.row
    if effect_id in FRACTION_STATS and any(not -1 <= v <= 1 for v in value):
        report.error(row.file, row.line, "value", row.get("value"), f"{effect_id} is a fraction (0.25 = 25%) and must be within -1..1")
        return False
    if effect_id == "mana_cost_amp" and any(v < -1 for v in value):
        report.error(row.file, row.line, "value", row.get("value"), "mana_cost_amp cannot go below -1 (a cost cannot be negative)")
        return False
    return True


def _scalar_ratio(e: EffectRow, name: str, values: list[float] | None, default: float, report: Report) -> float | None:
    """A ratio column that the runtime stores as a single f32 (no per-level values)."""
    row = e.row
    if not values:
        return default
    if any(v != values[0] for v in values):
        report.error(row.file, row.line, name, row.get(name), "this ratio cannot vary by level")
        return None
    if values[0] <= 0:
        report.error(row.file, row.line, name, row.get(name), "ratio must be above 0")
        return None
    return values[0]


def _resource_effect(
    e: EffectRow,
    effect_id: str,
    scaling: str,
    scaling_type: str | None,
    value: list[float] | None,
    amount: list[float] | None,
    report: Report,
) -> str | None:
    """`AbilityEffect::{DrainHealth, DrainMana, SpendResource}`.

    Columns: `value` = amount, `amount` = ratio (see the table in the plan / README).
    """
    row = e.row
    if effect_id not in RESOURCE_EFFECTS:
        report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"unknown resource effect; expected one of: {', '.join(RESOURCE_EFFECTS)}")
        return None
    kind = RESOURCE_EFFECTS[effect_id]
    if not _require(e, "value", report, "resource amount"):
        return None
    if any(v <= 0 for v in value):
        report.error(row.file, row.line, "value", row.get("value"), "resource amount must be above 0")
        return None
    if scaling_type and kind != "drain_health":
        report.error(row.file, row.line, "scaling_type", scaling_type, "scaling is only supported by damage, heal, and health_drain effects")
        return None
    # `attacker` resolves to the unit using the effect for on_attack / on_attack_hit.
    is_self = e.target in SELF_RESOURCE_TARGETS or (e.target == "attacker" and e.trigger in ("on_attack", "on_attack_hit"))
    if kind == "spend":
        if e.target not in SELF_RESOURCE_TARGETS:
            report.error(row.file, row.line, "effect_target", e.target, f"{effect_id} spends the user's own resource; use one of: {', '.join(SELF_RESOURCE_TARGETS)}")
            return None
    elif is_self:
        report.error(row.file, row.line, "effect_target", e.target, f"{effect_id} acts on other units; use cast_target, trigger_unit, affected_units, units_in_radius, or attacker (on_damage_taken)")
        return None
    if kind == "drain_health":
        if not _require(e, "damage_type", report, "physical, magical, or pure"):
            return None
        damage_type = parse_enum(row, "damage_type", list(DAMAGE_TYPES), report, required=False)
        ratio = _scalar_ratio(e, "amount", amount, 1.0, report)
        if damage_type is None or ratio is None:
            return None
        return (
            f"AbilityEffect::DrainHealth {{ amount: {rank_value(value)}, "
            f"damage_type: DamageType::{DAMAGE_TYPES[damage_type]}, scaling: {scaling}, transfer_ratio: {f32(ratio)} }}"
        )
    if kind == "drain_mana":
        ratio = _scalar_ratio(e, "amount", amount, 1.0, report)
        if ratio is None:
            return None
        if effect_id == "mana_burn":
            if not _require(e, "damage_type", report, "physical, magical, or pure"):
                return None
            damage_type = parse_enum(row, "damage_type", list(DAMAGE_TYPES), report, required=False)
            if damage_type is None:
                return None
            return (
                f"AbilityEffect::DrainMana {{ amount: {rank_value(value)}, burn_ratio: {f32(ratio)}, "
                f"damage_type: DamageType::{DAMAGE_TYPES[damage_type]}, transfer_ratio: 0.0 }}"
            )
        _unused(e, report, "damage_type")
        return (
            f"AbilityEffect::DrainMana {{ amount: {rank_value(value)}, burn_ratio: 0.0, "
            f"damage_type: DamageType::Pure, transfer_ratio: {f32(ratio)} }}"
        )
    # spend / conversion
    _unused(e, report, "damage_type")
    convert = "None"
    if effect_id in RESOURCE_CONVERTED_TO:
        if not _require(e, "amount", report, "resource gained per point spent"):
            return None
        ratio = _scalar_ratio(e, "amount", amount, 1.0, report)
        if ratio is None:
            return None
        convert = f"Some((ResourceKind::{RESOURCE_CONVERTED_TO[effect_id]}, {f32(ratio)}))"
    else:
        _unused(e, report, "amount")
    return (
        f"AbilityEffect::SpendResource {{ resource: ResourceKind::{RESOURCE_SPENT[effect_id]}, "
        f"amount: {rank_value(value)}, convert_to: {convert} }}"
    )


def _displacement_effect(
    e: EffectRow,
    effect_id: str,
    ctx: EffectContext,
    value: list[float] | None,
    duration: list[float] | None,
    amount: list[float] | None,
    report: Report,
) -> str | None:
    """`AbilityEffect::Displace` for the forced-movement kinds, validated per kind.

    Columns: `value` = distance, `duration` = seconds, `amount` = speed (charge only).
    """
    row = e.row
    kind = MOVEMENTS[effect_id]
    if effect_id in ("dash", "charge") and e.target not in SELF_MOVEMENT_TARGETS:
        report.error(
            row.file,
            row.line,
            "effect_target",
            e.target,
            f"{effect_id} moves the unit that uses it; use one of: {', '.join(SELF_MOVEMENT_TARGETS)}",
        )
        return None
    if effect_id == "drag" and e.target in SELF_MOVEMENT_TARGETS:
        report.error(
            row.file,
            row.line,
            "effect_target",
            e.target,
            "drag carries other units along with the caster's own movement; use trigger_unit, affected_units, or units_in_radius",
        )
        return None
    speed = "None"
    default_duration = DEFAULT_MOVEMENT_DURATION
    if effect_id == "knockback" and not _require(e, "value", report, "knockback distance"):
        return None
    if effect_id == "dash":
        if not _require(e, "value", report, "maximum dash distance"):
            return None
        default_duration = DEFAULT_DASH_DURATION
    if effect_id == "charge":
        if ctx.target_type is not None and ctx.target_type != "unit":
            report.error(
                row.file,
                row.line,
                "effect_type",
                "movement",
                f"charge runs at the cast target and needs target_type unit (got {ctx.target_type})",
            )
            return None
        if not _require(e, "amount", report, "charge speed in world units per second"):
            return None
        if not _require(e, "duration", report, "maximum charge time in seconds"):
            return None
        if any(s <= 0 for s in amount):
            report.error(row.file, row.line, "amount", row.get("amount"), "charge speed must be above 0")
            return None
        speed = f"Some({rank_value(amount)})"
    if effect_id == "cyclone" and not _require(e, "duration", report, "cyclone duration in seconds"):
        return None
    if effect_id == "drag":
        default_duration = 0.0  # blank: until the carrying movement ends
    unused = ["damage_type"]
    if effect_id != "charge":
        unused.append("amount")
    if effect_id in ("cyclone", "drag"):
        unused.append("value")
    _unused(e, report, *unused)
    distance = f"Some({rank_value(value)})" if value and effect_id not in ("cyclone", "drag") else "None"
    return (
        f"AbilityEffect::Displace {{ kind: DisplacementKind::{kind}, "
        f"distance: {distance}, duration: {rank_value(duration or [default_duration])}, speed: {speed} }}"
    )


def _number(text: str) -> float | None:
    try:
        return float(text)
    except ValueError:
        return None


def _tick_interval_ok(interval: list[float], duration: list[float], row, report: Report) -> bool:
    """A burn's tick interval (`amount`) must be above 0 and at most the duration at every rank."""
    ok = True
    for rank, seconds in enumerate(interval):
        lasts = duration[min(rank, len(duration) - 1)] if duration else 0.0
        if seconds <= 0:
            report.error(row.file, row.line, "amount", row.get("amount"), "burn tick interval (seconds) must be above 0; leave blank for damage per second")
            ok = False
        elif seconds > lasts:
            report.error(row.file, row.line, "amount", row.get("amount"), f"burn tick interval {seconds:g}s exceeds the {lasts:g}s duration")
            ok = False
    return ok


def runtime_status_id(text: str) -> str | None:
    """Runtime id for a status named in a trigger argument, or None when it is unknown.

    Accepts CSV status ids (`stun`), runtime ids (`spell_immunity`) and the per-owner ids that
    timed `stat_modifier` rows create (`<owner>_<stat>`) and the ones burns create (`<owner>_burn`)."""
    if text in STATUSES:
        return STATUSES[text][1]
    if text in {runtime for _, runtime, _ in STATUSES.values()}:
        return text
    for stat in (*TIMED_STATS, *RESOURCE_STATS, "burn"):
        suffix = "_" + stat
        if text.endswith(suffix) and ID_PATTERN.match(text[: -len(suffix)]):
            return text
    return None


def trigger_to_rust(e: EffectRow, variant: str, report: Report) -> str | None:
    """`AbilityTrigger::...` for the row's trigger cell, validating its colon arguments."""
    row = e.row
    args = list(e.trigger_args)

    def bad(message: str) -> None:
        report.error(row.file, row.line, "trigger", row.get("trigger"), message)

    def integer(text: str, what: str, low: int, high: int) -> int | None:
        value = _number(text)
        if value is None or not value.is_integer() or not low <= value <= high:
            bad(f"{what} must be a whole number from {low} to {high} (got {text!r})")
            return None
        return int(value)

    def radius(text: str) -> int | None:
        value = _number(text)
        if value is None or value <= 0 or value > 100000:
            bad(f"radius must be a number above 0 (got {text!r})")
            return None
        return max(1, round(value))

    name = e.trigger
    if name not in REACTIVE_PARAM_TRIGGERS:
        if args:
            bad(f"{name} takes no argument (write just {name})")
            return None
        return f"AbilityTrigger::{variant}"
    if name == "on_health_below":
        if len(args) != 1:
            bad("on_health_below needs a percent: on_health_below:30")
            return None
        percent = integer(args[0], "health percent", 1, 99)
        return None if percent is None else f"AbilityTrigger::OnHealthBelow {{ percent: {percent} }}"
    if name == "on_interval":
        seconds = _number(args[0]) if len(args) == 1 else None
        if seconds is None or seconds < 0.1 or seconds > 86400:
            bad("on_interval needs a period in seconds of at least 0.1: on_interval:5")
            return None
        return f"AbilityTrigger::OnInterval {{ millis: {round(seconds * 1000)} }}"
    if name in ("on_enemy_enter", "on_enemy_leave"):
        if len(args) != 1:
            bad(f"{name} needs a radius: {name}:600")
            return None
        value = radius(args[0])
        return None if value is None else f"AbilityTrigger::{variant} {{ radius: {value} }}"
    if name in ("on_status_applied", "on_status_removed"):
        if len(args) > 1:
            bad(f"{name} takes at most one filter: buff, debuff or a status id")
            return None
        if not args or args[0] == "any":
            return f"AbilityTrigger::{variant}(StatusFilter::Any)"
        if args[0] in ("buff", "debuff"):
            return f"AbilityTrigger::{variant}(StatusFilter::{args[0].capitalize()})"
        status = runtime_status_id(args[0])
        if status is None:
            bad(f"unknown status {args[0]!r}; use buff, debuff, a status id ({', '.join(STATUSES)}) or a <owner>_<stat> modifier id")
            return None
        return f'AbilityTrigger::{variant}(StatusFilter::Id("{status}"))'
    # while:<condition>[:arg[:arg]]
    condition = args[0] if args else ""
    rest = args[1:]
    if condition in ("moving", "stationary"):
        if rest:
            bad(f"while:{condition} takes no argument")
            return None
        return f"AbilityTrigger::While(Condition::{condition.capitalize()})"
    if condition == "health_below":
        if len(rest) != 1:
            bad("while:health_below needs a percent: while:health_below:50")
            return None
        percent = integer(rest[0], "health percent", 1, 99)
        return None if percent is None else f"AbilityTrigger::While(Condition::HealthBelow {{ percent: {percent} }})"
    if condition in ("enemies_near", "allies_near"):
        if len(rest) != 2:
            bad(f"while:{condition} needs a radius and a count: while:{condition}:600:2")
            return None
        value, count = radius(rest[0]), integer(rest[1], "unit count", 1, 255)
        if value is None or count is None:
            return None
        variant_name = "EnemiesNear" if condition == "enemies_near" else "AlliesNear"
        return f"AbilityTrigger::While(Condition::{variant_name} {{ radius: {value}, at_least: {count} }})"
    if condition == "has_status":
        if len(rest) != 1:
            bad("while:has_status needs a status id: while:has_status:stun")
            return None
        status = runtime_status_id(rest[0])
        if status is None:
            bad(f"unknown status {rest[0]!r}; use a status id ({', '.join(STATUSES)}) or a <owner>_<stat> modifier id")
            return None
        return f'AbilityTrigger::While(Condition::HasStatus("{status}"))'
    bad(
        "while needs a condition: health_below:<percent>, moving, stationary, "
        "enemies_near:<radius>:<count>, allies_near:<radius>:<count> or has_status:<id>"
    )
    return None


def held_status_id(e: EffectRow) -> str | None:
    """Runtime id of the status a `while` row holds, or None when it cannot be told."""
    effect_id = e.row.get("effect_id").lower()
    if e.effect_type == "apply_status" and effect_id == "burn":
        return f"{e.owner}_burn"
    if e.effect_type == "apply_status" and effect_id in STATUSES:
        return STATUSES[effect_id][1]
    if e.effect_type == "stat_modifier" and effect_id:
        return f"{e.owner}_{effect_id}"
    return None


def reaction_triggers(effects: list[EffectRow]) -> set[str]:
    """Reaction triggers plus the custom triggers that reaction rows feed through gates."""
    reaction = set(REACTION_TRIGGERS)
    changed = True
    while changed:
        changed = False
        for e in effects:
            fed = e.row.get("effect_id").lower()
            if e.effect_type in CUSTOM_TRIGGER_FEEDERS and e.trigger in reaction and fed not in reaction:
                reaction.add(fed)
                changed = True
    return reaction


def chain_fed_triggers(effects: list[EffectRow]) -> set[str]:
    """Custom triggers fed by a `chain` row, directly or through further gates.

    Chain hops carry no `AttackMeta`, so a `perform_attack` under such a trigger would start an
    attack whose hit re-fires the trigger that fed the chain: an unbounded loop (F7).
    """
    fed_triggers: set[str] = set()
    changed = True
    while changed:
        changed = False
        for e in effects:
            fed = e.row.get("effect_id").lower()
            if e.effect_type not in CUSTOM_TRIGGER_FEEDERS or fed in fed_triggers:
                continue
            if e.effect_type == "chain" or e.trigger in fed_triggers:
                fed_triggers.add(fed)
                changed = True
    return fed_triggers


def warn_duplicate_held_statuses(effects: list[EffectRow], report: Report) -> None:
    """Two `while` rows of one owner holding the same status id would fight over it."""
    seen: dict[str, int] = {}
    for e in effects:
        if e.trigger != "while":
            continue
        status = held_status_id(e)
        if status is None:
            continue
        if status in seen:
            report.warn(
                e.row.file,
                e.row.line,
                "effect_id",
                e.row.get("effect_id"),
                f"holds status {status!r} like line {seen[status]}: held statuses share one id, so they fight; give each a unique id",
            )
        else:
            seen[status] = e.row.line


def effect_to_rust(e: EffectRow, ctx: EffectContext, report: Report) -> tuple[str, str, str] | None:
    """Translate one effect row into Rust `(trigger, target, effect)` expressions, or None on error."""
    row = e.row
    errors_before = len(report.errors)

    if e.trigger_args and e.trigger not in ABILITY_TRIGGERS and e.trigger not in ITEM_TRIGGERS:
        report.error(row.file, row.line, "trigger", row.get("trigger"), f"custom trigger {e.trigger} takes no argument (write just {e.trigger})")
        return None

    # --- trigger (resolve per owner kind so shared names like `passive` work for both)
    if ctx.kind == "ability":
        if e.trigger in ABILITY_TRIGGERS:
            trigger = trigger_to_rust(e, ABILITY_TRIGGERS[e.trigger], report)
            if trigger is None:
                return None
        elif CUSTOM_TRIGGER_PATTERN.match(e.trigger):
            trigger = f'AbilityTrigger::Custom("{e.trigger}")'
        else:
            report.error(
                row.file,
                row.line,
                "trigger",
                e.trigger,
                f"item trigger; abilities use: {', '.join(ABILITY_TRIGGERS)} or a custom on_<name>",
            )
            return None
    elif e.trigger in ITEM_TRIGGERS:
        trigger = trigger_to_rust(e, ITEM_TRIGGERS[e.trigger], report)
        if trigger is None:
            return None
    elif CUSTOM_TRIGGER_PATTERN.match(e.trigger):
        trigger = f'AbilityTrigger::Custom("{e.trigger}")'
    else:
        report.error(
            row.file,
            row.line,
            "trigger",
            e.trigger,
            f"ability trigger; items use: {', '.join(ITEM_TRIGGERS)}",
        )
        return None
    if e.trigger in ctx.invalid_trigger:
        report.error(row.file, row.line, "trigger", e.trigger, ctx.invalid_trigger[e.trigger])
        return None
    if e.trigger == "while_toggled":
        mana_shield = e.effect_type == "apply_status" and row.get("effect_id").lower() == "mana_shield"
        if e.effect_type != "stat_modifier" and not mana_shield:
            report.error(
                row.file,
                row.line,
                "effect_type",
                e.effect_type,
                "while_toggled only supports stat_modifier effects and the mana_shield status",
            )
            return None
        if e.target != "caster":
            report.error(row.file, row.line, "effect_target", e.target, "while_toggled bonuses apply to the caster")
            return None

    if e.trigger == "on_move" and e.effect_type != "accumulate":
        report.error(
            row.file,
            row.line,
            "effect_type",
            e.effect_type,
            "on_move fires every frame the owner moves, so its rows must be accumulate (e.g. every 1000 units)",
        )
        return None
    if e.trigger == "while":
        if e.effect_type not in ("apply_status", "stat_modifier"):
            report.error(
                row.file,
                row.line,
                "effect_type",
                e.effect_type,
                "while holds a permanent status: use apply_status or a timed-stat stat_modifier",
            )
            return None
        if e.target not in ("caster", "owner"):
            report.error(row.file, row.line, "effect_target", e.target, "while bonuses apply to the owner: use owner or caster")
            return None
        if row.get("duration"):
            report.error(row.file, row.line, "duration", row.get("duration"), "while statuses last while the condition holds; leave blank")
            return None
        held = held_status_id(e)
        if (
            len(e.trigger_args) == 2
            and e.trigger_args[0] == "has_status"
            and held is not None
            and runtime_status_id(e.trigger_args[1]) == held
        ):
            report.error(
                row.file,
                row.line,
                "trigger",
                row.get("trigger"),
                f"the condition checks status {held!r}, which this row holds: it would keep itself on forever",
            )
            return None
    if e.effect_type == "perform_attack" and e.trigger in (ctx.reaction or REACTION_TRIGGERS):
        report.error(
            row.file,
            row.line,
            "effect_type",
            e.effect_type,
            f"{e.trigger} reacts to damage or statuses, and reactions never start attacks",
        )
        return None
    if e.effect_type == "perform_attack" and e.trigger in (ctx.chain_fed or ()):
        report.error(
            row.file,
            row.line,
            "effect_type",
            e.effect_type,
            f"{e.trigger} is fed by a chain row; a chain hop carries no attack, so perform_attack here "
            "could re-fire the trigger that fed the chain without end",
        )
        return None

    combo = ctx.invalid_combo.get((e.trigger, e.target))
    if combo:
        report.error(row.file, row.line, "effect_target", e.target, combo)

    # --- effect
    value = _levels(e, "value", ctx, report)
    duration = _levels(e, "duration", ctx, report, minimum=0)
    amount = _levels(e, "amount", ctx, report)
    radius = _levels(e, "radius", ctx, report, minimum=0)
    scaling_value = parse_scalar(row, "scaling_value", report)
    scaling_type = parse_enum(row, "scaling_type", sorted(SCALING_TYPES), report, required=False)
    if len(report.errors) != errors_before and not combo:
        return None
    if scaling_type and scaling_value is None:
        report.error(row.file, row.line, "scaling_value", "", f"required when scaling_type is {scaling_type}")
        return None
    if scaling_value is not None and not scaling_type:
        report.error(row.file, row.line, "scaling_type", "", "required when scaling_value is set")
        return None
    if ctx.kind == "item" and scaling_type == "distance":
        report.error(
            row.file,
            row.line,
            "scaling_type",
            scaling_type,
            "distance scaling is ability-only (items have no rank-scaled cast range)",
        )
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
        if e.trigger == "while":
            duration = [0.0]
        elif e.trigger == "while_toggled":
            if duration:
                report.error(row.file, row.line, "duration", row.get("duration"), "while_toggled statuses last while toggled; leave blank")
                return None
            duration = [0.0]
        elif not _require(e, "duration", report, "status duration in seconds"):
            return None
        if not no_scaling():
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
        elif effect_id == "shield":
            pool = amount or value
            if not pool:
                report.error(row.file, row.line, "amount", "", "required: shield pool amount")
                return None
            spec = f"StatusSpec::Shield {{ amount: {rank_value(pool)} }}"
            _unused(e, report, "value")
        elif effect_id == "burn":
            if not value:
                report.error(row.file, row.line, "value", "", "required: burn damage (per second, or per tick when amount sets a tick interval)")
                return None
            if not _require(e, "damage_type", report, "physical, magical, or pure"):
                return None
            damage_type = parse_enum(row, "damage_type", list(DAMAGE_TYPES), report, required=False)
            if damage_type is None:
                return None
            tick_interval = "RankValue::ZERO"
            if amount:
                if not _tick_interval_ok(amount, duration, row, report):
                    return None
                tick_interval = rank_value(amount)
            spec = (
                f'StatusSpec::Burn {{ id: "{ctx.owner_id}_burn", amount: {rank_value(value)}, '
                f"damage_type: DamageType::{DAMAGE_TYPES[damage_type]}, tick_interval: {tick_interval} }}"
            )
        elif effect_id == "attack_speed_slow":
            percent = amount or value
            if not percent:
                report.error(row.file, row.line, "amount", "", "required: attack_speed_slow percentage (e.g. 30)")
                return None
            if any(not 0 <= p <= 100 for p in percent):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), "attack_speed_slow percentage must be within 0..100")
                return None
            spec = f"StatusSpec::AttackSpeedSlow {{ percent: {rank_value(percent)} }}"
            if amount:
                _unused(e, report, "value")
        elif effect_id == "turn_rate_slow":
            percent = amount or value
            if not percent:
                report.error(row.file, row.line, "amount", "", "required: turn_rate_slow percentage (e.g. 30)")
                return None
            if any(not 0 <= p <= 100 for p in percent):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), "turn_rate_slow percentage must be within 0..100")
                return None
            spec = f"StatusSpec::TurnRateSlow {{ percent: {rank_value(percent)} }}"
            if amount:
                _unused(e, report, "value")
        elif effect_id == "turn_rate_bonus":
            percent = amount or value
            if not percent:
                report.error(row.file, row.line, "amount", "", "required: turn_rate_bonus percentage (e.g. 50)")
                return None
            if any(not 0 <= p <= 400 for p in percent):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), "turn_rate_bonus percentage must be within 0..400")
                return None
            spec = f"StatusSpec::TurnRateBonus {{ percent: {rank_value(percent)} }}"
            if amount:
                _unused(e, report, "value")
        elif effect_id in ("leash", "tether"):
            if not _require(e, "value", report, f"{effect_id} radius in world units"):
                return None
            if any(r <= 0 for r in value):
                report.error(row.file, row.line, "value", row.get("value"), f"{effect_id} radius must be above 0")
                return None
            anchor = "TriggerPoint" if effect_id == "leash" else "Caster"
            spec = f"StatusSpec::Leash {{ radius: {rank_value(value)}, anchor: LeashAnchorKind::{anchor} }}"
            _unused(e, report, "amount")
        elif effect_id == "heal_over_time":
            if not _require(e, "value", report, "health restored per second"):
                return None
            if any(v <= 0 for v in value):
                report.error(row.file, row.line, "value", row.get("value"), "heal_over_time per-second amount must be above 0")
                return None
            spec = f"StatusSpec::HealOverTime {{ hps: {rank_value(value)} }}"
            _unused(e, report, "amount")
        elif effect_id == "heal_reduction":
            percent = amount or value
            if not percent:
                report.error(row.file, row.line, "amount", "", "required: heal_reduction percentage (e.g. 40)")
                return None
            if any(not 0 <= p <= 100 for p in percent):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), "heal_reduction percentage must be within 0..100")
                return None
            spec = f"StatusSpec::HealReduction {{ percent: {rank_value(percent)} }}"
            if amount:
                _unused(e, report, "value")
        elif effect_id == "mana_shield":
            if not _require(e, "value", report, "percent of damage absorbed by mana (0..100)"):
                return None
            if not _require(e, "amount", report, "damage absorbed per point of mana"):
                return None
            if any(not 0 < p <= 100 for p in value):
                report.error(row.file, row.line, "value", row.get("value"), "mana_shield absorb percent must be above 0 and at most 100")
                return None
            if any(p <= 0 for p in amount):
                report.error(row.file, row.line, "amount", row.get("amount"), "mana_shield damage per mana must be above 0")
                return None
            spec = (
                f"StatusSpec::ManaShield {{ absorb_percent: {rank_value(value)}, "
                f"damage_per_mana: {rank_value(amount)} }}"
            )
        elif effect_id in ("evasion", "accuracy"):
            percent = amount or value
            if not percent:
                report.error(row.file, row.line, "amount", "", f"required: {effect_id} percentage (e.g. 30)")
                return None
            if any(not 0 < p <= 100 for p in percent):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), f"{effect_id} percentage must be above 0 and at most 100")
                return None
            variant = effect_id.capitalize()
            spec = f"StatusSpec::{variant} {{ percent: {rank_value(percent)} }}"
            if amount:
                _unused(e, report, "value")
        elif effect_id == "blind":
            miss = amount or value
            if not miss:
                report.error(row.file, row.line, "amount", "", "required: blind miss chance percent (e.g. 50)")
                return None
            if any(not 0 <= p <= 100 for p in miss):
                report.error(row.file, row.line, "amount" if amount else "value", row.get("amount") or row.get("value"), "blind miss percent must be within 0..100")
                return None
            spec = f"StatusSpec::Blind {{ miss_chance_percent: {rank_value(miss)} }}"
            if amount:
                _unused(e, report, "value")
        else:
            _unused(e, report, "value", "amount")
        if effect_id not in ("burn",):
            _unused(e, report, "damage_type")
        effect = f"AbilityEffect::ApplyStatus {{ status: {spec}, duration: {rank_value(duration)} }}"
    elif t in ("remove_status", "dispel"):
        if not _require(e, "effect_id", report, "buff, debuff, or a status id") or not no_scaling():
            return None
        _unused(e, report, "value", "damage_type", "duration", "amount")
        if effect_id in ("buff", "debuff"):
            beneficial = effect_id == "debuff"
            strong = bool(amount and any(a >= 0.5 for a in amount))
            strength = "DispelStrength::Strong" if strong else "DispelStrength::Basic"
            effect = (
                f"AbilityEffect::Dispel {{ kind: StatusKind::{effect_id.capitalize()}, strength: {strength} }}"
            )
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
        if effect_id == "teleport":
            if e.target not in TELEPORT_TARGETS:
                report.error(row.file, row.line, "effect_target", e.target, f"teleport sends its target to the cast's aim point; use one of: {', '.join(TELEPORT_TARGETS)}")
                return None
            _unused(e, report, "damage_type", "amount", "value", "duration", "radius")
            effect = "AbilityEffect::Teleport"
        else:
            effect = _displacement_effect(e, effect_id, ctx, value, duration, amount, report)
            if effect is None:
                return None
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
            if not _fraction_stat_ok(e, effect_id, value, report):
                return None
            beneficial = True
            effect = f"AbilityEffect::PassiveStat {{ stat: {PASSIVE_STATS[effect_id]}, amount: {rank_value(value)} }}"
        else:
            if effect_id not in TIMED_STATS and effect_id not in RESOURCE_STATS:
                report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"timed stat modifiers support: {', '.join([*TIMED_STATS, *RESOURCE_STATS])}")
                return None
            toggled = e.trigger in ("while_toggled", "while")
            if toggled:
                if duration:
                    report.error(row.file, row.line, "duration", row.get("duration"), "while_toggled bonuses last while toggled; leave blank")
                    return None
                duration = [0.0]
            elif not _require(e, "duration", report, "timed stat modifiers need a duration (only item passives are permanent)"):
                return None
            if effect_id in RESOURCE_STATS:
                if not _fraction_stat_ok(e, effect_id, value, report):
                    return None
                if effect_id in NEGATIVE_IS_BENEFICIAL:
                    beneficial = all(v <= 0 for v in value)
                else:
                    beneficial = all(v >= 0 for v in value)
                kind = "Buff" if beneficial else "Debuff"
                effect = (
                    f'AbilityEffect::ApplyStatus {{ status: StatusSpec::ResourceModifier {{ id: "{ctx.owner_id}_{effect_id}", '
                    f"kind: StatusKind::{kind}, stat: {RESOURCE_STATS[effect_id]}, amount: {rank_value(value)} }}, "
                    f"duration: {rank_value(duration)} }}"
                )
            else:
                beneficial = all(v >= 0 for v in value)
                kind = "Buff" if beneficial else "Debuff"
                fields = {name: "RankValue::ZERO" for name in TIMED_STATS.values()}
                fields[TIMED_STATS[effect_id]] = rank_value(value)
                body = ", ".join(f"{k}: {v}" for k, v in fields.items())
                effect = (
                    f'AbilityEffect::ApplyStatus {{ status: StatusSpec::Modifier {{ id: "{ctx.owner_id}_{effect_id}", '
                    f"kind: StatusKind::{kind}, {body} }}, duration: {rank_value(duration)} }}"
                )
    elif t == "resource":
        _unused(e, report, "duration")
        effect = _resource_effect(e, effect_id, scaling, scaling_type, value, amount, report)
        if effect is None:
            return None
    elif t in GATE_TYPES:
        if not _require(e, "effect_id", report, "the custom trigger to fire (on_<name>)"):
            return None
        if not CUSTOM_TRIGGER_PATTERN.match(effect_id) or effect_id in ABILITY_TRIGGERS or effect_id in ITEM_TRIGGERS:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "must be a custom on_<name> trigger, not a built-in trigger")
            return None
        if effect_id == e.trigger:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"a {t} cannot fire its own trigger")
            return None
        if e.target not in ("caster", "owner", "attacker"):
            report.error(row.file, row.line, "effect_target", e.target, f"{t} acts on the unit using it; use owner, caster, or attacker")
            return None
        if ctx.owner_triggers is not None and effect_id not in ctx.owner_triggers:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"no row of {e.owner!r} uses trigger {effect_id}, so this {t} would do nothing")
            return None
        if not no_scaling():
            return None
        _unused(e, report, "damage_type", "duration", "amount", "radius")
        if t in ("proc", "pseudo_proc"):
            if not _require(e, "value", report, "proc chance percent (above 0, at most 100)"):
                return None
            if any(not 0 < p <= 100 for p in value):
                report.error(row.file, row.line, "value", row.get("value"), "proc chance percent must be above 0 and at most 100")
                return None
            chance = rank_value([p / 100 for p in value])
            pseudo = "true" if t == "pseudo_proc" else "false"
            effect = f"AbilityEffect::Proc {{ chance: {chance}, then: AbilityTrigger::Custom(\"{effect_id}\"), pseudo: {pseudo} }}"
        elif t == "accumulate":
            if not _require(e, "value", report, "threshold to reach (above 0), e.g. 1000 units or 300 damage"):
                return None
            if any(v <= 0 for v in value):
                report.error(row.file, row.line, "value", row.get("value"), "accumulate threshold must be above 0")
                return None
            effect = f"AbilityEffect::Accumulate {{ threshold: {rank_value(value)}, then: AbilityTrigger::Custom(\"{effect_id}\") }}"
        else:
            if not _require(e, "value", report, "attacks between firings (integer, at least 2)"):
                return None
            if len(value) != 1 or not float(value[0]).is_integer() or value[0] < 2:
                report.error(row.file, row.line, "value", row.get("value"), "counter needs a single integer of at least 2")
                return None
            effect = f"AbilityEffect::Counter {{ every: {int(value[0])}, then: AbilityTrigger::Custom(\"{effect_id}\") }}"
    elif t == "chain":
        if not _require(e, "effect_id", report, "the custom trigger to fire per hop (on_<name>)"):
            return None
        if not CUSTOM_TRIGGER_PATTERN.match(effect_id) or effect_id in ABILITY_TRIGGERS or effect_id in ITEM_TRIGGERS:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "must be a custom on_<name> trigger, not a built-in trigger")
            return None
        if effect_id == e.trigger:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "a chain cannot fire its own trigger")
            return None
        if e.target in ("affected_units", "units_in_radius"):
            report.error(row.file, row.line, "effect_target", e.target, "a chain starts from one unit; use cast_target, trigger_unit, attacker, owner, or caster")
            return None
        if ctx.owner_triggers is not None and effect_id not in ctx.owner_triggers:
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), f"no row of {e.owner!r} uses trigger {effect_id}, so this chain would do nothing")
            return None
        if not no_scaling():
            return None
        if not _require(e, "value", report, "number of jumps (integer, at least 1)"):
            return None
        if len(value) != 1 or not float(value[0]).is_integer() or value[0] < 1:
            report.error(row.file, row.line, "value", row.get("value"), "chain needs a single integer number of jumps of at least 1")
            return None
        if not _require(e, "radius", report, "jump radius in world units"):
            return None
        if any(r <= 0 for r in radius):
            report.error(row.file, row.line, "radius", row.get("radius"), "chain jump radius must be above 0")
            return None
        if not _require(e, "amount", report, "percent of the magnitude kept per jump (above 0, at most 100)"):
            return None
        if len(amount) != 1 or not 0 < amount[0] <= 100:
            report.error(row.file, row.line, "amount", row.get("amount"), "chain needs a single percent above 0 and at most 100")
            return None
        _unused(e, report, "damage_type", "duration")
        chain_team = ctx.team if ctx.team in ("Enemy", "Ally", "Any") else "Enemy"
        effect = (
            f"AbilityEffect::Chain {{ jumps: {int(value[0])}, radius: {rank_value(radius)}, "
            f"falloff: {f32(amount[0] / 100)}, team: TargetTeam::{chain_team}, "
            f'then: AbilityTrigger::Custom("{effect_id}") }}'
        )
    elif t == "perform_attack":
        if effect_id not in ("", "exclude_trigger_unit"):
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "perform_attack effect_id must be blank or exclude_trigger_unit")
            return None
        if not no_scaling():
            return None
        percent = value or [100.0]
        if any(p <= 0 for p in percent):
            report.error(row.file, row.line, "value", row.get("value"), "perform_attack damage percent must be above 0")
            return None
        max_targets = 0
        if amount:
            if len(amount) != 1 or not float(amount[0]).is_integer() or amount[0] < 0:
                report.error(row.file, row.line, "amount", row.get("amount"), "max targets must be a single integer of at least 0 (0 = all)")
                return None
            max_targets = int(amount[0])
        _unused(e, report, "damage_type", "duration")
        effect = (
            f"AbilityEffect::PerformAttack {{ damage_ratio: {rank_value([p / 100 for p in percent])}, "
            f"max_targets: {max_targets}, exclude_trigger_unit: {'true' if effect_id else 'false'} }}"
        )
    elif t == "custom":
        if not _require(e, "effect_id", report, "registered custom effect id"):
            return None
        if not ID_PATTERN.match(effect_id):
            report.error(row.file, row.line, "effect_id", row.get("effect_id"), "custom effect ids must be snake_case")
            return None
        if ctx.registered_custom is not None and effect_id not in ctx.registered_custom:
            report.warn(
                row.file,
                row.line,
                "effect_id",
                row.get("effect_id"),
                "custom effect id is not registered via register_custom_effect in src/ (may fail at runtime)",
            )
        if not no_scaling():
            return None
        _unused(e, report, "damage_type")
        effect = (
            f'AbilityEffect::Custom {{ id: "{effect_id}", value: {rank_value(value)}, '
            f"amount: {rank_value(amount)}, duration: {rank_value(duration)} }}"
        )
    else:  # pragma: no cover - parse_enum already rejected it
        return None

    if t not in ("movement", "chain") and e.target != "units_in_radius" and row.get("radius"):
        report.warn(row.file, row.line, "radius", row.get("radius"), "only used by units_in_radius targets")

    # --- target
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
        if e.trigger in ATTACKER_TRIGGERS:
            target = "EffectTarget::TriggerUnit"
        elif e.trigger in ("on_attack", "on_attack_hit"):
            target = "EffectTarget::Caster"
        else:
            report.error(row.file, row.line, "effect_target", target_name, "attacker is only defined for on_attack, on_attack_hit, on_damage_taken, and on_death")
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
