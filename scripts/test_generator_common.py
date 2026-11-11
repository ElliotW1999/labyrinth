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


class HealOverTimePercentTests(unittest.TestCase):
    @staticmethod
    def _hot(levels: int = 1, **cells: str):
        effect, ctx = _effect("apply_status", "heal_over_time", target="caster", duration="4", **cells)
        ctx.levels = levels
        report = Report()
        return effect_to_rust(effect, ctx, report), report

    def test_flat_and_percent_parts_fill_their_fields(self) -> None:
        result, report = self._hot(value="40", amount="2.5")
        self.assertEqual(report.errors, [])
        self.assertIn("hps: RankValue::fixed(40.0), max_health_pct: RankValue::fixed(2.5)", result[2])
        # Level lists work on both parts.
        result, report = self._hot(levels=2, value="40|45", amount="1.5|2.5")
        self.assertEqual(report.errors, [])
        self.assertIn("RankValue::from_level_1(40.0, 5.0)", result[2])
        self.assertIn("RankValue::from_level_1(1.5, 1.0)", result[2])

    def test_percent_alone_is_enough(self) -> None:
        result, report = self._hot(amount="3")
        self.assertEqual(report.errors, [])
        self.assertIn("hps: RankValue::ZERO, max_health_pct: RankValue::fixed(3.0)", result[2])

    def test_percent_is_within_zero_and_one_hundred(self) -> None:
        for bad in ("-1", "101"):
            result, report = self._hot(value="10", amount=bad)
            self.assertIsNone(result, bad)
            self.assertTrue(any("0..100" in e.message for e in report.errors), bad)

    def test_one_of_value_or_amount_must_be_above_zero(self) -> None:
        for cells in ({}, {"value": "0"}, {"amount": "0"}, {"value": "0", "amount": "0"}):
            result, report = self._hot(**cells)
            self.assertIsNone(result, cells)
            self.assertTrue(report.errors, cells)


class HealStatusTests(unittest.TestCase):
    def test_heal_statuses_heal_over_time(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "heal_over_time", target="caster", value="25", duration="6")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::HealOverTime { hps: RankValue::fixed(25.0), max_health_pct: RankValue::ZERO }", result[2])

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


class AreaPolarityTests(unittest.TestCase):
    def _area(self, effect_type, effect_id, team, **cells):
        report = Report()
        effect, ctx = _effect(effect_type, effect_id, target="units_in_radius", **cells)
        ctx.team, ctx.has_aoe = team, True
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors, report.errors)
        return result[1]

    def test_ally_ability_damage_area_hits_enemies(self) -> None:
        self.assertIn("team: TargetTeam::Enemy", self._area("damage", "", "Ally", value="90", damage_type="pure"))

    def test_ally_ability_heal_area_hits_allies(self) -> None:
        self.assertIn("team: TargetTeam::Ally", self._area("heal", "", "Ally", value="90"))

    def test_enemy_ability_heal_area_is_unchanged(self) -> None:
        self.assertIn("team: TargetTeam::Enemy", self._area("heal", "", "Enemy", value="90"))

    def test_self_ability_stun_area_hits_enemies(self) -> None:
        self.assertIn("team: TargetTeam::Enemy", self._area("apply_status", "stun", "Ally", duration="1"))


class MagicResistanceStatusTests(unittest.TestCase):
    def _status(self, effect_type="apply_status", **cells):
        report = Report()
        effect, ctx = _effect(effect_type, "magic_resistance", target="caster", **cells)
        ctx.levels = 4
        return effect_to_rust(effect, ctx, report), report

    def test_percent_from_amount_emits_owner_id(self) -> None:
        result, report = self._status(amount="5|10|15|20", duration="10")
        self.assertIsNotNone(result, report.errors)
        self.assertIn('StatusSpec::MagicResistance { id: "tester_magic_resistance"', result[2])
        self.assertIn("percent: RankValue", result[2])

    def test_value_is_accepted_and_range_is_checked(self) -> None:
        result, _ = self._status(value="25", duration="10")
        self.assertIsNotNone(result)
        for bad in ("0", "101", "-5"):
            result, report = self._status(amount=bad, duration="10")
            self.assertIsNone(result, bad)
            self.assertEqual(report.errors[0].field, "amount")
        result, report = self._status(duration="10")
        self.assertIsNone(result)

    def test_remove_status_is_rejected_with_owner_id_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "magic_resistance", target="caster")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_magic_resistance", report.errors[0].message)


class DamageShareStatusTests(unittest.TestCase):
    def _status(self, target="cast_target", **cells):
        report = Report()
        effect, ctx = _effect("apply_status", "damage_share", target=target, **cells)
        ctx.levels = 4
        return effect_to_rust(effect, ctx, report), report

    def test_percent_from_amount_or_value(self) -> None:
        result, report = self._status(amount="100", duration="10")
        self.assertIsNotNone(result, report.errors)
        self.assertIn("StatusSpec::DamageShare { percent: RankValue::fixed(100.0) }", result[2])
        self.assertEqual(result[1], "EffectTarget::CastTarget")
        result, _ = self._status(value="50", duration="10")
        self.assertIsNotNone(result)

    def test_invalid_percent_and_self_targets_are_rejected(self) -> None:
        for name, cells, field in (
            ("zero", {"amount": "0"}, "amount"),
            ("above 100", {"amount": "150"}, "amount"),
            ("missing", {}, "amount"),
            ("caster target", {"amount": "100", "target": "caster"}, "effect_target"),
            ("owner target", {"amount": "100", "target": "owner"}, "effect_target"),
        ):
            result, report = self._status(duration="10", **cells)
            self.assertIsNone(result, name)
            self.assertEqual(report.errors[0].field, field, name)


