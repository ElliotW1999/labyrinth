"""Unit tests for shared generator validation."""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from generator_common import (  # noqa: E402
    ActivationSpec,
    EffectContext,
    EffectRow,
    Report,
    Row,
    ABILITY_TRIGGERS,
    GLOBAL_CAST_RANGE,
    ITEM_TRIGGERS,
    NOT_EMITTED_TRIGGERS,
    effect_to_rust,
    held_status_id,
    runtime_status_id,
    chain_fed_triggers,
    reaction_triggers,
    warn_duplicate_held_statuses,
    warn_unemitted_triggers,
    warn_unreachable_custom_triggers,
    parse_activation_spec,
    projectile_each_mechanic_rust,
    projectile_mechanic_rust,
    validate_activation,
)


def row(cells: dict[str, str], line: int = 2) -> Row:
    return Row("test.csv", line, cells)


class GlobalCastRangeTests(unittest.TestCase):
    def _spec(self, kind: str, cast_range: str, target_type: str = "point"):
        report = Report()
        spec = parse_activation_spec(
            row({"target_type": target_type, "target_team": "enemy", "cast_range": cast_range}),
            kind=kind,
            has_active_effects=True,
            report=report,
            levels=kind == "ability",
        )
        return spec, report

    def test_global_is_case_insensitive_for_abilities_and_items(self) -> None:
        for text in ("global", "GLOBAL", " Global "):
            spec, report = self._spec("ability", text)
            self.assertFalse(report.errors, text)
            self.assertEqual(spec.cast_range, [GLOBAL_CAST_RANGE])
        spec, report = self._spec("item", "global")
        self.assertFalse(report.errors)
        self.assertEqual(spec.cast_range_scalar, GLOBAL_CAST_RANGE)

    def test_global_cannot_join_a_level_list(self) -> None:
        spec, report = self._spec("ability", "800|global")
        self.assertTrue(report.errors)
        self.assertEqual(report.errors[0].field, "cast_range")
        self.assertIn("level list", report.errors[0].message)

    def test_global_satisfies_the_cast_range_requirement(self) -> None:
        for target_type in ("point", "unit", "direction"):
            spec, report = self._spec("ability", "global", target_type)
            validate_activation(
                row({"target_type": target_type, "target_team": "enemy", "cast_range": "global"}),
                spec,
                kind="ability",
                ability_type="active",
                report=report,
            )
            self.assertFalse(report.errors, target_type)


class BoundedTeleportTests(unittest.TestCase):
    @staticmethod
    def _teleport(target: str = "caster", **cells: str):
        base = {
            "ability_id": "mover",
            "effect_index": "0",
            "trigger": "on_cast",
            "effect_target": target,
            "effect_type": "movement",
            "effect_id": "teleport",
        }
        base.update(cells)
        effect = EffectRow(row(base), "mover", 0, "on_cast", target, "movement")
        ctx = EffectContext(kind="ability", owner_id="mover", levels=1, center_for={}, target_type="point")
        report = Report()
        return effect_to_rust(effect, ctx, report), report

    def test_blank_bounds_keep_the_plain_teleport(self) -> None:
        result, report = self._teleport()
        self.assertFalse(report.errors)
        self.assertIn("Teleport { min_distance: RankValue::ZERO, max_distance: None }", result[2])

    def test_value_is_the_maximum_and_amount_the_minimum(self) -> None:
        result, report = self._teleport(value="1075", amount="200")
        self.assertFalse(report.errors)
        self.assertIn("min_distance: RankValue::fixed(200.0)", result[2])
        self.assertIn("max_distance: Some(RankValue::fixed(1075.0))", result[2])

    def test_minimum_above_maximum_is_rejected(self) -> None:
        result, report = self._teleport(value="200", amount="500")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "amount")
        self.assertIn("exceeds", report.errors[0].message)

    def test_bounds_need_the_caster_as_target(self) -> None:
        result, report = self._teleport(target="cast_target", value="800")
        self.assertIsNone(result)
        self.assertIn("caster", report.errors[0].message)

    def test_max_must_be_positive_and_min_not_negative(self) -> None:
        result, report = self._teleport(value="0")
        self.assertIsNone(result)
        result, report = self._teleport(amount="-1")
        self.assertIsNone(result)


class ValidateActivationTests(unittest.TestCase):
    def test_teleport_item_requires_point_target(self) -> None:
        report = Report()
        spec = ActivationSpec(target_type="no_target", target_team="self")
        validate_activation(row({"target_type": "no_target"}), spec, kind="item", ability_type=None, report=report)
        self.assertFalse(report.errors)

    def test_projectile_speed_requires_projectile_type(self) -> None:
        report = Report()
        spec = ActivationSpec(
            target_type="unit",
            target_team="enemy",
            cast_range_scalar=600.0,
            projectile_type="none",
            projectile_speed=900.0,
        )
        validate_activation(
            row({"target_type": "unit", "target_team": "enemy", "cast_range": "600", "projectile_speed": "900"}),
            spec,
            kind="item",
            ability_type=None,
            report=report,
        )
        self.assertEqual(len(report.errors), 1)
        self.assertIn("projectile_speed", report.errors[0].field or "")

    def test_passive_ability_skips_cast_range(self) -> None:
        report = Report()
        spec = ActivationSpec(target_type="no_target", target_team="self", projectile_type="none")
        validate_activation(
            row({"target_type": "no_target", "target_team": "self", "ability_type": "passive"}),
            spec,
            kind="ability",
            ability_type="passive",
            report=report,
        )
        self.assertFalse(report.errors)

    def test_unit_target_requires_cast_range(self) -> None:
        report = Report()
        spec = ActivationSpec(target_type="point", target_team="enemy")
        validate_activation(row({"target_type": "point", "target_team": "enemy"}), spec, kind="item", ability_type=None, report=report)
        self.assertEqual(len(report.errors), 1)
        self.assertIn("cast_range", report.errors[0].field or "")


class ItemEffectScalingTests(unittest.TestCase):
    @staticmethod
    def _damage_row(kind: str, scaling_type: str) -> tuple[EffectRow, EffectContext]:
        cells = {
            "item_id": "potion",
            "effect_index": "0",
            "trigger": "on_use",
            "effect_target": "owner",
            "effect_type": "damage",
            "value": "10",
            "damage_type": "magical",
            "scaling_type": scaling_type,
            "scaling_value": "0.5" if scaling_type else "",
        }
        effect = EffectRow(row(cells), "potion", 0, "on_use", "owner", "damage")
        ctx = EffectContext(
            kind=kind,
            owner_id="potion",
            levels=None if kind == "item" else 1,
            center_for={"on_use": "Caster"},
        )
        return effect, ctx

    def test_item_distance_scaling_rejected(self) -> None:
        report = Report()
        effect, ctx = self._damage_row("item", "distance")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(len(report.errors), 1)
        self.assertEqual(report.errors[0].field, "scaling_type")
        self.assertIn("ability-only", report.errors[0].message)

    def test_item_other_scaling_still_accepted(self) -> None:
        report = Report()
        effect, ctx = self._damage_row("item", "strength")
        self.assertIsNotNone(effect_to_rust(effect, ctx, report))
        self.assertFalse(report.errors)


