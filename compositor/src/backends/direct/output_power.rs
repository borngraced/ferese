//! Runtime power is independent of connector presence and configured enablement.
//! Only successful hardware transitions change the applied state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Power {
    On,
    Off,
}

#[derive(Debug)]
pub(super) struct OutputPower {
    requested: Power,
    applied: Option<Power>,
}

impl Default for OutputPower {
    fn default() -> Self {
        Self {
            requested: Power::On,
            applied: None,
        }
    }
}

impl OutputPower {
    pub fn request(&mut self, power: Power) {
        self.requested = power;
    }

    pub fn can_render(&self) -> bool {
        self.requested == Power::On
    }

    pub fn needs_power_off(&self) -> bool {
        self.requested == Power::Off && self.applied != Some(Power::Off)
    }

    pub fn needs_wake(&self) -> bool {
        self.requested == Power::Off || self.applied != Some(Power::On)
    }

    pub fn powered_off(&mut self) {
        self.applied = Some(Power::Off);
    }

    pub fn presented(&mut self) {
        self.applied = Some(Power::On);
    }

    pub fn suspended(&mut self) {
        self.applied = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_power_off_keeps_the_applied_state_and_remains_retryable() {
        let mut power = OutputPower::default();
        power.presented();
        power.request(Power::Off);
        assert!(!power.can_render());
        assert_eq!(power.applied, Some(Power::On));
        assert!(power.needs_power_off());
        power.powered_off();
        assert!(!power.needs_power_off());
    }

    #[test]
    fn waking_does_not_claim_success_before_presentation() {
        let mut power = OutputPower::default();
        power.request(Power::Off);
        power.powered_off();
        power.request(Power::On);
        assert!(power.can_render());
        assert_eq!(power.applied, Some(Power::Off));
        power.presented();
        assert!(!power.needs_wake());
    }

    #[test]
    fn pending_frame_retirement_and_suspend_do_not_lose_requested_power() {
        let mut power = OutputPower::default();
        power.request(Power::Off);
        power.presented();
        assert!(power.needs_power_off());
        power.suspended();
        assert_eq!(power.applied, None);
        assert!(power.needs_power_off());
        assert!(!power.can_render());
    }
}