class OrbitTests(unittest.TestCase):
    GOOD = {"value": "3", "duration": "3|4|5", "radius": "180", "amount": "1.41"}

    def _orbit(self, effect_id="on_touch:80", target="caster", kind="ability", **cells):
        report = Report()
        effect, ctx = _effect("orbit", effect_id, target=target, kind=kind, **{**self.GOOD, **cells})
        ctx.levels = 3
        ctx.has_aoe = kind == "ability"
        return effect_to_rust(effect, ctx, report), report

    def test_orbit_emits_the_effect(self) -> None:
        result, report = self._orbit()
        self.assertIsNotNone(result, report.errors)
        code = result[2]
        self.assertIn("AbilityEffect::Orbit { count: 3, radius: AreaRadius::Fixed(RankValue::fixed(180.0))", code)
        self.assertIn("hit_radius: 80.0", code)
        self.assertIn("period: RankValue::fixed(1.41)", code)
        self.assertIn("duration: RankValue::from_level_1(3.0, 1.0)", code)
        self.assertIn("team: TargetTeam::Enemy", code)
        self.assertIn('then: AbilityTrigger::Custom("on_touch")', code)
        self.assertEqual(result[1], "EffectTarget::Caster")

    def test_blank_radius_falls_back_to_the_ability_radius(self) -> None:
        result, report = self._orbit(radius="")
        self.assertIn("radius: AreaRadius::Ability", result[2])
        result, report = self._orbit(radius="", kind="item")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "radius")

    def test_invalid_orbits_are_rejected(self) -> None:
        cases = (
            ("zero orbiters", {"value": "0"}, "value"),
            ("fractional orbiters", {"value": "2.5"}, "value"),
            ("missing orbiters", {"value": ""}, "value"),
            ("zero period", {"amount": "0"}, "amount"),
            ("missing period", {"amount": ""}, "amount"),
            ("missing hit radius", {"effect_id": "on_touch"}, "effect_id"),
            ("zero hit radius", {"effect_id": "on_touch:0"}, "effect_id"),
            ("junk hit radius", {"effect_id": "on_touch:big"}, "effect_id"),
            ("built-in trigger", {"effect_id": "on_cast:80"}, "effect_id"),
            ("missing duration", {"duration": ""}, "duration"),
            ("zero radius", {"radius": "0"}, "radius"),
            ("cast_target", {"target": "cast_target"}, "effect_target"),
        )
        for name, cells, field in cases:
            result, report = self._orbit(**cells)
            self.assertIsNone(result, name)
            self.assertEqual(report.errors[0].field, field, name)

    def test_orbit_feeds_its_trigger_for_reachability_and_fed_sets(self) -> None:
        effect, _ = _effect("orbit", "on_touch:80", **self.GOOD)
        other, _ = _effect("damage", "", trigger="on_touch", target="trigger_unit")
        self.assertEqual(chain_fed_triggers([effect]), {"on_touch"})
        report = Report()
        warn_unreachable_custom_triggers([effect, other], set(), report)
        self.assertFalse(report.warnings)

    def test_perform_attack_under_an_orbit_fed_trigger_is_rejected(self) -> None:
        report = Report()
        orbit, _ = _effect("orbit", "on_touch:80", **self.GOOD)
        attack, ctx = _effect("perform_attack", "", trigger="on_touch", target="caster")
        ctx.chain_fed = chain_fed_triggers([orbit, attack])
        self.assertIsNone(effect_to_rust(attack, ctx, report))


class ZoneTests(unittest.TestCase):
    GOOD = {"value": "1", "duration": "10", "amount": "1.2"}

    def _zone(self, effect_id="on_tick", target="caster", kind="ability", **cells):
        report = Report()
        effect, ctx = _effect("zone", effect_id, target=target, kind=kind, **{**self.GOOD, **cells})
        ctx.has_aoe = kind == "ability"
        return effect_to_rust(effect, ctx, report), report

    def test_zone_emits_the_effect_with_the_ability_radius(self) -> None:
        result, report = self._zone()
        self.assertFalse(report.errors)
        code = result[2]
        self.assertIn("AbilityEffect::Zone { radius: AreaRadius::Ability, team: TargetTeam::Enemy", code)
        self.assertIn("delay: RankValue::fixed(1.2)", code)
        self.assertIn("duration: RankValue::fixed(10.0)", code)
        self.assertIn("interval: RankValue::fixed(1.0)", code)
        self.assertIn('then: AbilityTrigger::Custom("on_tick")', code)
        self.assertEqual(result[1], "EffectTarget::Caster")

    def test_blank_delay_is_zero_and_levels_are_allowed(self) -> None:
        report = Report()
        effect, ctx = _effect("zone", "on_tick", target="caster", **{**self.GOOD, "amount": "", "value": "1|1|1|2", "duration": "10"})
        ctx.levels, ctx.has_aoe = 4, True
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors, report.errors)
        self.assertIn("delay: RankValue::fixed(0.0)", result[2])

    def test_explicit_radius_wins_and_items_must_set_one(self) -> None:
        result, report = self._zone(radius="350")
        self.assertIn("radius: AreaRadius::Fixed(RankValue::fixed(350.0))", result[2])
        result, report = self._zone(kind="item")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "radius")
        result, report = self._zone(kind="item", radius="350")
        self.assertFalse(report.errors)
        self.assertIn("AreaRadius::Fixed", result[2])

    def test_invalid_zones_are_rejected(self) -> None:
        cases = (
            ("interval of zero", {"value": "0"}, "value"),
            ("interval above the duration", {"value": "11"}, "value"),
            ("negative delay", {"amount": "-1"}, "amount"),
            ("missing duration", {"duration": ""}, "duration"),
            ("missing interval", {"value": ""}, "value"),
            ("zero radius", {"radius": "0"}, "radius"),
            ("built-in trigger", {"effect_id": "on_cast"}, "effect_id"),
        )
        for name, cells, field in cases:
            result, report = self._zone(**cells)
            self.assertIsNone(result, name)
            self.assertEqual(report.errors[0].field, field, name)

    def test_zone_must_be_placed_by_the_caster(self) -> None:
        result, report = self._zone(target="units_in_radius", radius="300")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "effect_target")

    def test_zone_feeds_its_trigger_for_reachability_and_fed_sets(self) -> None:
        effect, _ = _effect("zone", "on_tick", **self.GOOD)
        other, _ = _effect("damage", "", trigger="on_tick", target="affected_units")
        self.assertEqual(chain_fed_triggers([effect]), {"on_tick"})
        report = Report()
        warn_unreachable_custom_triggers([effect, other], set(), report)
        self.assertFalse(report.warnings)

    def test_perform_attack_under_a_zone_fed_trigger_is_rejected(self) -> None:
        report = Report()
        zone, _ = _effect("zone", "on_tick", **self.GOOD)
        attack, ctx = _effect("perform_attack", "", trigger="on_tick", target="caster")
        ctx.chain_fed = chain_fed_triggers([zone, attack])
        self.assertIsNone(effect_to_rust(attack, ctx, report))
        self.assertIn("zone", report.errors[0].message)