class WhileToggledTests(unittest.TestCase):
    @staticmethod
    def _effect(duration: str = "", owner_is_toggle: bool = True) -> tuple[EffectRow, EffectContext]:
        cells = {
            "ability_id": "overcharge",
            "effect_index": "0",
            "trigger": "while_toggled",
            "effect_target": "caster",
            "effect_type": "stat_modifier",
            "effect_id": "move_speed",
            "value": "10",
            "duration": duration,
        }
        effect = EffectRow(row(cells), "overcharge", 0, "while_toggled", "caster", "stat_modifier")
        ctx = EffectContext(kind="ability", owner_id="overcharge", levels=7, center_for={})
        if not owner_is_toggle:
            # Mirrors AbilityGenerator.effect_context for non-toggle abilities.
            ctx.invalid_trigger["while_toggled"] = "while_toggled is only for ability_type toggle"
        return effect, ctx

    def test_while_toggled_accepted_with_zero_duration(self) -> None:
        report = Report()
        effect, ctx = self._effect()
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("AbilityTrigger::WhileToggled", result[0])
        self.assertIn("duration: RankValue::fixed(0.0)", result[2])

    def test_while_toggled_rejected_on_non_toggle_ability(self) -> None:
        report = Report()
        effect, ctx = self._effect(owner_is_toggle=False)
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("only for ability_type toggle", report.errors[0].message)

    def test_while_toggled_rejects_a_duration(self) -> None:
        report = Report()
        effect, ctx = self._effect(duration="5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "duration")
        self.assertIn("while toggled", report.errors[0].message)


class TriggerTableTests(unittest.TestCase):
    def test_both_trigger_tables_accept_on_channel_interrupted(self) -> None:
        self.assertEqual(ABILITY_TRIGGERS["on_channel_interrupted"], "OnChannelInterrupted")
        self.assertEqual(ITEM_TRIGGERS["on_channel_interrupted"], "OnChannelInterrupted")
        self.assertEqual(ABILITY_TRIGGERS["on_channel_end"], "OnChannelEnd")


class ItemChargeTests(unittest.TestCase):
    def test_item_charge_restore_time_rejected(self) -> None:
        report = Report()
        cells = {"target_type": "no_target", "target_team": "self", "max_charges": "3", "charge_restore_time": "30"}
        spec = parse_activation_spec(row(cells), kind="item", has_active_effects=True, report=report, levels=False)
        validate_activation(row(cells), spec, kind="item", ability_type=None, report=report)
        errors = [e for e in report.errors if e.field == "charge_restore_time"]
        self.assertEqual(len(errors), 1)
        self.assertIn("do not regenerate", errors[0].message)

    def test_item_charges_without_restore_time_are_fine(self) -> None:
        report = Report()
        cells = {"target_type": "no_target", "target_team": "self", "max_charges": "3"}
        spec = parse_activation_spec(row(cells), kind="item", has_active_effects=True, report=report, levels=False)
        validate_activation(row(cells), spec, kind="item", ability_type=None, report=report)
        self.assertFalse(report.errors)
        self.assertFalse(report.warnings)


class ParseActivationSpecTests(unittest.TestCase):
    def test_item_without_on_use_warns_orphan_columns(self) -> None:
        report = Report()
        spec = parse_activation_spec(
            row({"target_type": "point", "cast_range": "800"}),
            kind="item",
            has_active_effects=False,
            report=report,
        )
        self.assertIsNone(spec)
        self.assertTrue(report.warnings)


class MovementEffectTests(unittest.TestCase):
    @staticmethod
    def _movement(
        effect_id: str,
        target: str = "caster",
        trigger: str = "on_cast",
        target_type: str | None = "point",
        kind: str = "ability",
        **cells: str,
    ) -> tuple[EffectRow, EffectContext]:
        base = {
            "ability_id": "mover",
            "effect_index": "0",
            "trigger": trigger,
            "effect_target": target,
            "effect_type": "movement",
            "effect_id": effect_id,
        }
        base.update(cells)
        effect = EffectRow(row(base), "mover", 0, trigger, target, "movement")
        ctx = EffectContext(
            kind=kind,
            owner_id="mover",
            levels=None if kind == "item" else 1,
            center_for={},
            target_type=target_type,
        )
        return effect, ctx

    def test_movement_dash_requires_caster_target(self) -> None:
        report = Report()
        effect, ctx = self._movement("dash", target="cast_target", value="400")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "effect_target")

        report = Report()
        effect, ctx = self._movement("dash", value="")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "value")

        report = Report()
        effect, ctx = self._movement("dash", target="owner", value="400")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("DisplacementKind::Dash", result[2])
        self.assertIn("distance: Some(RankValue::fixed(400.0))", result[2])
        self.assertIn("duration: RankValue::fixed(0.3)", result[2])
        self.assertIn("speed: None", result[2])

    def test_charge_requires_unit_target_and_speed(self) -> None:
        report = Report()
        effect, ctx = self._movement("charge", target_type="point", amount="900", duration="2")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("target_type unit", report.errors[0].message)

        report = Report()
        effect, ctx = self._movement("charge", target_type="unit", duration="2")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "amount")

        report = Report()
        effect, ctx = self._movement("charge", target_type="unit", amount="900")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "duration")

        report = Report()
        effect, ctx = self._movement("charge", target_type="unit", amount="900", duration="2")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("DisplacementKind::Charge", result[2])
        self.assertIn("speed: Some(RankValue::fixed(900.0))", result[2])
        self.assertIn("distance: None", result[2])

    def test_drag_rejects_caster(self) -> None:
        report = Report()
        effect, ctx = self._movement("drag", target="caster", trigger="on_unit_contact")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "effect_target")

        report = Report()
        effect, ctx = self._movement("drag", target="trigger_unit", trigger="on_unit_contact")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("DisplacementKind::Drag", result[2])
        self.assertIn("duration: RankValue::fixed(0.0)", result[2])

    def test_leash_requires_radius(self) -> None:
        cells = {
            "ability_id": "mover",
            "effect_index": "0",
            "trigger": "on_cast",
            "effect_target": "units_in_radius",
            "effect_type": "apply_status",
            "effect_id": "leash",
            "duration": "4",
            "radius": "300",
        }
        ctx = EffectContext(kind="ability", owner_id="mover", levels=1, center_for={}, has_aoe=True)

        report = Report()
        effect = EffectRow(row(cells), "mover", 0, "on_cast", "units_in_radius", "apply_status")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "value")

        report = Report()
        effect = EffectRow(row({**cells, "value": "500"}), "mover", 0, "on_cast", "units_in_radius", "apply_status")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("LeashAnchorKind::TriggerPoint", result[2])

        report = Report()
        tether = {**cells, "effect_id": "tether", "value": "500"}
        effect = EffectRow(row(tether), "mover", 0, "on_cast", "units_in_radius", "apply_status")
        self.assertIn("LeashAnchorKind::Caster", effect_to_rust(effect, ctx, report)[2])

    def test_on_movement_end_in_trigger_tables(self) -> None:
        self.assertEqual(ABILITY_TRIGGERS["on_movement_end"], "OnMovementEnd")
        self.assertEqual(ITEM_TRIGGERS["on_movement_end"], "OnMovementEnd")
        self.assertEqual(ITEM_TRIGGERS["on_unit_contact"], "OnUnitContact")

    def test_cyclone_requires_duration(self) -> None:
        report = Report()
        effect, ctx = self._movement("cyclone", target="cast_target", target_type="unit")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "duration")

        report = Report()
        effect, ctx = self._movement("cyclone", target="cast_target", target_type="unit", duration="2.5")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("DisplacementKind::Cyclone", result[2])
        self.assertIn("duration: RankValue::fixed(2.5)", result[2])

    def test_flying_and_turn_rate_bonus_statuses(self) -> None:
        cells = {
            "ability_id": "mover",
            "effect_index": "0",
            "trigger": "on_cast",
            "effect_target": "caster",
            "effect_type": "apply_status",
            "duration": "4",
        }
        ctx = EffectContext(kind="ability", owner_id="mover", levels=1, center_for={})
        report = Report()
        effect = EffectRow(row({**cells, "effect_id": "flying"}), "mover", 0, "on_cast", "caster", "apply_status")
        self.assertIn("StatusSpec::Flying", effect_to_rust(effect, ctx, report)[2])
        self.assertFalse(report.errors)

        report = Report()
        bonus = {**cells, "effect_id": "turn_rate_bonus", "amount": "500"}
        effect = EffectRow(row(bonus), "mover", 0, "on_cast", "caster", "apply_status")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("0..400", report.errors[0].message)

        report = Report()
        effect = EffectRow(row({**bonus, "amount": "100"}), "mover", 0, "on_cast", "caster", "apply_status")
        self.assertIn("StatusSpec::TurnRateBonus", effect_to_rust(effect, ctx, report)[2])


class ProjectileTypeTests(unittest.TestCase):
    def test_skillshot_single_validates_like_a_skillshot(self) -> None:
        for target_type, ok in (("point", True), ("direction", True), ("unit", False)):
            report = Report()
            cells = {
                "target_type": target_type,
                "target_team": "enemy",
                "cast_range": "900",
                "projectile_type": "skillshot_single",
            }
            spec = parse_activation_spec(row(cells), kind="ability", has_active_effects=True, report=report, levels=1)
            validate_activation(row(cells), spec, kind="ability", ability_type="active", report=report)
            self.assertEqual(not report.errors, ok, target_type)
            self.assertEqual(spec.projectile_type, "skillshot_single")

    def _end_radius(self, kind: str = "ability", **overrides: str):
        cells = {
            "target_type": "point",
            "target_team": "enemy",
            "cast_range": "900",
            "aoe_radius": "100",
            "projectile_type": "skillshot",
            "projectile_end_radius": "450",
        }
        cells.update(overrides)
        report = Report()
        spec = parse_activation_spec(row(cells), kind=kind, has_active_effects=True, report=report, levels=kind == "ability")
        validate_activation(row(cells), spec, kind=kind, ability_type="active", report=report)
        return spec, report

    def test_end_radius_emits_into_the_projectile_mechanic(self) -> None:
        spec, report = self._end_radius()
        self.assertFalse(report.errors)
        rust = projectile_mechanic_rust(spec.projectile_type, 642.857, spec.projectile_end_radius)
        self.assertIn("end_radius: Some(RankValue::fixed(450.0))", rust)
        self.assertIn("end_radius: None", projectile_mechanic_rust("skillshot", None))

    def test_end_radius_is_rejected_on_non_skillshots_and_without_aoe_radius(self) -> None:
        for overrides in (
            {"target_type": "unit", "projectile_type": "targeted"},
            {"projectile_type": "none"},
            {"aoe_radius": ""},
            {"projectile_end_radius": "0"},
        ):
            _, report = self._end_radius(**overrides)
            self.assertTrue(report.errors, overrides)
            self.assertEqual(report.errors[0].field, "projectile_end_radius", overrides)

    def test_items_reject_end_radius_until_they_have_an_aoe_radius(self) -> None:
        _, report = self._end_radius(kind="item", aoe_radius="")
        self.assertIn("items have no aoe_radius", report.errors[0].message)

    def _each(self, kind: str = "ability", **overrides: str):
        cells = {
            "target_type": "no_target",
            "target_team": "enemy",
            "aoe_radius": "600",
            "projectile_type": "targeted_each",
            "projectile_speed": "900",
        }
        if kind == "item":
            del cells["aoe_radius"]
        cells.update(overrides)
        report = Report()
        spec = parse_activation_spec(row(cells), kind=kind, has_active_effects=True, report=report, levels=kind == "ability")
        validate_activation(row(cells), spec, kind=kind, ability_type="active", report=report)
        return spec, report

    def test_targeted_each_accepts_no_target_and_point_abilities(self) -> None:
        for target_type, center in (("no_target", "Caster"), ("point", "Aim")):
            spec, report = self._each(target_type=target_type, cast_range="900")
            self.assertFalse(report.errors, target_type)
            rust = projectile_each_mechanic_rust(target_type, "enemy", spec.projectile_speed)
            self.assertIn(f"center: AreaCenter::{center}", rust)
            self.assertIn("team: TargetTeam::Enemy", rust)
            self.assertIn("speed: Some(900.0)", rust)

    def test_targeted_each_needs_aoe_radius_and_a_valid_target_type(self) -> None:
        _, report = self._each(aoe_radius="")
        self.assertEqual(report.errors[0].field, "aoe_radius")
        _, report = self._each(aoe_radius="0")
        self.assertEqual(report.errors[0].field, "aoe_radius")
        _, report = self._each(target_type="unit", cast_range="600")
        self.assertEqual(report.errors[0].field, "projectile_type")
        _, report = self._each(target_team="self")
        self.assertTrue(report.errors)

    def test_items_reject_targeted_each(self) -> None:
        _, report = self._each(kind="item", target_type="no_target", target_team="enemy")
        self.assertEqual(report.errors[0].field, "projectile_type")
        self.assertIn("items do not have yet", report.errors[0].message)


