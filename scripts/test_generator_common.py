"""Unit tests for shared generator validation."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from generator_common import (  # noqa: E402
    ActivationSpec,
    Report,
    Row,
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
