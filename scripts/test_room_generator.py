"""Unit tests for RoomGenerator."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import RoomGenerator  # noqa: E402

REPO = Path(__file__).resolve().parents[1]
LAYOUTS = {"Camp", "Arena", "Pillars", "Large"}

ROOM_HEADER = "id,name,kind,layout,min_depth,max_depth,weight,reward,reward_amount,shop,description\n"
SPAWN_HEADER = "room_id,spawn_index,creep,class,count,x,z\n"

VALID_ROOMS = (
    "camp,Camp,safe,camp,0,0,1,none,,TRUE,Rest\n"
    "fight,Fight,combat,arena,1,2,2,gold,300,FALSE,\n"
    "brawl,Brawl,combat,pillars,1,2,1,neutral_item,,FALSE,\n"
    "boss,Boss Room,boss,large,3,3,1,none,,FALSE,\n"
)
VALID_SPAWNS = (
    "fight,0,melee,normal,3,0,-500\n"
    "brawl,0,ranged,captain,1,100,-200\n"
    "boss,0,melee,boss,1,0,-900\n"
)


def load(rooms: str = VALID_ROOMS, spawns: str = VALID_SPAWNS, layouts: set[str] = LAYOUTS):
    with tempfile.TemporaryDirectory() as tmp:
        rooms_csv = Path(tmp) / "rooms.csv"
        spawns_csv = Path(tmp) / "room_spawns.csv"
        rooms_csv.write_text(ROOM_HEADER + rooms, encoding="utf-8")
        spawns_csv.write_text(SPAWN_HEADER + spawns, encoding="utf-8")
        return RoomGenerator.load(rooms_csv, spawns_csv, layouts)


def messages(report) -> list[str]:
    return [e.message for e in report.errors]


class ValidRoomsTests(unittest.TestCase):
    def test_valid_data_has_no_errors(self) -> None:
        rooms, report = load()
        self.assertFalse(report.errors, messages(report))
        self.assertEqual([r.id for r in rooms], ["camp", "fight", "brawl", "boss"])
        self.assertEqual(RoomGenerator.final_depth(rooms), 3)

    def test_render_emits_enum_catalog_and_final_depth(self) -> None:
        rooms, _ = load()
        text = RoomGenerator.render(rooms)
        self.assertIn("pub const FINAL_DEPTH: u32 = 3;", text)
        self.assertIn("pub enum RoomId {\n    Camp,\n    Fight,\n    Brawl,\n    Boss,\n}", text)
        self.assertIn("goal: RoomGoal::None", text)
        self.assertIn("goal: RoomGoal::KillAll", text)
        self.assertIn("reward: RoomReward::Gold(300)", text)
        self.assertIn("reward: RoomReward::NeutralItem", text)
        self.assertIn("class: CreepClass::Captain", text)
        self.assertIn("offset: Vec2::new(0.0, -500.0)", text)
        self.assertIn('name: "Boss Room"', text)

    def test_spawns_are_ordered_by_index(self) -> None:
        rooms, _ = load(spawns=VALID_SPAWNS + "fight,5,ranged,normal,2,0,-800\nfight,2,melee,normal,1,10,-300\n")
        fight = next(r for r in rooms if r.id == "fight")
        self.assertEqual([s.index for s in fight.spawns], [0, 2, 5])


class RoomRuleTests(unittest.TestCase):
    def assert_error(self, report, text: str) -> None:
        self.assertTrue(any(text in m for m in messages(report)), messages(report))

    def test_duplicate_id_is_an_error(self) -> None:
        _, report = load(VALID_ROOMS + "fight,Other,combat,arena,1,2,1,none,,FALSE,\n")
        self.assert_error(report, "duplicate room id")

    def test_unknown_layout_names_the_variants(self) -> None:
        _, report = load(VALID_ROOMS.replace(",pillars,", ",cave,"))
        self.assert_error(report, "no such RoomLayoutId variant")
        self.assert_error(report, "arena")

    def test_exactly_one_safe_room_at_depth_zero_with_a_shop(self) -> None:
        _, report = load(VALID_ROOMS.replace("camp,Camp,safe", "camp,Camp,combat"))
        self.assert_error(report, "exactly one safe room")
        _, report = load(VALID_ROOMS + "camp2,Camp 2,safe,camp,0,0,1,none,,TRUE,\n")
        self.assert_error(report, "exactly one safe room")
        _, report = load(VALID_ROOMS.replace("camp,0,0,1,none,,TRUE", "camp,0,1,1,none,,TRUE"))
        self.assert_error(report, "depth 0 only")
        _, report = load(VALID_ROOMS.replace("none,,TRUE,Rest", "none,,FALSE,Rest"))
        self.assert_error(report, "shop=TRUE")

    def test_safe_room_has_no_spawns(self) -> None:
        _, report = load(spawns=VALID_SPAWNS + "camp,0,melee,normal,1,0,0\n")
        self.assert_error(report, "no spawns")

    def test_non_safe_rooms_start_at_depth_one_and_need_spawns(self) -> None:
        _, report = load(VALID_ROOMS.replace("fight,Fight,combat,arena,1,2", "fight,Fight,combat,arena,0,2"))
        self.assert_error(report, "depth 1 or later")
        _, report = load(spawns="boss,0,melee,boss,1,0,-900\nbrawl,0,ranged,captain,1,100,-200\n")
        self.assert_error(report, "needs at least one row in the spawns CSV")

    def test_boss_rules(self) -> None:
        _, report = load(VALID_ROOMS.replace("boss,Boss Room,boss,large,3,3", "boss,Boss Room,boss,large,2,3"))
        self.assert_error(report, "boss rooms sit only at FINAL_DEPTH")
        _, report = load(VALID_ROOMS.replace("brawl,Brawl,combat,pillars,1,2", "brawl,Brawl,combat,pillars,1,3"))
        self.assert_error(report, "only boss rooms reach FINAL_DEPTH")
        no_boss = "\n".join(line for line in VALID_ROOMS.splitlines() if "boss" not in line) + "\n"
        _, report = load(no_boss)
        self.assert_error(report, "at least one boss room")

    def test_boss_room_without_a_boss_spawn_warns(self) -> None:
        _, report = load(spawns=VALID_SPAWNS.replace("melee,boss,1", "melee,captain,1"))
        self.assertFalse(report.errors, messages(report))
        self.assertTrue(any("boss-class spawn" in w.message for w in report.warnings))

    def test_every_depth_below_final_needs_an_eligible_room(self) -> None:
        # Depth 2 has nothing: fight and brawl only reach depth 1.
        rooms = VALID_ROOMS.replace("fight,Fight,combat,arena,1,2", "fight,Fight,combat,arena,1,1").replace(
            "brawl,Brawl,combat,pillars,1,2", "brawl,Brawl,combat,pillars,1,1"
        )
        _, report = load(rooms)
        self.assert_error(report, "no combat or elite room is eligible at depth 2")

    def test_a_single_eligible_room_warns(self) -> None:
        rooms = VALID_ROOMS.replace("brawl,Brawl,combat,pillars,1,2", "brawl,Brawl,combat,pillars,1,1")
        _, report = load(rooms)
        self.assertFalse(report.errors, messages(report))
        self.assertTrue(any("only one room" in w.message and "depth 2" in w.message for w in report.warnings))

    def test_gold_needs_an_amount(self) -> None:
        _, report = load(VALID_ROOMS.replace("gold,300", "gold,"))
        self.assert_error(report, "needs reward_amount >= 1")
        _, report = load(VALID_ROOMS.replace("gold,300", "gold,0"))
        self.assertTrue(report.errors)
        _, report = load(VALID_ROOMS.replace("neutral_item,,", "neutral_item,50"))
        self.assertTrue(any("ignored" in w.message for w in report.warnings))

    def test_depth_range_must_be_ordered_and_numeric(self) -> None:
        _, report = load(VALID_ROOMS.replace("fight,Fight,combat,arena,1,2", "fight,Fight,combat,arena,2,1"))
        self.assert_error(report, "at least min_depth")
        _, report = load(VALID_ROOMS.replace("fight,Fight,combat,arena,1,2", "fight,Fight,combat,arena,x,2"))
        self.assertTrue(report.errors)

    def test_weight_defaults_to_one_and_must_be_positive(self) -> None:
        rooms, _ = load(VALID_ROOMS.replace("arena,1,2,2,gold", "arena,1,2,,gold"))
        self.assertEqual(next(r for r in rooms if r.id == "fight").weight, 1)
        _, report = load(VALID_ROOMS.replace("arena,1,2,2,gold", "arena,1,2,0,gold"))
        self.assertTrue(report.errors)


class SpawnRuleTests(unittest.TestCase):
    def assert_error(self, report, text: str) -> None:
        self.assertTrue(any(text in m for m in messages(report)), messages(report))

    def test_unknown_room_creep_class_and_count(self) -> None:
        _, report = load(spawns=VALID_SPAWNS + "ghost,0,melee,normal,1,0,0\n")
        self.assert_error(report, "no room with this id")
        _, report = load(spawns=VALID_SPAWNS.replace("melee,normal,3", "flying,normal,3"))
        self.assert_error(report, "unknown value")
        _, report = load(spawns=VALID_SPAWNS.replace("melee,normal,3", "melee,elder,3"))
        self.assert_error(report, "unknown value")
        _, report = load(spawns=VALID_SPAWNS.replace("melee,normal,3", "melee,normal,0"))
        self.assertTrue(report.errors)

    def test_duplicate_spawn_index_and_missing_position(self) -> None:
        _, report = load(spawns=VALID_SPAWNS + "fight,0,ranged,normal,1,5,5\n")
        self.assert_error(report, "duplicate spawn_index")
        _, report = load(spawns=VALID_SPAWNS.replace("melee,normal,3,0,-500", "melee,normal,3,,-500"))
        self.assertTrue(report.errors)


class RepoDataTests(unittest.TestCase):
    def test_layouts_are_read_from_the_marker_region(self) -> None:
        self.assertEqual(RoomGenerator.find_layouts(REPO), LAYOUTS)

    def test_the_repo_data_validates(self) -> None:
        rooms, report = RoomGenerator.load(
            REPO / "data" / "rooms.csv", REPO / "data" / "room_spawns.csv", RoomGenerator.find_layouts(REPO)
        )
        self.assertFalse(report.errors, messages(report))
        self.assertFalse(report.warnings, [w.message for w in report.warnings])
        self.assertEqual(RoomGenerator.final_depth(rooms), 4)

    @unittest.skipUnless(shutil.which("rustfmt"), "rustfmt is needed to normalise the output")
    def test_generated_catalog_is_byte_identical_to_the_committed_file(self) -> None:
        rooms, _ = RoomGenerator.load(
            REPO / "data" / "rooms.csv", REPO / "data" / "room_spawns.csv", RoomGenerator.find_layouts(REPO)
        )
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "room_catalog.rs"
            out.write_text(RoomGenerator.render(rooms), encoding="utf-8")
            subprocess.run(["rustfmt", "--edition", "2024", str(out)], check=True)
            self.assertEqual(out.read_text(encoding="utf-8"), (REPO / RoomGenerator.CATALOG_PATH).read_text(encoding="utf-8"))

    def test_check_flag_writes_nothing(self) -> None:
        before = (REPO / RoomGenerator.CATALOG_PATH).read_bytes()
        result = subprocess.run(
            [sys.executable, str(REPO / "scripts" / "RoomGenerator.py"), "--check"], capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((REPO / RoomGenerator.CATALOG_PATH).read_bytes(), before)

    def test_invalid_csv_exits_without_writing(self) -> None:
        before = (REPO / RoomGenerator.CATALOG_PATH).read_bytes()
        with tempfile.TemporaryDirectory() as tmp:
            rooms = Path(tmp) / "rooms.csv"
            spawns = Path(tmp) / "spawns.csv"
            rooms.write_text(ROOM_HEADER + "x,X,combat,nowhere,1,1,1,none,,FALSE,\n", encoding="utf-8")
            spawns.write_text(SPAWN_HEADER, encoding="utf-8")
            result = subprocess.run(
                [sys.executable, str(REPO / "scripts" / "RoomGenerator.py"), str(rooms), str(spawns)],
                capture_output=True,
                text=True,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((REPO / RoomGenerator.CATALOG_PATH).read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
