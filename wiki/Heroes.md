# Heroes

### Heroes

| Hero | Primary | Kit |
| --- | --- | --- |
| Vanguard | Strength | Dash, Shockwave (brief **forceful** push), Bolt (unit), Nova |
| Skirmisher | Agility | Blink, Flurry (silence+disarm), Caltrops, Execute (unit, bonus vs low HP) |
| Arcanist | Intelligence | Missile, Frost (root), Barrier (debuff immunity), Meteor (stun) |
| Warden | Strength | Bulwark, ShieldBash, Taunt, Aegis (stubs — implement later) |
| Hexer | Intelligence | HexBolt, Curse, Ward, Ritual (stubs — implement later) |
| Queen of Pain | Intelligence | Shadow Strike, QoP Blink, Scream of Pain, Sonic Wave |
| Magnus | Strength | Shockwave, Empower, Skewer, Reverse Polarity |
| Sniper | Agility | Shrapnel, Headshot, Take Aim, Assassinate |
| Ursa | Agility | Earthshock, Overpower, Fury Swipes, Enrage |
| Slark | Agility | Dark Pact, Pounce, Essence Shift, Shadow Dance |
| Winter Wyvern | Intelligence | Arctic Burn (flight: range, attack point, phasing; attacks burn and slow), Splinter Blast (splash projectile), Cold Embrace (freeze an ally: physical immunity and healing), Winter's Curse (dispel + stun; nearby enemies are taunted onto the target) |
| Shadowheart | Strength | Purification (heal an ally and pure-damage the enemies around it), Warding Bond (10 s: armor and percent magic resistance on both, and every hit the ally takes is also dealt to Shadowheart), Spirit Guardians (three orbiting spirits deal pure damage on contact), Sharran Conversion (enemy creeps that die near her rise again as her allies for 30 s) |
| Gyrocopter | Agility | Rocket Barrage, Homing Missile, Flak Cannon, Call Down |

Each hero has its own base Str/Agi/Int and per-level growth. Heroes start at **level 6** (6 skill points, five levels of attribute growth applied) with **25000 gold** (`units::STARTING_HERO_LEVEL` / `STARTING_GOLD`). The hero select screen is a grid grouped by primary attribute; the map (terrain, buildings, shops, obstacle course, creep waves) is generated only once a hero is picked. Enemy heroes within **1500** units (`scale::XP_SHARE_RADIUS`) of a dying unit share its XP bounty.

### Adding heroes (HeroGenerator)

Append kits via CSV — the script patches `src/heroes.rs` / `src/components.rs` (and the heroes table in `wiki/Heroes.md`). Ability **gameplay** stays stubbed until you implement casts.

```bash
python3 scripts/HeroGenerator.py data/heroes.example.csv
# preview only:
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
```

Required columns: `Hero_name`, `is_melee`, `Ability_Q/W/E/R_Name`, `base_STR/AGI/INT`, `STR/AGI/INT_per_level`. Optional: `primary`, `blurb`, `base_health`, `base_health_regen`, `base_mana`, `base_mana_regen`, `attack_damage` (the level-1 total minus the primary attribute, see Hero stat conventions), `attack_range` (`is_melee` must agree with it: base ranges up to 250, `scale::MELEE_CLASSIFICATION_MAX_RANGE`, are melee and get the instant slash and cleave, so a melee hero may have e.g. 200; the generator rejects a mismatch), `base_attack_time`, `attack_point`, `attack_backswing` (both 0 to 1.0 s), `move_speed`, `armor`, `magic_resist` (blank = 6.321839, the uniform 27.5%; another value warns), `attack_projectile_speed` (ranged heroes only, above 0: the absolute speed of the basic-attack projectile, e.g. 1500; blank derives it from the attack range; ignored with a warning on melee heroes), `turn_rate` (above 0; blank keeps `facing::HERO_TURN_RATE`, 0.6), and `base_attack_speed` (20..700; blank keeps the shared 100, so attack speed is `(base_attack_speed + agility + bonuses) / 100 / BAT`; set e.g. 120 for Slark). Blank cells use the generator defaults (see `scripts/HeroGenerator.py`). Rows whose `Hero_name` already exists are skipped.

#### Hero stat conventions

Spec values are level-1 **totals**; the CSV columns are **bases** back-solved from them (Variant B), because attributes are added on top of the base in `HeroAttributes::apply_to`:

| Column | Formula |
| --- | --- |
| `base_health` | `HP − STR × 18` |
| `base_health_regen` | `regen − STR × 0.12` |
| `base_mana` | `mana − INT × 12` |
| `base_mana_regen` | `regen − INT × 0.05` |
| `armor` | `armor − AGI × 0.14` |
| `magic_resist` | `6.321839` for every hero (27.5%); Intelligence grants no magic resist, so it is flat |
| `attack_damage` | `total − primary attribute` (each point of the primary attribute adds 1 attack damage) |

Heroes spawn at level 6 with five levels of growth applied, so in-game values exceed the spec's level-1 values. Worked example, Winter Wyvern (STR 22, AGI 16, INT 26, primary INT; spec HP 604, regen 2.45, mana 387, mana regen 1.55, armor 1, damage 46, 27.5% magic resist): `base_health = 604 − 22×18 = 208`, `base_health_regen = 2.45 − 22×0.12 = −0.19`, `base_mana = 387 − 26×12 = 75`, `base_mana_regen = 1.55 − 26×0.05 = 0.25`, `armor = 1 − 16×0.14 = −1.24`, `magic_resist = 6.321839`, `attack_damage = 46 − 26 = 20`. Blank `attack_damage` defaults to 55 (melee) / 48 (ranged) minus the primary attribute, and blank `magic_resist` to 6.321839; any other `magic_resist` prints a warning. Ability ranks the spec leaves unspecified follow the half-step convention (see Ability ranks).

### Attributes

- **Strength** — max HP and HP regen
- **Agility** — armor and attack-speed rating
- **Intelligence** — max mana and mana regen (no magic resist: every hero has a flat 27.5%, rating 6.321839)
- **Primary attribute** — +1 attack damage per point (level-ups, items and timed attributes included), applied in `HeroAttributes::apply_to` / `apply_delta`
- Each level raises Str / Agi / Int using that hero's growth rates (HUD shows IAS/APS and derived combat stats)