def _effect(
    effect_type: str,
    effect_id: str,
    *,
    target: str = "cast_target",
    trigger: str = "on_cast",
    kind: str = "ability",
    **cells: str,
) -> tuple[EffectRow, EffectContext]:
    base = {
        "ability_id": "tester",
        "effect_index": "0",
        "trigger": trigger,
        "effect_target": target,
        "effect_type": effect_type,
        "effect_id": effect_id,
    }
    base.update(cells)
    name, *trigger_args = trigger.split(":")
    effect = EffectRow(row(base), "tester", 0, name, target, effect_type, tuple(trigger_args))
    ctx = EffectContext(
        kind=kind,
        owner_id="tester",
        levels=None if kind == "item" else 1,
        center_for={},
        target_type="unit",
    )
    return effect, ctx


class ResourceEffectTests(unittest.TestCase):
    def test_resource_effects_health_drain(self) -> None:
        report = Report()
        effect, ctx = _effect("resource", "health_drain", value="80", damage_type="magical", amount="0.5")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("AbilityEffect::DrainHealth", result[2])
        self.assertIn("transfer_ratio: 0.5", result[2])
        self.assertIn("DamageType::Magical", result[2])

        report = Report()
        effect, ctx = _effect("resource", "health_drain", value="80", damage_type="magical")
        self.assertIn("transfer_ratio: 1.0", effect_to_rust(effect, ctx, report)[2])

        report = Report()
        effect, ctx = _effect("resource", "health_drain", value="80", damage_type="magical", scaling_type="intelligence", scaling_value="1")
        self.assertIn("EffectScaling::CasterAttribute", effect_to_rust(effect, ctx, report)[2])

        report = Report()
        effect, ctx = _effect("resource", "health_drain", value="80")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "damage_type")

    def test_resource_effects_mana_burn_and_drain(self) -> None:
        report = Report()
        effect, ctx = _effect("resource", "mana_burn", value="100", damage_type="pure", amount="0.8")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("burn_ratio: 0.8", result[2])
        self.assertIn("transfer_ratio: 0.0", result[2])

        report = Report()
        effect, ctx = _effect("resource", "mana_burn", value="100")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "damage_type")

        report = Report()
        effect, ctx = _effect("resource", "mana_drain", value="60", amount="0.5")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("burn_ratio: 0.0", result[2])
        self.assertIn("transfer_ratio: 0.5", result[2])

    def test_resource_effects_reject_scaling_on_mana_drain(self) -> None:
        report = Report()
        effect, ctx = _effect("resource", "mana_drain", value="60", scaling_type="intelligence", scaling_value="1")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "scaling_type")

    def test_resource_effects_hostile_kinds_reject_self_targets(self) -> None:
        for effect_id, extra in (("mana_drain", {}), ("mana_burn", {"damage_type": "pure"}), ("health_drain", {"damage_type": "pure"})):
            report = Report()
            effect, ctx = _effect("resource", effect_id, target="caster", value="10", **extra)
            self.assertIsNone(effect_to_rust(effect, ctx, report), effect_id)
            self.assertEqual(report.errors[0].field, "effect_target")

    def test_spend_requires_self_target(self) -> None:
        for effect_id in ("spend_health", "spend_mana"):
            report = Report()
            effect, ctx = _effect("resource", effect_id, target="cast_target", value="50")
            self.assertIsNone(effect_to_rust(effect, ctx, report))
            self.assertEqual(report.errors[0].field, "effect_target")
            self.assertIn("own resource", report.errors[0].message)

            report = Report()
            effect, ctx = _effect("resource", effect_id, target="owner", value="50")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors)
            self.assertIn("AbilityEffect::SpendResource", result[2])
            self.assertIn("convert_to: None", result[2])

    def test_resource_effects_conversion_needs_a_ratio(self) -> None:
        report = Report()
        effect, ctx = _effect("resource", "health_to_mana", target="owner", value="100")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "amount")

        report = Report()
        effect, ctx = _effect("resource", "health_to_mana", target="owner", value="100", amount="0.5")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("resource: ResourceKind::Health", result[2])
        self.assertIn("convert_to: Some((ResourceKind::Mana, 0.5))", result[2])

        report = Report()
        effect, ctx = _effect("resource", "mana_to_health", target="owner", value="100", amount="2")
        result = effect_to_rust(effect, ctx, report)
        self.assertIn("resource: ResourceKind::Mana", result[2])
        self.assertIn("convert_to: Some((ResourceKind::Health, 2.0))", result[2])

    def test_resource_effects_unknown_id_and_nonpositive_amount(self) -> None:
        report = Report()
        effect, ctx = _effect("resource", "mana_everything", value="10")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "effect_id")

        report = Report()
        effect, ctx = _effect("resource", "mana_drain", value="0")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "value")


class HealStatusTests(unittest.TestCase):
    def test_heal_statuses_heal_over_time(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "heal_over_time", target="caster", value="25", duration="6")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::HealOverTime { hps: RankValue::fixed(25.0) }", result[2])

        report = Report()
        effect, ctx = _effect("apply_status", "heal_over_time", target="caster", duration="6")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "value")

    def test_heal_statuses_heal_reduction_percent_range(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "heal_reduction", amount="40", duration="5")
        self.assertIn("StatusSpec::HealReduction { percent: RankValue::fixed(40.0) }", effect_to_rust(effect, ctx, report)[2])
        self.assertFalse(report.errors)

        report = Report()
        effect, ctx = _effect("apply_status", "heal_reduction", amount="140", duration="5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("0..100", report.errors[0].message)

        report = Report()
        effect, ctx = _effect("apply_status", "heal_reduction", duration="5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "amount")

    def test_heal_statuses_heal_block(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "heal_block", duration="3")
        self.assertIn("StatusSpec::HealBlock", effect_to_rust(effect, ctx, report)[2])
        self.assertFalse(report.errors)

    def test_heal_statuses_mana_shield(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "mana_shield", target="caster", value="50", amount="2", duration="8")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::ManaShield { absorb_percent: RankValue::fixed(50.0), damage_per_mana: RankValue::fixed(2.0) }", result[2])

        report = Report()
        effect, ctx = _effect("apply_status", "mana_shield", target="caster", value="50", duration="8")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "amount")

        report = Report()
        effect, ctx = _effect("apply_status", "mana_shield", target="caster", value="150", amount="2", duration="8")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "value")

    def test_heal_statuses_mana_shield_works_while_toggled(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "mana_shield", target="caster", trigger="while_toggled", value="50", amount="2")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("AbilityTrigger::WhileToggled", result[0])
        self.assertIn("duration: RankValue::fixed(0.0)", result[2])

        report = Report()
        effect, ctx = _effect("apply_status", "mana_shield", target="caster", trigger="while_toggled", value="50", amount="2", duration="5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "duration")

    def test_while_toggled_still_rejects_other_statuses(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "stun", target="caster", trigger="while_toggled", duration="2")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "effect_type")