class AttachedZoneTests(unittest.TestCase):
    GOOD = {"value": "0.1", "duration": "1", "amount": "1.5"}

    def _zone(self, effect_type="attached_zone", effect_id="on_pulse", target="caster", kind="ability", **cells):
        report = Report()
        effect, ctx = _effect(effect_type, effect_id, target=target, kind=kind, **{**self.GOOD, **cells})
        ctx.has_aoe = kind == "ability"
        return effect_to_rust(effect, ctx, report), report

    def test_attached_zone_emits_a_following_zone(self) -> None:
        result, report = self._zone()
        self.assertFalse(report.errors)
        code = result[2]
        self.assertIn("AbilityEffect::Zone { radius: AreaRadius::Ability, team: TargetTeam::Enemy", code)
        self.assertIn("delay: RankValue::fixed(1.5)", code)
        self.assertIn("duration: RankValue::fixed(1.0)", code)
        self.assertIn("interval: RankValue::fixed(0.1)", code)
        self.assertIn('then: AbilityTrigger::Custom("on_pulse")', code)
        self.assertTrue(code.endswith("follow_caster: true }"), code)
        self.assertEqual(result[1], "EffectTarget::Caster")

    def test_plain_zone_is_not_following(self) -> None:
        result, report = self._zone(effect_type="zone")
        self.assertFalse(report.errors)
        self.assertTrue(result[2].endswith("follow_caster: false }"), result[2])

    def test_attached_zone_has_the_same_errors_as_a_zone(self) -> None:
        cases = (
            {"value": "0"},
            {"value": "2"},
            {"amount": "-1"},
            {"duration": ""},
            {"radius": "0"},
            {"effect_id": "on_cast"},
            {"target": "units_in_radius", "radius": "300"},
        )
        for cells in cases:
            plain, plain_report = self._zone(effect_type="zone", **cells)
            attached, attached_report = self._zone(**cells)
            self.assertIsNone(plain, cells)
            self.assertIsNone(attached, cells)
            self.assertEqual(plain_report.errors[0].field, attached_report.errors[0].field, cells)

    def test_attached_zone_cannot_target_affected_units(self) -> None:
        result, report = self._zone(target="affected_units")
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "effect_target")

    def test_attached_zone_feeds_its_trigger(self) -> None:
        effect, _ = _effect("attached_zone", "on_pulse", **self.GOOD)
        other, _ = _effect("damage", "", trigger="on_pulse", target="affected_units")
        self.assertEqual(chain_fed_triggers([effect]), {"on_pulse"})
        report = Report()
        warn_unreachable_custom_triggers([effect, other], set(), report)
        self.assertFalse(report.warnings)

    def test_attached_zone_does_not_warn_about_its_radius_column(self) -> None:
        report = Report()
        effect, ctx = _effect("attached_zone", "on_pulse", target="caster", radius="300", **self.GOOD)
        ctx.has_aoe = True
        self.assertIsNotNone(effect_to_rust(effect, ctx, report))
        self.assertFalse(report.warnings)


class MiniStunTests(unittest.TestCase):
    def _stun(self, **cells: str):
        report = Report()
        effect, ctx = _effect("apply_status", "mini_stun", **cells)
        return effect_to_rust(effect, ctx, report), report

    def test_mini_stun_binds_a_per_owner_stun_id(self) -> None:
        result, report = self._stun(duration="0.1")
        self.assertFalse(report.errors)
        self.assertIn('StatusSpec::OwnedStun { id: "tester_stun" }', result[2])

    def test_mini_stun_needs_a_duration(self) -> None:
        result, report = self._stun()
        self.assertIsNone(result)
        self.assertEqual(report.errors[0].field, "duration")

    def test_mini_stun_below_one_tick_warns(self) -> None:
        result, report = self._stun(duration="0.01")
        self.assertIsNotNone(result)
        self.assertTrue(any(w.field == "duration" for w in report.warnings))

    def test_plain_stun_keeps_the_shared_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "stun", duration="1")
        result = effect_to_rust(effect, ctx, report)
        self.assertIn("StatusSpec::Stun", result[2])
        self.assertNotIn("OwnedStun", result[2])

    def test_ids_resolve_for_held_conditions_and_triggers(self) -> None:
        effect, _ = _effect("apply_status", "mini_stun", duration="0.1")
        self.assertEqual(held_status_id(effect), "tester_stun")
        self.assertEqual(runtime_status_id("tester_stun"), "tester_stun")
        self.assertIsNone(runtime_status_id("mini_stun"))

    def test_remove_status_mini_stun_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "mini_stun")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_stun", report.errors[0].message)


class SlowStatusTests(unittest.TestCase):
    def test_generated_slow_carries_a_per_owner_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "slow", amount="30", duration="2")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn('StatusSpec::Slow { id: "tester_slow"', result[2])

    def test_runtime_and_held_ids_follow_the_owner(self) -> None:
        self.assertEqual(runtime_status_id("tester_slow"), "tester_slow")
        effect, _ = _effect("apply_status", "slow", amount="30", duration="2")
        self.assertEqual(held_status_id(effect), "tester_slow")

    def test_remove_status_slow_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "slow")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_slow", report.errors[0].message)


class IndependentModifierTests(unittest.TestCase):
    def test_suffix_marks_flat_and_resource_modifiers_independent(self) -> None:
        for stat, spec in (("attack_damage", "StatusSpec::Modifier"), ("agility", "StatusSpec::ResourceModifier")):
            report = Report()
            effect, ctx = _effect("stat_modifier", f"{stat}:independent", value="3", duration="10")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, stat)
            self.assertIn(spec, result[2])
            self.assertIn(f'id: "tester_{stat}"', result[2])
            self.assertIn("independent: true", result[2])
            self.assertEqual(held_status_id(effect), f"tester_{stat}")

    def test_plain_modifiers_are_not_independent(self) -> None:
        for stat in ("attack_damage", "agility"):
            report = Report()
            effect, ctx = _effect("stat_modifier", stat, value="3", duration="10")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors)
            self.assertIn("independent: false", result[2])
            self.assertNotIn("independent: true", result[2])

    def test_suffix_is_rejected_on_passive_and_while_rows(self) -> None:
        for trigger, kind in (("passive", "item"), ("while:moving", "item"), ("while_toggled", "ability")):
            report = Report()
            effect, ctx = _effect("stat_modifier", "armor:independent", target="caster", trigger=trigger, kind=kind, value="5")
            self.assertIsNone(effect_to_rust(effect, ctx, report), trigger)
            self.assertTrue(any(":independent only applies" in err.message for err in report.errors), trigger)

    def test_unknown_suffix_is_rejected(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "armor:forever", value="3", duration="10")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertTrue(any("unknown stat_modifier suffix" in err.message for err in report.errors))


class AttributeStatModifierTests(unittest.TestCase):
    def test_timed_attributes_map_to_resource_modifiers(self) -> None:
        for stat, variant in (
            ("strength", "Strength"),
            ("agility", "Agility"),
            ("intelligence", "Intelligence"),
        ):
            report = Report()
            effect, ctx = _effect("stat_modifier", stat, value="3", duration="10")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, stat)
            self.assertIn(f"stat: ResourceStat::{variant}", result[2])
            self.assertIn("kind: StatusKind::Buff", result[2])
            self.assertIn(f'id: "tester_{stat}"', result[2])
            self.assertEqual(held_status_id(effect), f"tester_{stat}")

    def test_negative_attribute_is_a_debuff_steal(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "agility", value="-3", duration="10")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("kind: StatusKind::Debuff", result[2])

    def test_timed_attribute_needs_a_duration(self) -> None:
        report = Report()
        effect, ctx = _effect("stat_modifier", "agility", value="3")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertTrue(report.errors)


