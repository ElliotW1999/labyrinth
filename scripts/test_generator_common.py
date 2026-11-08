"""Unit tests for shared generator validation."""

from __future__ import annotations

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
    ITEM_TRIGGERS,
    effect_to_rust,
    parse_activation_spec,
    validate_activation,
)


def row(cells: dict[str, str], line: int = 2) -> Row:
    return Row("test.csv", line, cells)


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


if __name__ == "__main__":
    unittest.main()