class BurnStatusTests(unittest.TestCase):
    def _burn(self, **cells: str):
        base = {"damage_type": "magical", "duration": "16"}
        base.update(cells)
        report = Report()
        effect, ctx = _effect("apply_status", "burn", **base)
        return effect_to_rust(effect, ctx, report), report

    def test_continuous_burn_keeps_a_zero_interval_and_per_owner_id(self) -> None:
        result, report = self._burn(value="12")
        self.assertFalse(report.errors)
        self.assertIn('id: "tester_burn"', result[2])
        self.assertIn("tick_interval: RankValue::ZERO", result[2])

    def test_ticking_burn_emits_the_interval(self) -> None:
        result, report = self._burn(value="20", amount="3")
        self.assertFalse(report.errors)
        self.assertIn("amount: RankValue::fixed(20.0)", result[2])
        self.assertIn("tick_interval: RankValue::fixed(3.0)", result[2])

    def test_burn_requires_value_and_no_longer_falls_back_to_amount(self) -> None:
        result, report = self._burn(amount="3")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "value")

    def test_burn_interval_must_be_above_zero(self) -> None:
        result, report = self._burn(value="20", amount="0")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "amount")
        self.assertIn("above 0", report.errors[0].message)

    def test_burn_interval_cannot_exceed_the_duration(self) -> None:
        result, report = self._burn(value="20", amount="17")
        self.assertIsNone(result)
        self.assertIn("exceeds", report.errors[0].message)

    def test_owner_burn_ids_are_valid_trigger_arguments(self) -> None:
        self.assertEqual(runtime_status_id("shadow_strike_burn"), "shadow_strike_burn")
        self.assertEqual(runtime_status_id("burn"), "burn")


class ResourceStatModifierTests(unittest.TestCase):
    def test_resource_stat_modifiers_passive_items(self) -> None:
        for stat, variant in (
            ("lifesteal", "Lifesteal"),
            ("spell_lifesteal", "SpellLifesteal"),
            ("heal_amp", "HealAmp"),
            ("heal_received_amp", "HealReceivedAmp"),
            ("mana_cost_amp", "ManaCostAmp"),
        ):
            report = Report()
            effect, ctx = _effect("stat_modifier", stat, target="owner", trigger="passive", kind="item", value="0.2")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, stat)
            self.assertIn(f"PassiveStat::{variant}", result[2])

    def test_resource_stat_modifiers_ranges(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "lifesteal", target="owner", trigger="passive", kind="item", value="1.5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("-1..1", report.errors[0].message)

        report = Report()
        effect, ctx = _effect("stat_modifier", "mana_cost_amp", target="owner", trigger="passive", kind="item", value="-1.5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))

        report = Report()
        effect, ctx = _effect("stat_modifier", "mana_cost_amp", target="owner", trigger="passive", kind="item", value="2")
        self.assertIsNotNone(effect_to_rust(effect, ctx, report), "a cost increase above 100% is allowed")

    def test_resource_stat_modifiers_timed(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "max_health", target="caster", value="300", duration="10")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::ResourceModifier", result[2])
        self.assertIn('id: "tester_max_health"', result[2])
        self.assertIn("stat: ResourceStat::MaxHealth", result[2])
        self.assertIn("StatusKind::Buff", result[2])

        report = Report()
        effect, ctx = _effect("stat_modifier", "mana_cost_amp", target="caster", value="-0.3", duration="10")
        self.assertIn("StatusKind::Buff", effect_to_rust(effect, ctx, report)[2], "cheaper costs are a buff")

        report = Report()
        effect, ctx = _effect("stat_modifier", "heal_received_amp", target="cast_target", value="-0.3", duration="10")
        self.assertIn("StatusKind::Debuff", effect_to_rust(effect, ctx, report)[2])

        report = Report()
        effect, ctx = _effect("stat_modifier", "health_regen", target="caster", value="5")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "duration")

    def test_resource_stat_modifiers_toggled_and_legacy_timed_stats_unchanged(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "mana_regen", target="caster", trigger="while_toggled", value="3")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("stat: ResourceStat::ManaRegen", result[2])

        report = Report()
        effect, ctx = _effect("stat_modifier", "armor", target="caster", value="5", duration="4")
        result = effect_to_rust(effect, ctx, report)
        self.assertIn("StatusSpec::Modifier", result[2])


class AttackModifierTests(unittest.TestCase):
    def test_ability_attack_triggers_accepted(self) -> None:
        self.assertEqual(ABILITY_TRIGGERS["on_attack"], "OnAttack")
        self.assertEqual(ABILITY_TRIGGERS["on_attack_hit"], "OnAttackHit")
        report = Report()
        effect, ctx = _effect("damage", "", target="trigger_unit", trigger="on_attack_hit", value="20", damage_type="physical")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("AbilityTrigger::OnAttackHit", result[0])

    def test_proc_requires_custom_trigger_with_rows(self) -> None:
        report = Report()
        effect, ctx = _effect("proc", "on_bash", target="owner", trigger="on_attack_hit", kind="item", value="25")
        ctx.owner_triggers = {"on_attack_hit", "on_bash"}
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("AbilityEffect::Proc { chance: RankValue::fixed(0.25)", result[2])
        self.assertIn('AbilityTrigger::Custom("on_bash")', result[2])

        for bad_id, triggers in (("on_bash", {"on_attack_hit"}), ("on_cast", {"on_cast"}), ("bash", {"bash"}), ("on_attack_hit", {"on_attack_hit"})):
            report = Report()
            effect, ctx = _effect("proc", bad_id, target="owner", trigger="on_attack_hit", kind="item", value="25")
            ctx.owner_triggers = triggers
            self.assertIsNone(effect_to_rust(effect, ctx, report), bad_id)
            self.assertEqual(report.errors[0].field, "effect_id")

        report = Report()
        effect, ctx = _effect("proc", "on_bash", target="trigger_unit", trigger="on_attack_hit", kind="item", value="25")
        ctx.owner_triggers = {"on_bash"}
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "effect_target")

    def test_proc_chance_range(self) -> None:
        for value, ok in (("100", True), ("0.5", True), ("0", False), ("101", False), ("", False)):
            report = Report()
            effect, ctx = _effect("proc", "on_bash", target="owner", trigger="on_attack_hit", kind="item", value=value)
            ctx.owner_triggers = {"on_bash"}
            result = effect_to_rust(effect, ctx, report)
            self.assertEqual(result is not None, ok, value)

    def test_counter_requires_integer_at_least_two(self) -> None:
        for value, ok in (("4", True), ("2", True), ("1", False), ("2.5", False), ("", False)):
            report = Report()
            effect, ctx = _effect("counter", "on_fourth", target="owner", trigger="on_attack_hit", kind="item", value=value)
            ctx.owner_triggers = {"on_fourth"}
            result = effect_to_rust(effect, ctx, report)
            self.assertEqual(result is not None, ok, value)
            if ok:
                self.assertIn(f"every: {int(float(value))}", result[2])

    def test_perform_attack_columns(self) -> None:
        report = Report()
        effect, ctx = _effect("perform_attack", "", target="cast_target")
        self.assertIn("damage_ratio: RankValue::fixed(1.0), max_targets: 0, exclude_trigger_unit: false", effect_to_rust(effect, ctx, report)[2])

        report = Report()
        effect, ctx = _effect("perform_attack", "exclude_trigger_unit", target="trigger_unit", trigger="on_attack", kind="item", value="70", amount="2")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("damage_ratio: RankValue::fixed(0.7), max_targets: 2, exclude_trigger_unit: true", result[2])

        for cells in ({"effect_id": "bogus"}, {"value": "0"}, {"amount": "1.5"}, {"amount": "-1"}):
            report = Report()
            args = {"effect_id": "", **cells}
            effect, ctx = _effect("perform_attack", args.pop("effect_id"), target="cast_target", **args)
            self.assertIsNone(effect_to_rust(effect, ctx, report), cells)

    def test_evasion_accuracy_true_strike_statuses(self) -> None:
        for status, expected in (("evasion", "StatusSpec::Evasion { percent: RankValue::fixed(30.0) }"), ("accuracy", "StatusSpec::Accuracy { percent: RankValue::fixed(30.0) }")):
            report = Report()
            effect, ctx = _effect("apply_status", status, target="caster", amount="30", duration="5")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, status)
            self.assertIn(expected, result[2])
            report = Report()
            effect, ctx = _effect("apply_status", status, target="caster", amount="0", duration="5")
            self.assertIsNone(effect_to_rust(effect, ctx, report))
        report = Report()
        effect, ctx = _effect("apply_status", "true_strike", target="caster", duration="5")
        self.assertIn("StatusSpec::TrueStrike", effect_to_rust(effect, ctx, report)[2])
        self.assertFalse(report.errors)

    def test_attack_passive_stat_ranges(self) -> None:
        for stat, variant in (
            ("crit_chance", "CritChance"), ("crit_multiplier", "CritMultiplier"), ("evasion", "Evasion"),
            ("accuracy", "Accuracy"), ("cleave", "CleaveRatio"), ("splash", "SplashRatio"),
            ("attack_range", "AttackRange"), ("attack_projectile_speed", "AttackProjectileSpeed"),
        ):
            report = Report()
            effect, ctx = _effect("stat_modifier", stat, target="owner", trigger="passive", kind="item", value="0.5")
            self.assertIn(f"PassiveStat::{variant}", effect_to_rust(effect, ctx, report)[2], stat)
        for stat in ("crit_chance", "evasion", "accuracy", "cleave", "splash"):
            report = Report()
            effect, ctx = _effect("stat_modifier", stat, target="owner", trigger="passive", kind="item", value="1.5")
            self.assertIsNone(effect_to_rust(effect, ctx, report), stat)
            self.assertIn("-1..1", report.errors[0].message)

    def test_timed_attack_range_modifier(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "attack_range", target="caster", value="100", duration="8")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::Modifier", result[2])
        self.assertIn("attack_range: RankValue::fixed(100.0)", result[2])
        self.assertIn("StatusKind::Buff", result[2])

    def test_unreachable_custom_trigger_warns(self) -> None:
        def make(trigger: str, effect_type: str, effect_id: str) -> EffectRow:
            cells = {"item_id": "t", "effect_index": "0", "trigger": trigger, "effect_target": "owner", "effect_type": effect_type, "effect_id": effect_id}
            return EffectRow(row(cells), "t", 0, trigger, "owner", effect_type)

        report = Report()
        warn_unreachable_custom_triggers([make("on_bash", "heal", "")], set(), report)
        self.assertEqual(len(report.warnings), 1)
        report = Report()
        warn_unreachable_custom_triggers([make("on_attack_hit", "proc", "on_bash"), make("on_bash", "heal", "")], set(), report)
        self.assertFalse(report.warnings)

    @staticmethod
    def _trigger_row(owner_column: str, trigger: str) -> EffectRow:
        cells = {owner_column: "t", "effect_index": "0", "trigger": trigger, "effect_target": "caster", "effect_type": "heal", "effect_id": ""}
        return EffectRow(row(cells), "t", 0, trigger, "caster", "heal")

    def test_unemitted_ability_trigger_warns(self) -> None:
        for trigger in ("on_channel_tick", "on_expire"):
            report = Report()
            warn_unemitted_triggers([self._trigger_row("ability_id", trigger)], report)
            self.assertEqual(len(report.warnings), 1, trigger)
            self.assertIn("never fire", report.warnings[0].message)

    def test_reactive_triggers_are_emitted_now(self) -> None:
        for owner in ("ability_id", "item_id"):
            for trigger in ("on_damage_taken", "on_kill", "on_death"):
                report = Report()
                warn_unemitted_triggers([self._trigger_row(owner, trigger)], report)
                self.assertFalse(report.warnings, (owner, trigger))

    def test_emitted_triggers_do_not_warn(self) -> None:
        emitted = [t for t in {**ABILITY_TRIGGERS, **ITEM_TRIGGERS} if t not in NOT_EMITTED_TRIGGERS]
        report = Report()
        warn_unemitted_triggers([self._trigger_row("item_id", t) for t in emitted], report)
        self.assertFalse(report.warnings)