class MoveSpeedBonusTests(unittest.TestCase):
    def test_bonus_uses_amount_or_value_and_the_owner_id(self) -> None:
        for column in ("amount", "value"):
            report = Report()
            effect, ctx = _effect("apply_status", "move_speed_bonus", duration="4", **{column: "20"})
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, column)
            self.assertIn(
                'StatusSpec::MoveSpeedBonus { id: "tester_move_speed_bonus", percent:', result[2]
            )
        self.assertEqual(held_status_id(effect), "tester_move_speed_bonus")
        self.assertEqual(runtime_status_id("tester_move_speed_bonus"), "tester_move_speed_bonus")

    def test_percent_must_be_above_zero_and_at_most_400(self) -> None:
        for bad in ("0", "500", "-5"):
            report = Report()
            effect, ctx = _effect("apply_status", "move_speed_bonus", duration="4", amount=bad)
            self.assertIsNone(effect_to_rust(effect, ctx, report))
            self.assertTrue(any("move_speed_bonus percentage" in e.message for e in report.errors), bad)
        report = Report()
        effect, ctx = _effect("apply_status", "move_speed_bonus", duration="4")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertTrue(any("required" in e.message for e in report.errors))

    def test_remove_status_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "move_speed_bonus")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_move_speed_bonus", report.errors[0].message)


class TauntTargetTests(unittest.TestCase):
    @staticmethod
    def _taunt(trigger: str = "on_cast", target_type: str = "unit", **cells: str):
        cells.setdefault("duration", "4")
        effect, ctx = _effect("apply_status", "taunt_target", target="units_in_radius", trigger=trigger, **cells)
        ctx.target_type = target_type
        ctx.has_aoe = True
        report = Report()
        return effect_to_rust(effect, ctx, report), report

    def test_value_is_the_attack_speed_bonus_and_the_status_id_is_taunt(self) -> None:
        result, report = self._taunt(value="50")
        self.assertEqual(report.errors, [])
        self.assertIn("StatusSpec::TauntCastTarget { attack_speed: RankValue::fixed(50.0) }", result[2])
        result, report = self._taunt()
        self.assertEqual(report.errors, [])
        self.assertIn("attack_speed: RankValue::ZERO", result[2])
        effect, _ = _effect("apply_status", "taunt_target", target="units_in_radius")
        self.assertEqual(held_status_id(effect), "taunt")
        self.assertEqual(runtime_status_id("taunt_target"), "taunt")

    def test_projectile_hit_also_carries_the_cast_target(self) -> None:
        result, report = self._taunt(trigger="on_projectile_hit")
        self.assertEqual(report.errors, [])
        self.assertIsNotNone(result)

    def test_no_target_ability_is_rejected(self) -> None:
        for target_type in ("no_target", "point"):
            result, report = self._taunt(target_type=target_type)
            self.assertIsNone(result, target_type)
            self.assertTrue(any("target_type unit" in e.message for e in report.errors), target_type)

    def test_rows_without_a_cast_target_are_rejected(self) -> None:
        for trigger in ("while", "passive", "on_attack_hit"):
            result, report = self._taunt(trigger=trigger, duration="")
            self.assertIsNone(result, trigger)
            self.assertTrue(report.errors, trigger)

    def test_negative_attack_speed_is_rejected(self) -> None:
        result, report = self._taunt(value="-10")
        self.assertIsNone(result)
        self.assertTrue(any("cannot be negative" in e.message for e in report.errors))


class FrozenStatusTests(unittest.TestCase):
    def test_frozen_is_a_plain_buff_status_keyword(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "frozen", target="cast_target", duration="4")
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.errors, [])
        self.assertIn("status: StatusSpec::Frozen,", result[2])
        self.assertEqual(held_status_id(effect), "frozen")
        self.assertEqual(runtime_status_id("frozen"), "frozen")

    def test_frozen_takes_no_value_and_can_be_removed_by_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "frozen", target="cast_target", duration="4", value="5")
        self.assertIsNotNone(effect_to_rust(effect, ctx, report))
        self.assertTrue(report.warnings)
        report = Report()
        effect, ctx = _effect("remove_status", "frozen")
        self.assertIn('AbilityEffect::RemoveStatus { id: "frozen" }', effect_to_rust(effect, ctx, report)[2])


class AttackPointStatusTests(unittest.TestCase):
    def test_attack_point_uses_value_and_the_owner_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "attack_point", target="caster", duration="7", value="0.1")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn(
            'StatusSpec::AttackPoint { id: "tester_attack_point", seconds: RankValue::fixed(0.1) }',
            result[2],
        )
        self.assertEqual(held_status_id(effect), "tester_attack_point")
        self.assertEqual(runtime_status_id("tester_attack_point"), "tester_attack_point")

    def test_seconds_must_be_within_zero_and_the_swing_cap(self) -> None:
        for bad in ("-0.1", "1.5"):
            report = Report()
            effect, ctx = _effect("apply_status", "attack_point", target="caster", duration="7", value=bad)
            self.assertIsNone(effect_to_rust(effect, ctx, report))
            self.assertTrue(any("attack_point seconds" in e.message for e in report.errors), bad)
        report = Report()
        effect, ctx = _effect("apply_status", "attack_point", target="caster", duration="7", value="1")
        self.assertIsNotNone(effect_to_rust(effect, ctx, report))
        report = Report()
        effect, ctx = _effect("apply_status", "attack_point", target="caster", duration="7")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertTrue(any("required" in e.message for e in report.errors))

    def test_remove_status_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "attack_point")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_attack_point", report.errors[0].message)


class UntargetableStatusTests(unittest.TestCase):
    def test_untargetable_keyword_is_a_self_buff_status(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "untargetable", duration="4")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("status: StatusSpec::Untargetable,", result[2])
        self.assertEqual(runtime_status_id("untargetable"), "untargetable")
        self.assertEqual(held_status_id(effect), "untargetable")


