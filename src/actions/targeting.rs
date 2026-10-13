//! Target rules shared by every activatable action.

use bevy::prelude::*;

use super::{ActionError, ActionSpec};
use crate::components::{CastTarget, Team};
use crate::dimensions::cast_distance;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    /// Fires immediately; aims at the caster's position.
    NoTarget,
    /// Always targets the caster (as the unit target).
    SelfTarget,
    /// Must be cast on a unit matching `target_team`.
    Unit,
    /// Ground point / skillshot direction. A hovered unit is accepted as the aim.
    Point,
    /// Ground area centred on the aim. A hovered unit is accepted as the aim.
    Area,
    /// A direction from the caster: never out of range; the aim is projected to
    /// `cast_range` along the clicked direction.
    Direction,
}

impl TargetType {
    /// Activates without picking anything (hotkey casts immediately).
    pub fn is_instant(self) -> bool {
        matches!(self, TargetType::NoTarget | TargetType::SelfTarget)
    }

    /// Only unit targets are accepted.
    pub fn needs_unit(self) -> bool {
        self == TargetType::Unit
    }

    /// Never needs to walk into range.
    pub fn ignores_range(self) -> bool {
        matches!(
            self,
            TargetType::NoTarget | TargetType::SelfTarget | TargetType::Direction
        )
    }
}

/// Which units an action (or one of its effects) may affect, relative to the caster.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetTeam {
    Enemy,
    /// Same team, including the caster.
    Ally,
    /// Both teams.
    Any,
    /// Only the caster itself.
    SelfOnly,
}

/// Team-only check. For [`TargetTeam::SelfOnly`] this cannot tell the caster from its
/// allies; [`target_allowed`] also compares the entity.
pub fn team_allows(rule: TargetTeam, caster: Team, target: Team) -> bool {
    match rule {
        TargetTeam::Enemy => target == caster.enemy(),
        TargetTeam::Ally | TargetTeam::SelfOnly => target == caster,
        TargetTeam::Any => true,
    }
}

pub fn target_allowed(
    rule: TargetTeam,
    caster: Entity,
    caster_team: Team,
    target: Entity,
    target_team: Team,
) -> bool {
    match rule {
        TargetTeam::SelfOnly => target == caster,
        _ => team_allows(rule, caster_team, target_team),
    }
}