class ChainEffectTests(unittest.TestCase):
    GOOD = {"value": "3", "radius": "450", "amount": "75"}

    def test_chain_columns_become_the_gate(self) -> None:
        for kind in ("ability", "item"):
            report = Report()
            effect, ctx = _effect("chain", "on_chain_hit", kind=kind, **self.GOOD)
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, kind)
            self.assertIn(
                'AbilityEffect::Chain { jumps: 3, radius: RankValue::fixed(450.0), falloff: 0.75, '
                'team: TargetTeam::Enemy, then: AbilityTrigger::Custom("on_chain_hit") }',
                result[2],
            )
            self.assertIn("EffectTarget::CastTarget", result[1])

    def test_chain_rejects_bad_columns(self) -> None:
        bad = (
            ({"value": "0"}, "jumps"),
            ({"value": "1.5"}, "jumps"),
            ({"value": ""}, "required"),
            ({"radius": "0"}, "radius"),
            ({"radius": ""}, "required"),
            ({"amount": "0"}, "percent"),
            ({"amount": "101"}, "percent"),
            ({"amount": ""}, "required"),
        )
        for override, word in bad:
            report = Report()
            effect, ctx = _effect("chain", "on_chain_hit", **{**self.GOOD, **override})
            self.assertIsNone(effect_to_rust(effect, ctx, report), override)
            self.assertIn(word, report.errors[0].message, override)

    def test_chain_needs_a_unit_start_target_and_a_custom_trigger(self) -> None:
        for target in ("affected_units", "units_in_radius"):
            report = Report()
            effect, ctx = _effect("chain", "on_chain_hit", target=target, **self.GOOD)
            self.assertIsNone(effect_to_rust(effect, ctx, report), target)
            self.assertEqual(report.errors[0].field, "effect_target")
        for trigger_id in ("on_use", "on_cast", "on_tester"):
            report = Report()
            effect, ctx = _effect("chain", trigger_id, trigger="on_tester", **self.GOOD)
            self.assertIsNone(effect_to_rust(effect, ctx, report), trigger_id)

    def test_chain_feeds_its_trigger(self) -> None:
        def make(trigger: str, effect_type: str, effect_id: str) -> EffectRow:
            cells = {"item_id": "t", "effect_index": "0", "trigger": trigger, "effect_target": "cast_target", "effect_type": effect_type, "effect_id": effect_id}
            return EffectRow(row(cells), "t", 0, trigger, "cast_target", effect_type)

        report = Report()
        warn_unreachable_custom_triggers([make("on_use", "chain", "on_chain_hit"), make("on_chain_hit", "damage", "")], set(), report)
        self.assertFalse(report.warnings)


class ItemProjectileTests(unittest.TestCase):
    ITEM_HEADER = "id,name,item_type,cost,sell_value,tier,rarity,stackable,max_stack,consumable,max_charges,description,cooldown,target_type,target_team,cast_range,projectile_type,projectile_speed\n"
    EFFECT_HEADER = "item_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value\n"

    def load(self, item_row: str, effect_rows: str):
        import tempfile

        import ItemGenerator

        with tempfile.TemporaryDirectory() as tmp:
            items = Path(tmp) / "items.csv"
            effects = Path(tmp) / "effects.csv"
            items.write_text(self.ITEM_HEADER + item_row + "\n", encoding="utf-8")
            effects.write_text(self.EFFECT_HEADER + effect_rows, encoding="utf-8")
            loaded, report = ItemGenerator.load(items, effects, set())
        return ItemGenerator, loaded, report

    def test_projectile_type_emits_the_projectile_mechanic(self) -> None:
        module, items, report = self.load(
            "bolt_wand,Bolt Wand,weapon,100,50,1,common,false,,false,,wand,10,unit,enemy,800,targeted,1200",
            "bolt_wand,0,on_projectile_hit,trigger_unit,damage,,50,magical,,,,,\n",
        )
        self.assertFalse(report.errors)
        self.assertFalse([w for w in report.warnings if w.field == "projectile_type"])
        rendered = module.item_static(items[0])
        self.assertIn(
            "mechanics: &[AbilityMechanic::Projectile { splash_radius: AreaRadius::Ability, speed: Some(1200.0), skillshot: false, first_hit: false, end_radius: None }],",
            rendered,
        )
        self.assertIn("AbilityTrigger::OnProjectileHit", rendered)

    def test_items_without_a_projectile_emit_empty_mechanics(self) -> None:
        module, items, report = self.load(
            "plain_wand,Plain Wand,weapon,100,50,1,common,false,,false,,wand,10,unit,enemy,800,,",
            "plain_wand,0,on_use,cast_target,damage,,50,magical,,,,,\n",
        )
        self.assertFalse(report.errors)
        self.assertIn("mechanics: &[],", module.item_static(items[0]))

    def test_projectile_triggers_need_a_projectile(self) -> None:
        for trigger, target in (("on_projectile_hit", "trigger_unit"), ("on_impact", "affected_units")):
            _, _, report = self.load(
                "dull_wand,Dull Wand,weapon,100,50,1,common,false,,false,,wand,10,unit,enemy,800,,",
                f"dull_wand,0,on_use,cast_target,damage,,50,magical,,,,,\ndull_wand,1,{trigger},{target},damage,,50,magical,,,,,\n",
            )
            self.assertTrue(any("no projectile" in e.message for e in report.errors), trigger)

    def test_projectile_item_counts_as_active(self) -> None:
        _, _, report = self.load(
            "bolt_wand,Bolt Wand,weapon,100,50,1,common,false,,false,,wand,10,unit,enemy,800,targeted,",
            "bolt_wand,0,on_projectile_hit,trigger_unit,damage,,50,magical,,,,,\n",
        )
        self.assertFalse(report.errors)
        self.assertFalse([w for w in report.warnings if w.field == "cooldown"])


def _gate_row(effect_type: str, effect_id: str, trigger: str, value: str, owner_triggers: set[str]):
    effect, ctx = _effect(effect_type, effect_id, target="owner", trigger=trigger, kind="item", value=value)
    ctx.owner_triggers = owner_triggers
    return effect, ctx