class ImmunityStatusTests(unittest.TestCase):
    def test_composite_immunity_keywords_map_to_their_specs(self) -> None:
        for keyword, spec in (
            ("debuff_immunity", "StatusSpec::DebuffImmunity"),
            ("spell_immunity", "StatusSpec::SpellImmunity"),
            ("attack_immunity", "StatusSpec::AttackImmunity"),
            ("untargetable", "StatusSpec::Untargetable"),
            ("banish", "StatusSpec::Banish"),
            ("break", "StatusSpec::Break"),
            ("taunt", "StatusSpec::Taunt"),
            ("hypnosis", "StatusSpec::Hypnosis"),
            ("reveal", "StatusSpec::Reveal"),
            ("spell_block", "StatusSpec::SpellBlock"),
            ("spell_reflect", "StatusSpec::SpellReflect"),
        ):
            report = Report()
            effect, ctx = _effect("apply_status", keyword, duration="2")
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, keyword)
            self.assertIn(f"status: {spec},", result[2])

    def test_runtime_ids_distinguish_debuff_and_spell_immunity(self) -> None:
        self.assertEqual(runtime_status_id("debuff_immunity"), "debuff_immunity")
        self.assertEqual(runtime_status_id("spell_immunity"), "spell_immunity")
        self.assertEqual(runtime_status_id("attack_immunity"), "attack_immunity")
        self.assertEqual(runtime_status_id("banish"), "banish")
        self.assertEqual(runtime_status_id("break"), "break")
        self.assertEqual(runtime_status_id("taunt"), "taunt")
        self.assertEqual(runtime_status_id("hypnosis"), "hypnosis")
        self.assertEqual(runtime_status_id("ethereal"), "ethereal")
        for keyword in ("invisibility", "reveal", "spell_block", "spell_reflect"):
            self.assertEqual(runtime_status_id(keyword), keyword)

    def test_invisibility_breaks_on_action_unless_amount_is_zero(self) -> None:
        for amount, expected in (({}, "true"), ({"amount": "0"}, "false"), ({"amount": "1"}, "true")):
            report = Report()
            effect, ctx = _effect("apply_status", "invisibility", duration="5", **amount)
            result = effect_to_rust(effect, ctx, report)
            self.assertFalse(report.errors, amount)
            self.assertIn(f"StatusSpec::Invisibility {{ breaks_on_action: {expected} }}", result[2])

    def test_ethereal_needs_a_bounded_magic_amp(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "ethereal", amount="40", duration="4")
        result = effect_to_rust(effect, ctx, report)
        self.assertFalse(report.errors)
        self.assertIn("StatusSpec::Ethereal { magic_amp_percent:", result[2])

        report = Report()
        effect, ctx = _effect("apply_status", "ethereal", duration="4")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("magic damage amp", report.errors[0].message)

        report = Report()
        effect, ctx = _effect("apply_status", "ethereal", amount="95", duration="4")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("0..90", report.errors[0].message)


class DamageReductionStatusTests(unittest.TestCase):
    def test_generated_damage_reduction_carries_a_per_owner_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "damage_reduction", amount="80", duration="4")
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.errors, [])
        self.assertIn('StatusSpec::DamageReduction { id: "tester_damage_reduction"', result[2])
        self.assertEqual(held_status_id(effect), "tester_damage_reduction")
        self.assertEqual(runtime_status_id("tester_damage_reduction"), "tester_damage_reduction")

    def test_percent_must_be_above_zero_and_at_most_100(self) -> None:
        for bad in ("0", "101", "-5"):
            report = Report()
            effect, ctx = _effect("apply_status", "damage_reduction", amount=bad, duration="4")
            self.assertIsNone(effect_to_rust(effect, ctx, report), bad)
            self.assertTrue(report.errors, bad)
        report = Report()
        effect, ctx = _effect("apply_status", "damage_reduction", duration="4")
        self.assertIsNone(effect_to_rust(effect, ctx, report))

    def test_remove_status_damage_reduction_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "damage_reduction")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_damage_reduction", report.errors[0].message)


class StatusResistanceStatusTests(unittest.TestCase):
    def test_generated_status_resistance_carries_a_per_owner_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "status_resistance", amount="50", duration="4")
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.errors, [])
        self.assertIn('StatusSpec::StatusResistance { id: "tester_status_resistance"', result[2])
        self.assertEqual(held_status_id(effect), "tester_status_resistance")
        self.assertEqual(runtime_status_id("tester_status_resistance"), "tester_status_resistance")

    def test_percent_must_be_above_zero_and_at_most_100(self) -> None:
        for bad in ("0", "150"):
            report = Report()
            effect, ctx = _effect("apply_status", "status_resistance", amount=bad, duration="4")
            self.assertIsNone(effect_to_rust(effect, ctx, report), bad)
            self.assertTrue(report.errors, bad)

    def test_remove_status_status_resistance_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "status_resistance")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_status_resistance", report.errors[0].message)


class StackingStatusTests(unittest.TestCase):
    def test_stacks_status_carries_a_per_owner_id(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "stacks", target="trigger_unit", duration="8")
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.errors, [])
        self.assertIn('StatusSpec::Stacks { id: "tester_stacks" }', result[2])
        self.assertEqual(held_status_id(effect), "tester_stacks")
        self.assertEqual(runtime_status_id("tester_stacks"), "tester_stacks")

    def test_stacks_needs_a_duration_and_rejects_while(self) -> None:
        report = Report()
        effect, ctx = _effect("apply_status", "stacks", target="trigger_unit")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertEqual(report.errors[0].field, "duration")
        report = Report()
        effect, ctx = _effect("apply_status", "stacks", target="caster", trigger="while", duration="8")
        self.assertIsNone(effect_to_rust(effect, ctx, report))

    def test_remove_status_stacks_is_rejected_with_a_hint(self) -> None:
        report = Report()
        effect, ctx = _effect("remove_status", "stacks")
        self.assertIsNone(effect_to_rust(effect, ctx, report))
        self.assertIn("<owner>_stacks", report.errors[0].message)

    def _damage(self, ctx_has_stacks, **cells):
        report = Report()
        cells = {"scaling_value": "1", **cells}
        effect, ctx = _effect(
            "damage", "", target="trigger_unit", value="12|20", damage_type="physical",
            scaling_type="target_stacks", **cells,
        )
        ctx.owner_has_stacks = ctx_has_stacks
        ctx.levels = 2
        return effect_to_rust(effect, ctx, report), report

    def test_target_stacks_scaling_reads_the_owners_stacks_with_an_offset(self) -> None:
        result, report = self._damage(True)
        self.assertEqual(report.errors, [])
        self.assertIn('EffectScaling::TargetStacks { id: "tester_stacks", offset: 1 }', result[2])
        result, report = self._damage(True, scaling_value="0")
        self.assertIn("offset: 0 }", result[2])

    def test_target_stacks_offset_must_be_zero_or_one(self) -> None:
        for bad in ("2", "-1", "0.5"):
            result, report = self._damage(True, scaling_value=bad)
            self.assertIsNone(result, bad)
            self.assertEqual(report.errors[0].field, "scaling_value", bad)

    def test_target_stacks_needs_a_stacks_row(self) -> None:
        result, report = self._damage(False)
        self.assertIsNone(result)
        self.assertIn("apply_status,stacks", report.errors[0].message)
        result, report = self._damage(None)
        self.assertEqual(report.errors, [], "unknown owner rows skip the check")

    def test_whole_ability_with_stacks_and_scaled_damage_generates(self) -> None:
        abilities, report = load_test_ability(
            ability={"ability_type": "passive", "target_type": "no_target"},
            effects=[
                {"trigger": "on_attack_hit", "effect_target": "trigger_unit", "effect_type": "apply_status",
                 "effect_id": "stacks", "duration": "8"},
                {"trigger": "on_attack_hit", "effect_target": "trigger_unit", "effect_type": "damage",
                 "value": "12", "damage_type": "physical", "scaling_type": "target_stacks", "scaling_value": "1"},
            ],
        )
        self.assertEqual(report.errors, [])
        self.assertIn("EffectScaling::TargetStacks", str(abilities[0].effects))
        # Without the stacks row the scaled row is rejected.
        _, report = load_test_ability(
            ability={"ability_type": "passive", "target_type": "no_target"},
            effects=[
                {"trigger": "on_attack_hit", "effect_target": "trigger_unit", "effect_type": "damage",
                 "value": "12", "damage_type": "physical", "scaling_type": "target_stacks", "scaling_value": "1"},
            ],
        )
        self.assertTrue(report.errors)


