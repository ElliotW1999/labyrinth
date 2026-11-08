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
    warn_unreachable_custom_triggers,
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
    effect = EffectRow(row(base), "tester", 0, trigger, target, effect_type)
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


if __name__ == "__main__":
    unittest.main()