/// Snapshot of a candidate unit target.
#[derive(Debug, Clone, Copy)]
pub struct UnitInfo {
    pub position: Vec3,
    pub team: Team,
    pub alive: bool,
    pub bounds: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedTarget {
    pub target: CastTarget,
    pub aim: Vec3,
    pub target_bounds: Option<f32>,
}

/// Where the activating unit stands.
#[derive(Debug, Clone, Copy)]
pub struct CasterInfo {
    pub entity: Entity,
    pub team: Team,
    pub position: Vec3,
    pub bounds: f32,
}

/// Validate `target` against the action's target type and team rule.
pub fn resolve_target(
    spec: &ActionSpec,
    caster: &CasterInfo,
    target: CastTarget,
    lookup: impl Fn(Entity) -> Option<UnitInfo>,
) -> Result<ResolvedTarget, ActionError> {
    let unit = |entity: Entity| {
        lookup(entity)
            .filter(|info| {
                info.alive
                    && target_allowed(
                        spec.target_team,
                        caster.entity,
                        caster.team,
                        entity,
                        info.team,
                    )
            })
            .ok_or(ActionError::InvalidTarget)
    };
    match spec.target_type {
        TargetType::NoTarget => Ok(ResolvedTarget {
            target: CastTarget::None,
            aim: caster.position,
            target_bounds: None,
        }),
        TargetType::SelfTarget => Ok(ResolvedTarget {
            target: CastTarget::Unit(caster.entity),
            aim: caster.position,
            target_bounds: None,
        }),
        TargetType::Direction => {
            let toward = match target {
                CastTarget::Unit(entity) => unit(entity)?.position,
                CastTarget::Point(point) => point,
                CastTarget::None => return Err(ActionError::InvalidTarget),
            };
            let dir = Vec3::new(
                toward.x - caster.position.x,
                0.0,
                toward.z - caster.position.z,
            )
            .try_normalize()
            .ok_or(ActionError::InvalidTarget)?;
            let aim = caster.position + dir * spec.cast_range;
            Ok(ResolvedTarget {
                target: CastTarget::Point(aim),
                aim,
                target_bounds: None,
            })
        }
        TargetType::Unit | TargetType::Point | TargetType::Area => match target {
            CastTarget::Unit(entity) => {
                let info = unit(entity)?;
                Ok(ResolvedTarget {
                    target,
                    aim: info.position,
                    target_bounds: Some(info.bounds),
                })
            }
            CastTarget::Point(point) if !spec.target_type.needs_unit() => Ok(ResolvedTarget {
                target,
                aim: point,
                target_bounds: None,
            }),
            _ => Err(ActionError::InvalidTarget),
        },
    }
}

pub fn in_range(
    spec: &ActionSpec,
    caster_pos: Vec3,
    caster_bounds: f32,
    resolved: &ResolvedTarget,
) -> bool {
    spec.target_type.ignores_range()
        || cast_distance(
            caster_pos,
            caster_bounds,
            resolved.aim,
            resolved.target_bounds,
        ) <= spec.cast_range
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{ActionCooldown, ActionCost, ActionId};
    use crate::components::AbilityId;

    fn spec(target_type: TargetType, target_team: TargetTeam, cast_range: f32) -> ActionSpec {
        ActionSpec {
            action: ActionId::Ability(AbilityId::Bolt),
            level: 1,
            target_type,
            target_team,
            cast_range,
            cast_time: 0.0,
            channel_time: 0.0,
            backswing: 0.0,
            cost: ActionCost::default(),
            cooldown: 0.0,
            cooldown_state: ActionCooldown::default(),
            disable: crate::actions::DisableRule::SilenceOrStun,
            occupies_caster: true,
        }
    }

    fn caster() -> CasterInfo {
        CasterInfo {
            entity: Entity::from_raw_u32(1).unwrap(),
            team: Team::Radiant,
            position: Vec3::new(10.0, 0.0, 0.0),
            bounds: 20.0,
        }
    }

    fn unit(team: Team) -> impl Fn(Entity) -> Option<UnitInfo> {
        move |_| {
            Some(UnitInfo {
                position: Vec3::new(500.0, 0.0, 0.0),
                team,
                alive: true,
                bounds: 10.0,
            })
        }
    }

    #[test]
    fn no_target_and_self_target_aim_at_the_caster_and_ignore_range() {
        let c = caster();
        let none = spec(TargetType::NoTarget, TargetTeam::Enemy, 0.0);
        let resolved = resolve_target(
            &none,
            &c,
            CastTarget::Point(Vec3::X * 900.0),
            unit(Team::Dire),
        )
        .unwrap();
        assert_eq!(resolved.target, CastTarget::None);
        assert_eq!(resolved.aim, c.position);
        assert!(in_range(&none, c.position, c.bounds, &resolved));

        let me = spec(TargetType::SelfTarget, TargetTeam::SelfOnly, 0.0);
        let resolved = resolve_target(&me, &c, CastTarget::None, unit(Team::Dire)).unwrap();
        assert_eq!(resolved.target, CastTarget::Unit(c.entity));
        assert!(in_range(&me, c.position, c.bounds, &resolved));
    }

    #[test]
    fn direction_targets_project_to_cast_range_and_never_walk() {
        let c = caster();
        let beam = spec(TargetType::Direction, TargetTeam::Enemy, 300.0);
        let resolved = resolve_target(
            &beam,
            &c,
            CastTarget::Point(Vec3::new(10.0, 0.0, 5000.0)),
            unit(Team::Dire),
        )
        .unwrap();
        assert_eq!(resolved.aim, Vec3::new(10.0, 0.0, 300.0));
        assert_eq!(resolved.target, CastTarget::Point(resolved.aim));
        assert!(in_range(&beam, c.position, c.bounds, &resolved));
        let at_unit = resolve_target(
            &beam,
            &c,
            CastTarget::Unit(Entity::from_raw_u32(9).unwrap()),
            unit(Team::Dire),
        )
        .unwrap();
        assert!(
            at_unit.aim.distance(Vec3::new(310.0, 0.0, 0.0)) < 1e-3,
            "a hovered unit only sets the direction"
        );
        assert_eq!(
            resolve_target(&beam, &c, CastTarget::Point(c.position), unit(Team::Dire)),
            Err(ActionError::InvalidTarget),
            "no direction"
        );
    }

    #[test]
    fn team_rules_filter_unit_targets() {
        let c = caster();
        let other = CastTarget::Unit(Entity::from_raw_u32(9).unwrap());
        let me = CastTarget::Unit(c.entity);
        let cases = [
            (TargetTeam::Enemy, Team::Dire, other, true),
            (TargetTeam::Enemy, Team::Radiant, other, false),
            (TargetTeam::Ally, Team::Radiant, other, true),
            (TargetTeam::Ally, Team::Dire, other, false),
            (TargetTeam::Any, Team::Dire, other, true),
            (TargetTeam::Any, Team::Radiant, other, true),
            (TargetTeam::SelfOnly, Team::Radiant, other, false),
            (TargetTeam::SelfOnly, Team::Radiant, me, true),
        ];
        for (team_rule, team, target, ok) in cases {
            let rule = spec(TargetType::Unit, team_rule, 600.0);
            assert_eq!(
                resolve_target(&rule, &c, target, unit(team)).is_ok(),
                ok,
                "{team_rule:?} vs {team:?}"
            );
        }
    }

    #[test]
    fn range_counts_bounds_and_point_targets_accept_hovered_units() {
        let c = caster();
        let point = spec(TargetType::Point, TargetTeam::Enemy, 450.0);
        let at_unit = resolve_target(
            &point,
            &c,
            CastTarget::Unit(Entity::from_raw_u32(9).unwrap()),
            unit(Team::Dire),
        )
        .unwrap();
        assert_eq!(at_unit.target_bounds, Some(10.0));
        // 490 apart, minus 20 + 10 of bounds = 460 > 450.
        assert!(!in_range(&point, c.position, c.bounds, &at_unit));
        let wider = spec(TargetType::Point, TargetTeam::Enemy, 460.0);
        assert!(in_range(&wider, c.position, c.bounds, &at_unit));
        let unit_only = spec(TargetType::Unit, TargetTeam::Enemy, 600.0);
        assert_eq!(
            resolve_target(&unit_only, &c, CastTarget::Point(Vec3::X), unit(Team::Dire)),
            Err(ActionError::InvalidTarget)
        );
    }
}