class ReactiveTriggerTests(unittest.TestCase):
    @staticmethod
    def _render(trigger: str, kind: str = "item", **cells: str):
        cells.setdefault("value", "10")
        cells.setdefault("damage_type", "magical")
        effect, ctx = _effect("damage", "", target="owner", trigger=trigger, kind=kind, **cells)
        report = Report()
        return effect_to_rust(effect, ctx, report), report

    def test_trigger_args_parsed_and_emitted(self) -> None:
        expected = {
            "on_health_below:25": "AbilityTrigger::OnHealthBelow { percent: 25 }",
            "on_interval:3": "AbilityTrigger::OnInterval { millis: 3000 }",
            "on_interval:0.5": "AbilityTrigger::OnInterval { millis: 500 }",
            "on_enemy_enter:600": "AbilityTrigger::OnEnemyEnter { radius: 600 }",
            "on_enemy_leave:450": "AbilityTrigger::OnEnemyLeave { radius: 450 }",
            "on_status_applied": "AbilityTrigger::OnStatusApplied(StatusFilter::Any)",
            "on_status_applied:debuff": "AbilityTrigger::OnStatusApplied(StatusFilter::Debuff)",
            "on_status_removed:buff": "AbilityTrigger::OnStatusRemoved(StatusFilter::Buff)",
            "on_status_applied:stun": 'AbilityTrigger::OnStatusApplied(StatusFilter::Id("stun"))',
            "on_status_removed:debuff_immunity": 'AbilityTrigger::OnStatusRemoved(StatusFilter::Id("spell_immunity"))',
            "on_status_applied:frost_hammer_armor": 'AbilityTrigger::OnStatusApplied(StatusFilter::Id("frost_hammer_armor"))',
        }
        for trigger, rust in expected.items():
            for kind in ("item", "ability"):
                result, report = self._render(trigger, kind)
                self.assertFalse(report.errors, (trigger, [e.message for e in report.errors]))
                self.assertEqual(result[0], rust, trigger)

        plain = {
            "on_damage_dealt": "OnDamageDealt",
            "on_damage_taken": "OnDamageTaken",
            "on_kill": "OnKill",
            "on_death": "OnDeath",
            "on_spell_cast": "OnSpellCast",
            "on_spell_hit": "OnSpellHit",
        }
        for trigger, variant in plain.items():
            for kind in ("item", "ability"):
                result, report = self._render(trigger, kind)
                self.assertFalse(report.errors, trigger)
                self.assertEqual(result[0], f"AbilityTrigger::{variant}")

    def test_while_conditions_emitted(self) -> None:
        expected = {
            "while:health_below:50": "Condition::HealthBelow { percent: 50 }",
            "while:moving": "Condition::Moving",
            "while:stationary": "Condition::Stationary",
            "while:enemies_near:600:2": "Condition::EnemiesNear { radius: 600, at_least: 2 }",
            "while:allies_near:600:1": "Condition::AlliesNear { radius: 600, at_least: 1 }",
            "while:has_status:stun": 'Condition::HasStatus("stun")',
        }
        for trigger, condition in expected.items():
            effect, ctx = _effect("apply_status", "evasion", target="owner", trigger=trigger, kind="item", value="30")
            report = Report()
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, (trigger, [e.message for e in report.errors]))
            self.assertEqual(result[0], f"AbilityTrigger::While({condition})")
            self.assertIn("duration: RankValue::fixed(0.0)", result[2])

    def test_trigger_arg_ranges(self) -> None:
        bad = [
            "on_health_below",  # missing
            "on_health_below:0",
            "on_health_below:100",
            "on_health_below:30.5",
            "on_health_below:abc",
            "on_interval",
            "on_interval:0.05",
            "on_interval:fast",
            "on_enemy_enter",
            "on_enemy_enter:0",
            "on_enemy_leave:-5",
            "on_status_applied:not_a_status",
            "on_status_applied:buff:debuff",
            "on_kill:3",  # takes no argument
            "on_spell_cast:1",
            "on_move:5",
        ]
        for trigger in bad:
            result, report = self._render(trigger)
            self.assertIsNone(result, trigger)
            self.assertEqual(report.errors[0].field, "trigger", trigger)
        for trigger in ("while", "while:fast", "while:health_below", "while:health_below:100", "while:moving:1", "while:enemies_near:600", "while:enemies_near:0:2", "while:enemies_near:600:0", "while:has_status", "while:has_status:nope"):
            effect, ctx = _effect("apply_status", "evasion", target="owner", trigger=trigger, kind="item", value="30")
            report = Report()
            self.assertIsNone(effect_to_rust(effect, ctx, report), trigger)
            self.assertEqual(report.errors[0].field, "trigger", trigger)

    def test_damage_kill_death_triggers_accepted_for_abilities(self) -> None:
        for trigger in ("on_damage_taken", "on_kill", "on_death", "on_damage_dealt"):
            self.assertIn(trigger, ABILITY_TRIGGERS)
            self.assertIn(trigger, ITEM_TRIGGERS)
            result, report = self._render(trigger, "ability")
            self.assertFalse(report.errors, trigger)
        # `attacker` is the unit that caused it: the attacker for damage taken, the killer on death.
        for trigger in ("on_damage_taken", "on_death"):
            effect, ctx = _effect("damage", "", target="attacker", trigger=trigger, kind="item", value="10", damage_type="magical")
            report = Report()
            self.assertIn("EffectTarget::TriggerUnit", effect_to_rust(effect, ctx, report)[1])
        effect, ctx = _effect("damage", "", target="attacker", trigger="on_kill", kind="item", value="10", damage_type="magical")
        self.assertIsNone(effect_to_rust(effect, ctx, Report()))

    def test_on_move_requires_accumulate(self) -> None:
        effect, ctx = _effect("heal", "", target="owner", trigger="on_move", kind="item", value="10")
        report = Report()
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("accumulate", report.errors[0].message)
        effect, ctx = _gate_row("accumulate", "on_far", "on_move", "1000", {"on_move", "on_far"})
        report = Report()
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertEqual(result[0], "AbilityTrigger::OnMove")

    def test_while_requires_permanent_owner_apply_status(self) -> None:
        def run(effect_type: str = "apply_status", effect_id: str = "evasion", target: str = "owner", **cells: str):
            cells = {"value": "30", **cells}
            effect, ctx = _effect(effect_type, effect_id, target=target, trigger="while:moving", kind="item", **cells)
            report = Report()
            return effect_to_rust(effect, ctx, report), report

        result, report = run()
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::Evasion", result[2])
        result, report = run(effect_type="damage", effect_id="", damage_type="magical", value="10")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "effect_type")
        result, report = run(target="trigger_unit")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "effect_target")
        result, report = run(duration="5")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "duration")
        # Timed stat modifiers are statuses too; they are held while the condition is true.
        result, report = run(effect_type="stat_modifier", effect_id="armor", value="10")
        self.assertFalse(report.errors)
        self.assertIn('id: "tester_armor"', result[2])

    def test_while_has_status_self_reference_rejected(self) -> None:
        effect, ctx = _effect("apply_status", "evasion", target="owner", trigger="while:has_status:evasion", kind="item", value="30")
        report = Report()
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("keep itself on", report.errors[0].message)
        effect, ctx = _effect("apply_status", "evasion", target="owner", trigger="while:has_status:stun", kind="item", value="30")
        self.assertIsNotNone(effect_to_rust(effect, ctx, Report()))

    def test_cooldown_ready_invalid_for_passive_and_non_use_items(self) -> None:
        import tempfile

        import AbilityGenerator
        import ItemGenerator

        effect_header = "{owner}_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value\n"
        ability_header = "id,name,slot,ability_type,target_type,target_team,cast_range,aoe_radius,cast_time,channel_time,cooldown,mana_cost,max_charges,charge_restore_time,projectile_type,projectile_speed,description\n"
        item_header = "id,name,item_type,cost,sell_value,tier,rarity,stackable,max_stack,consumable,max_charges,description,cooldown\n"
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)

            def ability_errors(ability_type: str) -> list[str]:
                (tmp / "a.csv").write_text(
                    ability_header + f"tester,Tester,q,{ability_type},no_target,self,,,0.1,,10,50,,,none,,x\n"
                )
                (tmp / "ae.csv").write_text(
                    effect_header.format(owner="ability") + "tester,0,on_cooldown_ready,owner,heal,,10,,,,,,\n"
                )
                _, report = AbilityGenerator.load(tmp / "a.csv", tmp / "ae.csv")
                return [e.message for e in report.errors]

            self.assertEqual(ability_errors("active"), [])
            self.assertTrue(any("no cooldown" in m for m in ability_errors("passive")))

            def item_errors(use_row: bool) -> list[str]:
                (tmp / "i.csv").write_text(item_header + "tester,Tester,weapon,100,50,1,common,false,,false,,x,5\n")
                rows = "tester,0,on_cooldown_ready,owner,heal,,10,,,,,,\n"
                if use_row:
                    rows += "tester,1,on_use,owner,heal,,10,,,,,,\n"
                (tmp / "ie.csv").write_text(effect_header.format(owner="item") + rows)
                _, report = ItemGenerator.load(tmp / "i.csv", tmp / "ie.csv", set())
                return [e.message for e in report.errors]

            self.assertEqual(item_errors(True), [])
            self.assertTrue(any("no active" in m for m in item_errors(False)))

    def test_perform_attack_rejected_under_reaction_triggers(self) -> None:
        for trigger in ("on_damage_taken", "on_damage_dealt", "on_kill", "on_death", "on_health_below:30", "on_status_applied:debuff", "on_status_removed"):
            effect, ctx = _effect("perform_attack", "", target="owner", trigger=trigger, kind="item")
            report = Report()
            self.assertIsNone(effect_to_rust(effect, ctx, report), trigger)
            self.assertIn("never start attacks", report.errors[0].message)
        for trigger in ("on_attack", "on_attack_hit", "on_spell_cast", "on_interval:5"):
            effect, ctx = _effect("perform_attack", "", target="owner", trigger=trigger, kind="item")
            self.assertIsNotNone(effect_to_rust(effect, ctx, Report()), trigger)

    def test_perform_attack_rejected_under_custom_trigger_fed_by_a_reaction(self) -> None:
        def make(trigger: str, effect_type: str, effect_id: str) -> EffectRow:
            cells = {"item_id": "t", "effect_index": "0", "trigger": trigger, "effect_target": "owner", "effect_type": effect_type, "effect_id": effect_id}
            return EffectRow(row(cells), "t", 0, trigger, "owner", effect_type)

        rows = [make("on_damage_taken", "proc", "on_echo"), make("on_echo", "perform_attack", "")]
        self.assertIn("on_echo", reaction_triggers(rows))
        effect, ctx = _effect("perform_attack", "", target="owner", trigger="on_echo", kind="item")
        ctx.reaction = reaction_triggers(rows)
        self.assertIsNone(effect_to_rust(effect, ctx, Report()))

    def test_not_emitted_triggers_are_channel_tick_and_expire(self) -> None:
        self.assertEqual(set(NOT_EMITTED_TRIGGERS), {"on_channel_tick", "on_expire"})

    @staticmethod
    def _item_row(trigger: str, effect_type: str, effect_id: str) -> EffectRow:
        cells = {"item_id": "t", "effect_index": "0", "trigger": trigger, "effect_target": "owner", "effect_type": effect_type, "effect_id": effect_id}
        return EffectRow(row(cells), "t", 0, trigger, "owner", effect_type)

    def test_chain_fed_by_reaction_is_a_reaction(self) -> None:
        rows = [self._item_row("on_damage_taken", "chain", "on_arc"), self._item_row("on_arc", "damage", "")]
        self.assertIn("on_arc", reaction_triggers(rows))
        self.assertIn("on_arc", chain_fed_triggers(rows))
        # A chain fed by a non-reaction trigger is chain-fed but not a reaction.
        rows = [self._item_row("on_attack_hit", "chain", "on_bounce")]
        self.assertNotIn("on_bounce", reaction_triggers(rows))
        self.assertIn("on_bounce", chain_fed_triggers(rows))

    def test_perform_attack_rejected_under_chain_fed_trigger(self) -> None:
        rows = [
            self._item_row("on_attack_hit", "chain", "on_bounce"),
            self._item_row("on_bounce", "proc", "on_lucky"),
            self._item_row("on_lucky", "perform_attack", ""),
        ]
        fed = chain_fed_triggers(rows)
        self.assertEqual(fed, {"on_bounce", "on_lucky"})
        for trigger in ("on_bounce", "on_lucky"):
            effect, ctx = _effect("perform_attack", "", target="owner", trigger=trigger, kind="item")
            ctx.reaction = reaction_triggers(rows)
            ctx.chain_fed = fed
            report = Report()
            self.assertIsNone(effect_to_rust(effect, ctx, report), trigger)
            self.assertIn("fed by a chain", report.errors[0].message)
        # Not fed by the chain: still fine.
        effect, ctx = _effect("perform_attack", "", target="owner", trigger="on_attack_hit", kind="item")
        ctx.chain_fed = fed
        self.assertIsNotNone(effect_to_rust(effect, ctx, Report()))

    def test_cooldown_ready_valid_for_projectile_items(self) -> None:
        import tempfile

        import ItemGenerator

        item_header = "id,name,item_type,cost,sell_value,tier,rarity,stackable,max_stack,consumable,max_charges,description,cooldown,target_type,target_team,cast_range,projectile_type,projectile_speed\n"
        effect_header = "item_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value\n"
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "i.csv").write_text(item_header + "tester,Tester,weapon,100,50,1,common,false,,false,,x,5,unit,enemy,600,targeted,900\n")
            (tmp / "ie.csv").write_text(
                effect_header
                + "tester,0,on_cooldown_ready,owner,heal,,10,,,,,,\n"
                + "tester,1,on_projectile_hit,trigger_unit,damage,,10,magical,,,,,\n"
            )
            _, report = ItemGenerator.load(tmp / "i.csv", tmp / "ie.csv", set())
            self.assertEqual([e.message for e in report.errors], [])

    def test_pseudo_proc_and_accumulate_columns(self) -> None:
        effect, ctx = _gate_row("pseudo_proc", "on_bash", "on_attack_hit", "25", {"on_bash"})
        report = Report()
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("AbilityEffect::Proc { chance: RankValue::fixed(0.25)", result[2])
        self.assertIn("pseudo: true", result[2])
        effect, ctx = _gate_row("proc", "on_bash", "on_attack_hit", "25", {"on_bash"})
        self.assertIn("pseudo: false", effect_to_rust(effect, ctx, Report())[2])
        effect, ctx = _gate_row("pseudo_proc", "on_bash", "on_attack_hit", "0", {"on_bash"})
        self.assertIsNone(effect_to_rust(effect, ctx, Report()))

        effect, ctx = _gate_row("accumulate", "on_full", "on_damage_taken", "300", {"on_full"})
        result = effect_to_rust(effect, ctx, Report())
        self.assertIn("AbilityEffect::Accumulate { threshold: RankValue::fixed(300.0)", result[2])
        self.assertIn('then: AbilityTrigger::Custom("on_full")', result[2])
        for value in ("", "0", "-5"):
            effect, ctx = _gate_row("accumulate", "on_full", "on_damage_taken", value, {"on_full"})
            self.assertIsNone(effect_to_rust(effect, ctx, Report()), value)
        # Levels are allowed on ability thresholds.
        effect, ctx = _effect("accumulate", "on_full", target="caster", trigger="on_damage_taken", kind="ability", value="100|200|300")
        ctx.levels = 3
        ctx.owner_triggers = {"on_full"}
        report = Report()
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("threshold: RankValue::from_level_1(100.0, 100.0)", result[2])

    def test_accumulate_feeds_custom_trigger(self) -> None:
        def make(trigger: str, effect_type: str, effect_id: str) -> EffectRow:
            cells = {"item_id": "t", "effect_index": "0", "trigger": trigger, "effect_target": "owner", "effect_type": effect_type, "effect_id": effect_id}
            return EffectRow(row(cells), "t", 0, trigger, "owner", effect_type)

        for gate in ("accumulate", "pseudo_proc"):
            report = Report()
            warn_unreachable_custom_triggers([make("on_move", gate, "on_far"), make("on_far", "heal", "")], set(), report)
            self.assertFalse(report.warnings, gate)
        report = Report()
        warn_unreachable_custom_triggers([make("on_far", "heal", "")], set(), report)
        self.assertEqual(len(report.warnings), 1)

    def test_duplicate_held_status_ids_warn(self) -> None:
        def make(index: int, trigger_cell: str, effect_id: str) -> EffectRow:
            cells = {"item_id": "t", "effect_index": str(index), "trigger": trigger_cell, "effect_target": "owner", "effect_type": "apply_status", "effect_id": effect_id}
            name, *args = trigger_cell.split(":")
            return EffectRow(row(cells, line=index + 2), "t", index, name, "owner", "apply_status", tuple(args))

        report = Report()
        warn_duplicate_held_statuses([make(0, "while:moving", "evasion"), make(1, "while:stationary", "evasion")], report)
        self.assertEqual(len(report.warnings), 1)
        report = Report()
        warn_duplicate_held_statuses([make(0, "while:moving", "evasion"), make(1, "while:stationary", "accuracy")], report)
        self.assertFalse(report.warnings)

    def test_custom_trigger_with_arguments_is_rejected(self) -> None:
        # A custom trigger such as `on_bash:5` must not silently lose its argument.
        result, report = self._render("on_bash:5")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "trigger")

    def test_while_stat_modifier_holds_a_permanent_status_with_the_owner_scoped_id(self) -> None:
        # The deviation: `while` accepts timed stat_modifier rows. They must emit a blank
        # (zero) duration, and the status id the duplicate-id warning and the `has_status`
        # self-reference rule reason about must be the id the emitted status really has.
        effect, ctx = _effect("stat_modifier", "armor", target="owner", trigger="while:moving", kind="item", value="5")
        report = Report()
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors, [e.message for e in report.errors])
        self.assertIn("duration: RankValue::fixed(0.0)", result[2])
        self.assertIn(f'id: "{ctx.owner_id}_armor"', result[2])
        self.assertEqual(held_status_id(effect), f"{effect.owner}_armor")

        # A passive-only stat still cannot be held.
        effect, ctx = _effect("stat_modifier", "strength", target="owner", trigger="while:moving", kind="item", value="5")
        report = Report()
        self.assertIsNone(effect_to_rust(effect, ctx, report))


