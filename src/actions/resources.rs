//! Costs, cooldowns, and charges shared by every activatable action.

use super::ActionError;
use crate::components::Mana;
use crate::items::StatusEffects;

/// What one activation spends when it commits.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ActionCost {
    pub mana: f32,
}

impl ActionCost {
    pub fn affordable(&self, mana: &Mana) -> bool {
        mana.current >= self.mana
    }

    pub fn pay(&self, mana: &mut Mana) -> bool {
        mana.try_spend(self.mana)
    }
}

/// Mutable cooldown / charge state of one action slot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ActionCooldown {
    pub remaining: f32,
    /// Remaining charges for charge-based actions (`None` = not charge-based).
    pub charges: Option<u32>,
    /// Seconds until the next charge is restored (while below the maximum).
    pub charge_restore_remaining: f32,
}

impl ActionCooldown {
    pub fn ready(&self) -> bool {
        self.remaining <= 0.0 && self.charges != Some(0)
    }

    /// Start the cooldown and spend a charge.
    pub fn start(&mut self, cooldown: f32) {
        self.remaining = cooldown;
        if let Some(charges) = self.charges.as_mut() {
            *charges = charges.saturating_sub(1);
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.remaining = (self.remaining - dt).max(0.0);
    }

    /// Give a charge-based action its charges the first time it is seen.
    pub fn init_charges(&mut self, max_charges: u32) {
        self.charges.get_or_insert(max_charges);
    }

    /// Restore one charge every `restore_time` seconds while below `max_charges`.
    pub fn restore_charges(&mut self, max_charges: u32, restore_time: f32, dt: f32) {
        let charges = self.charges.get_or_insert(max_charges);
        if *charges >= max_charges {
            self.charge_restore_remaining = restore_time;
            return;
        }
        self.charge_restore_remaining -= dt;
        if self.charge_restore_remaining <= 0.0 {
            *charges += 1;
            self.charge_restore_remaining += restore_time;
        }
    }
}

/// Source-independent readiness: off cooldown with a charge, affordable, and the
/// caster is able to act (not silenced / stunned).
pub fn check_ready(
    cooldown: &ActionCooldown,
    cost: &ActionCost,
    mana: &Mana,
    statuses: Option<&StatusEffects>,
) -> Result<(), ActionError> {
    if !cooldown.ready() {
        return Err(ActionError::OnCooldown);
    }
    if !cost.affordable(mana) {
        return Err(ActionError::NotEnoughMana);
    }
    if statuses.is_some_and(|s| !s.can_cast()) {
        return Err(ActionError::CasterDisabled);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldowns_and_charges_gate_readiness() {
        let mut cd = ActionCooldown::default();
        assert!(cd.ready());
        cd.start(2.0);
        assert!(!cd.ready());
        cd.tick(2.5);
        assert!(cd.ready());

        let mut charged = ActionCooldown::default();
        charged.restore_charges(2, 3.0, 0.0);
        assert_eq!(charged.charges, Some(2));
        charged.start(0.0);
        assert_eq!(charged.charges, Some(1));
        assert!(charged.ready());
        charged.start(0.0);
        assert!(!charged.ready(), "out of charges");
        charged.restore_charges(2, 3.0, 1.0);
        assert_eq!(
            charged.charges,
            Some(0),
            "restoring takes the full restore time"
        );
        charged.restore_charges(2, 3.0, 2.0);
        assert_eq!(charged.charges, Some(1));
    }

    #[test]
    fn readiness_checks_cooldown_then_mana_then_caster() {
        let cost = ActionCost { mana: 50.0 };
        let mana = Mana::new(100.0, 0.0);
        let cooling = ActionCooldown {
            remaining: 1.0,
            ..Default::default()
        };
        assert_eq!(
            check_ready(&cooling, &cost, &Mana::new(0.0, 0.0), None),
            Err(ActionError::OnCooldown)
        );
        assert_eq!(
            check_ready(
                &ActionCooldown::default(),
                &cost,
                &Mana::new(10.0, 0.0),
                None
            ),
            Err(ActionError::NotEnoughMana)
        );
        assert_eq!(
            check_ready(&ActionCooldown::default(), &cost, &mana, None),
            Ok(())
        );
    }
}
