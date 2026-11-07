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


if __name__ == "__main__":
    unittest.main()