class ExampleCatalogTests(unittest.TestCase):
    """The shipped example CSVs stay valid and the invalid ones stay rejected."""

    ROOT = Path(__file__).resolve().parent.parent
    CASES = (
        ("AbilityGenerator.py", "abilities.csv", "ability_effects.csv"),
        ("ItemGenerator.py", "items.csv", "item_effects.csv"),
    )

    def _check(self, script: str, directory: str, *files: str) -> "subprocess.CompletedProcess[str]":
        base = self.ROOT / "data" / "examples" / directory
        paths = [str(base / f) for f in files]
        return subprocess.run(
            [sys.executable, "-B", str(self.ROOT / "scripts" / script), "--check", *paths],
            capture_output=True,
            text=True,
        )

    def test_valid_examples_pass(self) -> None:
        for script, *files in self.CASES:
            result = self._check(script, "", *files)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_invalid_examples_are_rejected(self) -> None:
        for script, *files in self.CASES:
            result = self._check(script, "invalid", *files)
            self.assertEqual(result.returncode, 1, script)
            self.assertIn("validation error", result.stderr + result.stdout)


ABILITY_COLUMNS_FOR_TESTS = (
    "id,name,slot,ability_type,target_type,target_team,cast_range,aoe_radius,cast_time,"
    "channel_time,cooldown,mana_cost,max_charges,charge_restore_time,projectile_type,"
    "projectile_speed,description,cast_backswing,max_rank,toggle_mana_per_sec,"
    "projectile_end_radius"
).split(",")
EFFECT_COLUMNS_FOR_TESTS = (
    "ability_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,"
    "duration,radius,amount,scaling_type,scaling_value"
).split(",")