class DispelStrengthTests(unittest.TestCase):
    def test_amount_one_is_a_strong_dispel_without_a_warning(self) -> None:
        report = Report()
        effect, ctx = _effect("dispel", "debuff", target="caster", amount="1")
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.errors, [])
        self.assertEqual(report.warnings, [])
        self.assertIn("DispelStrength::Strong", result[2])

    def test_blank_amount_is_a_basic_dispel(self) -> None:
        report = Report()
        effect, ctx = _effect("dispel", "buff", target="caster")
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.warnings, [])
        self.assertIn("DispelStrength::Basic", result[2])


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
            "mechanics: &[AbilityMechanic::Projectile { splash_radius: AreaRadius::Ability, speed: Some(1200.0), skillshot: false, first_hit: false, end_radius: None, origin_offset: None }],",
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


class RequireStatusTests(unittest.TestCase):
    @staticmethod
    def _gate(effect_id: str, target: str = "caster", trigger: str = "on_attack_hit", consumers=("on_buffed",)):
        effect, ctx = _effect("require_status", effect_id, target=target, trigger=trigger, kind="ability")
        ctx.owner_triggers = set(consumers)
        report = Report()
        return effect_to_rust(effect, ctx, report), report

    def test_gate_emits_the_runtime_status_id_and_trigger(self) -> None:
        result, report = self._gate("on_buffed:tester_attack_range")
        self.assertEqual(report.errors, [])
        self.assertIn(
            'AbilityEffect::RequireStatus { id: "tester_attack_range", then: AbilityTrigger::Custom("on_buffed") }',
            result[2],
        )
        # A status keyword resolves to its runtime id.
        result, report = self._gate("on_buffed:phased")
        self.assertEqual(report.errors, [])
        self.assertIn('id: "phased"', result[2])
        for target in ("owner", "attacker"):
            result, report = self._gate("on_buffed:phased", target=target)
            self.assertEqual(report.errors, [], target)

    def test_gate_feeds_its_custom_trigger(self) -> None:
        def make(trigger: str, effect_type: str, effect_id: str) -> EffectRow:
            cells = {"ability_id": "t", "effect_index": "0", "trigger": trigger, "effect_target": "caster", "effect_type": effect_type, "effect_id": effect_id}
            return EffectRow(row(cells), "t", 0, trigger, "caster", effect_type)

        report = Report()
        warn_unreachable_custom_triggers(
            [make("on_attack_hit", "require_status", "on_buffed:phased"), make("on_buffed", "heal", "")],
            set(),
            report,
        )
        self.assertFalse(report.warnings)

    def test_unknown_status_is_rejected(self) -> None:
        for bad in ("on_buffed:not-a-status", "on_buffed:no_such_thing"):
            result, report = self._gate(bad)
            self.assertIsNone(result, bad)
            self.assertTrue(any("unknown status" in e.message for e in report.errors), bad)

    def test_missing_status_trigger_target_and_consumer_are_rejected(self) -> None:
        result, report = self._gate("on_buffed")
        self.assertIsNone(result)
        self.assertTrue(any("<on_trigger>:<status_id>" in e.message for e in report.errors))
        result, report = self._gate("on_buffed:phased", target="trigger_unit")
        self.assertIsNone(result)
        self.assertTrue(any("acts on the unit using it" in e.message for e in report.errors))
        result, report = self._gate("on_buffed:phased", consumers=())
        self.assertIsNone(result)
        self.assertTrue(any("would do nothing" in e.message for e in report.errors))
        result, report = self._gate("on_attack_hit:phased", trigger="on_attack_hit")
        self.assertIsNone(result)
        result, report = self._gate("on_buffed:phased", trigger="on_buffed")
        self.assertIsNone(result)
        self.assertTrue(any("own trigger" in e.message for e in report.errors))

    def test_ability_and_item_generators_share_the_gate(self) -> None:
        for kind, target in (("ability", "caster"), ("item", "owner")):
            effect, ctx = _effect("require_status", "on_buffed:phased", target=target, trigger="on_attack_hit", kind=kind)
            ctx.owner_triggers = {"on_buffed"}
            report = Report()
            result = effect_to_rust(effect, ctx, report)
            self.assertEqual(report.errors, [], kind)
            self.assertIn("AbilityEffect::RequireStatus", result[2], kind)


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
            "on_status_removed:debuff_immunity": 'AbilityTrigger::OnStatusRemoved(StatusFilter::Id("debuff_immunity"))',
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

        # A passive-only stat still cannot be held (strength is timed-capable since G3).
        effect, ctx = _effect("stat_modifier", "health", target="owner", trigger="while:moving", kind="item", value="5")
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
    "projectile_end_radius,projectile_origin_offset,cast_range_buffer"
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
    def test_backswing_and_cast_point_up_to_two_seconds_are_accepted_silently(self) -> None:
        for value in ("0.3", "1.6", "2"):
            _, report = load_test_ability({"cast_backswing": value, "cast_time": value})
            self.assertEqual(report.errors, [])
            self.assertEqual([w for w in report.warnings if w.field in ("cast_backswing", "cast_time")], [], value)

    def test_backswing_above_the_cap_warns(self) -> None:
        _, report = load_test_ability({"cast_backswing": "2.1"})
        self.assertEqual(report.errors, [])
        self.assertTrue(any(w.field == "cast_backswing" and "2.0" in w.message for w in report.warnings))

    def test_cast_point_above_the_cap_warns(self) -> None:
        _, report = load_test_ability({"cast_time": "2.5"})
        self.assertEqual(report.errors, [])
        self.assertTrue(any(w.field == "cast_time" and "2.0" in w.message for w in report.warnings))


class CastRangeBufferTests(unittest.TestCase):
    UNIT = {"target_type": "unit", "cast_range": "3000"}

    def _load(self, **cells: str):
        return load_test_ability({**self.UNIT, **cells})

    def setUp(self) -> None:
        global AbilityGenerator
        import AbilityGenerator

    def test_buffer_is_emitted_only_when_set(self) -> None:
        abilities, report = self._load(cast_range_buffer="600")
        self.assertEqual(report.errors, [])
        self.assertIn(".cast_range_buffer(RankValue::fixed(600.0))", AbilityGenerator.ability_fn(abilities[0]))
        abilities, report = self._load()
        self.assertEqual(report.errors, [])
        self.assertNotIn("cast_range_buffer", AbilityGenerator.ability_fn(abilities[0]))

    def test_buffer_accepts_a_level_list(self) -> None:
        abilities, report = self._load(cast_range_buffer="100|200|300|400", max_rank="4", cooldown="1|2|3|4")
        self.assertEqual(report.errors, [])
        self.assertIn("RankValue::from_level_1(100.0, 100.0)", AbilityGenerator.ability_fn(abilities[0]))

    def test_buffer_level_count_must_match_the_ranks(self) -> None:
        _, report = self._load(cast_range_buffer="100|200", max_rank="4")
        self.assertTrue(any(e.field == "cast_range_buffer" for e in report.errors))

    def test_buffer_must_be_above_zero(self) -> None:
        for bad in ("0", "-5"):
            _, report = self._load(cast_range_buffer=bad)
            self.assertTrue(any(e.field == "cast_range_buffer" for e in report.errors), bad)

    def test_buffer_requires_a_unit_target(self) -> None:
        for target_type in ("point", "no_target"):
            _, report = load_test_ability({"target_type": target_type, "cast_range": "800" if target_type == "point" else "", "cast_range_buffer": "600"})
            self.assertTrue(any(e.field == "cast_range_buffer" for e in report.errors), target_type)

    def test_buffer_with_global_range_warns(self) -> None:
        _, report = self._load(cast_range="global", cast_range_buffer="600")
        self.assertEqual(report.errors, [])
        self.assertTrue(any(w.field == "cast_range_buffer" for w in report.warnings))


class TimedProjectileSpeedTests(unittest.TestCase):
    @staticmethod
    def _stat(value: str, **cells: str):
        effect = {"effect_type": "stat_modifier", "effect_id": "attack_projectile_speed", "value": value, "duration": "5"}
        effect.update(cells)
        return load_test_ability(effects=[effect])

    def test_timed_projectile_speed_fills_its_modifier_field(self) -> None:
        abilities, report = self._stat("500")
        self.assertEqual(report.errors, [])
        text = str(abilities[0].effects)
        self.assertIn("attack_projectile_speed: RankValue::fixed(500.0)", text)
        self.assertIn('id: "tester_attack_projectile_speed"', text)
        self.assertIn("StatusKind::Buff", text)
        # The range field stays zero.
        self.assertIn("attack_range: RankValue::ZERO", text)

    def test_negative_projectile_speed_is_a_debuff(self) -> None:
        abilities, report = self._stat("-200")
        self.assertEqual(report.errors, [])
        self.assertIn("StatusKind::Debuff", str(abilities[0].effects))


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


class AttackChargeTests(unittest.TestCase):
    @staticmethod
    def _charged(**cells: str):
        effect = {
            "effect_type": "stat_modifier",
            "effect_id": "attack_speed_flat",
            "value": "400",
            "duration": "20",
            "amount": "3|4|5",
        }
        effect.update(cells)
        return load_test_ability(effects=[effect])

    def test_amount_on_a_timed_row_is_the_attack_charge_count(self) -> None:
        abilities, report = self._charged()
        self.assertEqual(report.errors, [])
        self.assertEqual(report.warnings, [])
        text = str(abilities[0].effects)
        self.assertIn("attack_charges: RankValue::from_level_1(3.0, 1.0)", text)
        self.assertIn("attack_speed_flat: RankValue::fixed(400.0)", text)

    def test_blank_amount_is_not_charge_limited(self) -> None:
        abilities, report = self._charged(amount="")
        self.assertEqual(report.errors, [])
        self.assertIn("attack_charges: RankValue::ZERO", str(abilities[0].effects))

    def test_charges_must_be_whole_numbers_of_at_least_one(self) -> None:
        for bad in ("0", "2.5", "-1"):
            _, report = self._charged(amount=bad)
            self.assertTrue(
                any("whole numbers of at least 1" in e.message for e in report.errors), bad
            )

    def test_charges_are_rejected_on_while_and_resource_rows(self) -> None:
        for trigger in ("while_toggled", "passive"):
            report = Report()
            effect, ctx = _effect(
                "stat_modifier", "attack_speed_flat", trigger=trigger, target="caster",
                value="400", amount="3",
            )
            self.assertIsNone(effect_to_rust(effect, ctx, report), trigger)
            self.assertTrue(any(e.field == "amount" for e in report.errors), trigger)
        _, report = self._charged(effect_id="lifesteal", value="0.2", amount="3")
        self.assertTrue(any(e.field == "amount" for e in report.errors))

    def test_items_get_the_same_charges(self) -> None:
        report = Report()
        effect, ctx = _effect(
            "stat_modifier", "attack_speed_flat", kind="item", target="owner",
            trigger="on_use", value="400", duration="20", amount="3",
        )
        result = effect_to_rust(effect, ctx, report)
        self.assertEqual(report.errors, [])
        self.assertIn("attack_charges: RankValue::fixed(3.0)", result[2])


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


class PullPointForwardOffsetTests(unittest.TestCase):
    @staticmethod
    def _pull(**cells: str):
        effect = {
            "effect_type": "movement", "effect_id": "pull_point", "effect_target": "units_in_radius",
            "duration": "0.05", "radius": "430",
        }
        effect.update(cells)
        return load_test_ability({"aoe_radius": "430"}, [effect])

    def test_amount_is_the_forward_offset(self) -> None:
        abilities, report = self._pull(amount="250")
        self.assertEqual(report.errors, [])
        self.assertEqual([w for w in report.warnings if w.field == "amount"], [])
        self.assertIn("forward_offset: Some(RankValue::fixed(250.0))", str(abilities[0].effects))

    def test_blank_amount_keeps_the_trigger_point(self) -> None:
        abilities, report = self._pull()
        self.assertEqual(report.errors, [])
        self.assertIn("forward_offset: None", str(abilities[0].effects))

    def test_negative_offset_is_rejected(self) -> None:
        _, report = self._pull(amount="-50")
        self.assertTrue(any("forward offset cannot be negative" in e.message for e in report.errors))