def load_test_ability(ability: dict[str, str] | None = None, effects: list[dict[str, str]] | None = None):
    """Run `AbilityGenerator.load` on one temporary ability (default: a no_target Q) plus effect rows.

    Returns `(abilities, report)`. `ability` overrides the default cells; each effect dict
    overrides `{ability_id: tester, effect_index: <n>, trigger: on_cast, effect_target: caster}`.
    """
    import csv
    import tempfile

    import AbilityGenerator

    cells = {
        "id": "tester", "name": "Tester", "slot": "Q", "ability_type": "active",
        "target_type": "no_target", "target_team": "enemy", "cast_time": "0.1",
        "cooldown": "10", "mana_cost": "50", "projectile_type": "none", "description": "x",
    }
    cells.update(ability or {})
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        with (tmp / "a.csv").open("w", newline="", encoding="utf-8") as fh:
            writer = csv.writer(fh)
            writer.writerow(ABILITY_COLUMNS_FOR_TESTS)
            writer.writerow([cells.get(c, "") for c in ABILITY_COLUMNS_FOR_TESTS])
        with (tmp / "ae.csv").open("w", newline="", encoding="utf-8") as fh:
            writer = csv.writer(fh)
            writer.writerow(EFFECT_COLUMNS_FOR_TESTS)
            for index, effect in enumerate(effects or []):
                base = {"ability_id": "tester", "effect_index": str(index), "trigger": "on_cast", "effect_target": "caster"}
                base.update(effect)
                writer.writerow([base.get(c, "") for c in EFFECT_COLUMNS_FOR_TESTS])
        return AbilityGenerator.load(tmp / "a.csv", tmp / "ae.csv")


class CastBackswingTests(unittest.TestCase):
    def test_backswing_up_to_one_and_a_half_is_accepted_silently(self) -> None:
        for value in ("0.3", "1.2", "1.3", "1.5"):
            _, report = load_test_ability({"cast_backswing": value})
            self.assertEqual(report.errors, [])
            self.assertEqual([w for w in report.warnings if w.field == "cast_backswing"], [], value)

    def test_backswing_above_the_cap_warns(self) -> None:
        _, report = load_test_ability({"cast_backswing": "1.6"})
        self.assertEqual(report.errors, [])
        self.assertTrue(any(w.field == "cast_backswing" and "1.5" in w.message for w in report.warnings))


class TimedStatModifierTests(unittest.TestCase):
    @staticmethod
    def _stat(effect_id: str, value: str, **cells: str):
        effect = {"effect_type": "stat_modifier", "effect_id": effect_id, "value": value, "duration": "5"}
        effect.update(cells)
        return load_test_ability(effects=[effect])

    def test_attack_damage_pct_is_a_percentage_of_base_damage(self) -> None:
        abilities, report = self._stat("attack_damage_pct", "16")
        self.assertEqual(report.errors, [])
        self.assertIn("attack_damage_pct: RankValue::fixed(16.0)", str(abilities[0].effects))
        self.assertIn('id: "tester_attack_damage_pct"', str(abilities[0].effects))
        # The flat attack_damage field stays zero.
        self.assertIn("attack_damage: RankValue::ZERO", str(abilities[0].effects))

    def test_attack_damage_pct_rejects_negative_and_huge_values(self) -> None:
        for bad in ("-10", "1001", "2000"):
            _, report = self._stat("attack_damage_pct", bad)
            self.assertTrue(
                any("must be within 0..1000" in e.message for e in report.errors), bad
            )
        _, report = self._stat("attack_damage_pct", "1000")
        self.assertEqual(report.errors, [])


    def test_timed_cleave_is_a_fraction_within_zero_and_one(self) -> None:
        abilities, report = self._stat("cleave", "0.16|0.24|0.32|0.4|0.48|0.56|0.64")
        self.assertEqual(report.errors, [])
        self.assertIn("cleave: RankValue::from_level_1(0.16, 0.08)", str(abilities[0].effects))
        self.assertIn('id: "tester_cleave"', str(abilities[0].effects))
        for bad in ("1.5", "-0.2"):
            _, report = self._stat("cleave", bad)
            self.assertTrue(
                any("must be within 0..1" in e.message for e in report.errors), bad
            )

    def test_cleave_shape_stats_are_positive_distances(self) -> None:
        for stat, value in (("cleave_radius", "150"), ("cleave_end_radius", "360"), ("cleave_distance", "650")):
            abilities, report = self._stat(stat, value)
            self.assertEqual(report.errors, [], stat)
            self.assertIn(f"{stat}: RankValue::fixed({value}.0)", str(abilities[0].effects))
            self.assertIn(f'id: "tester_{stat}"', str(abilities[0].effects))
            for bad in ("0", "-5"):
                _, report = self._stat(stat, bad)
                self.assertTrue(any("must be above 0" in e.message for e in report.errors), (stat, bad))


class DashContactRadiusTests(unittest.TestCase):
    @staticmethod
    def _dash(effect_id: str = "dash", **cells: str):
        effect = {"effect_type": "movement", "effect_id": effect_id, "value": "800", "duration": "1"}
        effect.update(cells)
        return load_test_ability({"target_type": "point", "cast_range": "800"}, [effect])

    def test_dash_radius_is_the_contact_radius(self) -> None:
        abilities, report = self._dash(radius="145")
        self.assertEqual(report.errors, [])
        self.assertIn("contact_radius: Some(RankValue::fixed(145.0))", str(abilities[0].effects))
        abilities, report = self._dash()
        self.assertIn("contact_radius: None", str(abilities[0].effects))

    def test_dash_radius_must_be_above_zero(self) -> None:
        _, report = self._dash(radius="0")
        self.assertTrue(any("contact radius must be above 0" in e.message for e in report.errors))

    def test_radius_on_other_movements_is_ignored_with_a_warning(self) -> None:
        effect = {"effect_type": "movement", "effect_id": "knockback", "value": "300", "effect_target": "cast_target", "radius": "145"}
        abilities, report = load_test_ability({"target_type": "unit", "cast_range": "800"}, [effect])
        self.assertEqual(report.errors, [])
        self.assertTrue(any(w.field == "radius" for w in report.warnings), report.warnings)
        self.assertIn("contact_radius: None", str(abilities[0].effects))


class HeroGeneratorTests(unittest.TestCase):
    HEADER = (
        "Hero_name,is_melee,Ability_Q_Name,Ability_W_Name,Ability_E_Name,Ability_R_Name,"
        "base_STR,base_AGI,base_INT,STR_per_level,AGI_per_level,INT_per_level,primary,"
        "attack_range{extra_header}"
    )

    def _load(self, is_melee: str, attack_range: str, extra_header: str = "", extra: str = ""):
        import tempfile

        import HeroGenerator

        text = (
            self.HEADER.format(extra_header=extra_header)
            + "\nTest Hero,"
            + is_melee
            + ",A,B,C,D,20,15,10,2,1.5,1,STR,"
            + attack_range
            + extra
            + "\n"
        )
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "heroes.csv"
            path.write_text(text, encoding="utf-8")
            return HeroGenerator.load_csv(path)

    def test_long_reach_melee_hero_is_accepted(self) -> None:
        self.assertEqual(self._load("true", "200")[0].attack_range, 200.0)
        self.assertEqual(self._load("true", "250")[0].attack_range, 250.0)
        self.assertEqual(self._load("true", "")[0].attack_range, 128.0)
        self.assertEqual(self._load("false", "550")[0].attack_range, 550.0)

    def test_turn_rate_is_optional_and_positive(self) -> None:
        import HeroGenerator

        hero = self._load("true", "150", ",turn_rate", ",0.8")[0]
        self.assertEqual(hero.turn_rate, 0.8)
        self.assertIn("                    0.8,\n", HeroGenerator.hero_def_arm(hero))
        blank = self._load("true", "150", ",turn_rate", ",")[0]
        self.assertIsNone(blank.turn_rate)
        self.assertIn("crate::facing::HERO_TURN_RATE", HeroGenerator.hero_def_arm(blank))
        for bad in ("0", "-0.5"):
            with self.assertRaises(SystemExit) as ctx:
                self._load("true", "150", ",turn_rate", "," + bad)
            self.assertIn("turn_rate must be above 0", str(ctx.exception))

    def test_is_melee_must_match_attack_range(self) -> None:
        with self.assertRaises(SystemExit) as ctx:
            self._load("true", "400")
        self.assertIn("disagrees with attack_range", str(ctx.exception))
        with self.assertRaises(SystemExit):
            self._load("false", "200")


if __name__ == "__main__":
    unittest.main()