class DragStunTests(unittest.TestCase):
    @staticmethod
    def _drag(effect_id: str, **cells: str):
        effect = {
            "trigger": "on_unit_contact", "effect_type": "movement", "effect_id": effect_id,
            "effect_target": "trigger_unit",
        }
        effect.update(cells)
        dash = {"effect_type": "movement", "effect_id": "dash", "value": "800", "duration": "1"}
        return load_test_ability({"target_type": "point", "cast_range": "800"}, [dash, effect])

    def test_drag_stun_binds_a_source_unique_stun_id(self) -> None:
        abilities, report = self._drag("drag_stun")
        self.assertEqual(report.errors, [])
        text = str(abilities[0].effects)
        self.assertIn("kind: DisplacementKind::Drag", text)
        self.assertIn('bound_stun: Some("tester_stun")', text)

    def test_plain_drag_binds_nothing(self) -> None:
        abilities, report = self._drag("drag")
        self.assertEqual(report.errors, [])
        self.assertIn("bound_stun: None", str(abilities[0].effects))

    def test_drag_stun_follows_the_drag_target_rules(self) -> None:
        _, report = self._drag("drag_stun", effect_target="caster")
        self.assertTrue(any("drag carries other units" in e.message for e in report.errors))


def _ability_rust(ability) -> str:
    import AbilityGenerator

    return AbilityGenerator.ability_fn(ability)


class ProjectileOriginOffsetTests(unittest.TestCase):
    @staticmethod
    def _wave(**cells: str):
        ability = {
            "target_type": "point", "cast_range": "1200", "aoe_radius": "200",
            "projectile_type": "skillshot", "projectile_speed": "1200",
        }
        ability.update(cells)
        return load_test_ability(ability)

    def test_offset_is_emitted_on_a_skillshot(self) -> None:
        abilities, report = self._wave(projectile_origin_offset="48")
        self.assertEqual(report.errors, [])
        self.assertIn("origin_offset: Some(RankValue::fixed(48.0))", _ability_rust(abilities[0]))

    def test_blank_offset_starts_at_the_caster(self) -> None:
        abilities, report = self._wave()
        self.assertEqual(report.errors, [])
        self.assertIn("origin_offset: None", _ability_rust(abilities[0]))

    def test_offset_needs_a_skillshot_and_a_positive_value(self) -> None:
        _, report = self._wave(projectile_origin_offset="0")
        self.assertTrue(any("must be above 0" in e.message for e in report.errors))
        _, report = load_test_ability(
            {"target_type": "unit", "target_team": "enemy", "cast_range": "600", "projectile_type": "targeted", "projectile_origin_offset": "48"}
        )
        self.assertTrue(any("only skillshots start in front" in e.message for e in report.errors))


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

    def test_primary_attribute_is_emitted(self) -> None:
        import HeroGenerator

        hero = self._load("true", "150")[0]
        self.assertIn("primary: Some(Attribute::Strength),", HeroGenerator.hero_def_arm(hero))

    def test_blank_attack_damage_subtracts_the_primary_attribute(self) -> None:
        # Melee total 55 and ranged total 48, minus the primary (STR 20 in the fixture).
        self.assertEqual(self._load("true", "150")[0].attack_damage, 35.0)
        self.assertEqual(self._load("false", "550")[0].attack_damage, 28.0)
        explicit = self._load("true", "150", ",attack_damage", ",31")[0]
        self.assertEqual(explicit.attack_damage, 31.0)

    def test_non_positive_attack_damage_total_is_rejected(self) -> None:
        with self.assertRaises(SystemExit) as ctx:
            self._load("true", "150", ",attack_damage", ",-20")
        self.assertIn("must be above 0", str(ctx.exception))

    def test_blank_magic_resist_is_the_uniform_27_5_percent_rating(self) -> None:
        import HeroGenerator

        self.assertEqual(HeroGenerator.HERO_MR_RATING, 6.321839)
        self.assertAlmostEqual(HeroGenerator.mr_fraction(6.321839), 0.275, places=6)
        self.assertEqual(self._load("true", "150")[0].magic_resist, 6.321839)
        # Independent of the primary attribute.
        self.assertEqual(self._load("false", "550")[0].magic_resist, 6.321839)

    def test_non_uniform_magic_resist_warns_but_is_accepted(self) -> None:
        import contextlib
        import io

        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            hero = self._load("true", "150", ",magic_resist", ",3.2")[0]
        self.assertEqual(hero.magic_resist, 3.2)
        self.assertIn("not the uniform 27.5%", err.getvalue())
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self._load("true", "150", ",magic_resist", ",6.321839")
        self.assertEqual(err.getvalue(), "")

    def test_apostrophes_do_not_split_identifiers(self) -> None:
        import HeroGenerator

        self.assertEqual(HeroGenerator.to_pascal_case("Winter's Curse"), "WintersCurse")
        self.assertEqual(HeroGenerator.to_pascal_case("Winter\u2019s Curse"), "WintersCurse")
        self.assertEqual(HeroGenerator.to_pascal_case("Queen of Pain"), "QueenOfPain")

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

    def test_base_attack_speed_is_optional_and_bounded(self) -> None:
        import HeroGenerator

        blank = self._load("true", "150", ",base_attack_speed", ",")[0]
        self.assertIsNone(blank.base_attack_speed)
        arm = HeroGenerator.hero_def_arm(blank)
        self.assertIn("combat: hero_combat(", arm)
        self.assertNotIn("base_attack_speed", arm)
        hero = self._load("true", "150", ",base_attack_speed", ",120")[0]
        self.assertEqual(hero.base_attack_speed, 120.0)
        arm = HeroGenerator.hero_def_arm(hero)
        self.assertIn("combat: CombatStats {", arm)
        self.assertIn("base_attack_speed: 120.0,", arm)
        self.assertIn("..hero_combat(", arm)
        for bad in ("0", "10", "701", "-5"):
            with self.assertRaises(SystemExit) as ctx:
                self._load("true", "150", ",base_attack_speed", "," + bad)
            self.assertIn("base_attack_speed must be within 20..700", str(ctx.exception))

    def test_is_melee_must_match_attack_range(self) -> None:
        with self.assertRaises(SystemExit) as ctx:
            self._load("true", "400")
        self.assertIn("disagrees with attack_range", str(ctx.exception))
        with self.assertRaises(SystemExit):
            self._load("false", "200")


if __name__ == "__main__":
    unittest.main()
