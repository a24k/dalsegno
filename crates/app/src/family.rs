//! The IP family policy. One active family, switched as a unit with its leases (FR-A3, FR-A4).

use dalsegno_core::stream::{IpFamily, IpPolicy};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

const BASE_AVOIDANCE_SECONDS: u64 = 6 * 3600;
const MAX_AVOIDANCE_SECONDS: u64 = 96 * 3600;

/// Why the active family changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SwitchReason {
    /// The origin asked for a bot check.
    BotCheck,
    /// Requests stayed refused after a URL refresh.
    PersistentRefusal,
    /// The preferred family works again.
    Recovered,
}

/// One recorded switch, for `doctor` and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FamilySwitch {
    /// Unix seconds.
    pub at: u64,
    /// Family before the switch.
    pub from: IpFamily,
    /// Family after the switch.
    pub to: IpFamily,
    /// Why it happened.
    pub reason: SwitchReason,
}

/// Result of reporting an origin-wide failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Moved to this family.
    Switched(IpFamily),
    /// No other family is allowed, so the stream must enter `error`.
    Exhausted,
}

/// The state machine in the acquisition design. Pure: time is passed in.
#[derive(Debug, Clone)]
pub struct FamilyController {
    policy: IpPolicy,
    has_ipv6: bool,
    active: IpFamily,
    avoid_until: Option<u64>,
    avoidance: u64,
    last_switch: Option<FamilySwitch>,
}

/// The controller shared by every stream.
pub type SharedFamily = Arc<Mutex<FamilyController>>;

impl FamilyController {
    /// Starts on the preferred family, or IPv4 when IPv6 is preferred but has no route.
    pub fn new(policy: IpPolicy, has_ipv6: bool) -> Self {
        let active = match policy {
            IpPolicy::V6First if has_ipv6 => IpFamily::V6,
            IpPolicy::V6First | IpPolicy::V4First | IpPolicy::V4Only => IpFamily::V4,
            IpPolicy::V6Only => IpFamily::V6,
        };
        Self {
            policy,
            has_ipv6,
            active,
            avoid_until: None,
            avoidance: BASE_AVOIDANCE_SECONDS,
            last_switch: None,
        }
    }

    /// Wraps the controller for sharing.
    pub fn shared(self) -> SharedFamily {
        Arc::new(Mutex::new(self))
    }

    /// Returns the policy.
    pub fn policy(&self) -> IpPolicy {
        self.policy
    }

    /// Returns the family in use.
    pub fn active(&self) -> IpFamily {
        self.active
    }

    /// Returns the most recent switch.
    pub fn last_switch(&self) -> Option<FamilySwitch> {
        self.last_switch
    }

    /// Returns the family the policy wants.
    pub fn preferred(&self) -> IpFamily {
        match self.policy {
            IpPolicy::V6First | IpPolicy::V6Only => IpFamily::V6,
            IpPolicy::V4First | IpPolicy::V4Only => IpFamily::V4,
        }
    }

    fn fallback(&self) -> Option<IpFamily> {
        match self.policy {
            IpPolicy::V6First if self.has_ipv6 => Some(IpFamily::V4),
            IpPolicy::V4First if self.has_ipv6 => Some(IpFamily::V6),
            IpPolicy::V6First | IpPolicy::V4First | IpPolicy::V6Only | IpPolicy::V4Only => None,
        }
    }

    /// Records an origin-wide failure of the active family (D-26).
    pub fn on_origin_failure(&mut self, now: u64, reason: SwitchReason) -> Transition {
        match self.fallback() {
            Some(fallback) if self.active != fallback => {
                self.switch_to(fallback, now, reason);
                self.avoid_until = Some(now.saturating_add(self.avoidance));
                Transition::Switched(fallback)
            }
            Some(_) | None => Transition::Exhausted,
        }
    }

    /// Returns whether the preferred family should be tried again.
    pub fn probe_due(&self, now: u64) -> bool {
        self.active != self.preferred() && self.avoid_until.is_some_and(|until| now >= until)
    }

    /// Records the result of trying the preferred family.
    pub fn on_probe_result(&mut self, now: u64, succeeded: bool) {
        if succeeded {
            self.switch_to(self.preferred(), now, SwitchReason::Recovered);
            self.avoid_until = None;
            self.avoidance = BASE_AVOIDANCE_SECONDS;
        } else {
            self.avoidance = (self.avoidance * 2).min(MAX_AVOIDANCE_SECONDS);
            self.avoid_until = Some(now.saturating_add(self.avoidance));
        }
    }

    fn switch_to(&mut self, to: IpFamily, at: u64, reason: SwitchReason) {
        self.last_switch = Some(FamilySwitch {
            at,
            from: self.active,
            to,
            reason,
        });
        self.active = to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000;

    fn switched() -> FamilyController {
        let mut controller = FamilyController::new(IpPolicy::V6First, true);
        controller.on_origin_failure(NOW, SwitchReason::BotCheck);
        controller
    }

    #[test]
    fn starts_on_the_policy_family() {
        let start = |policy, has_ipv6| FamilyController::new(policy, has_ipv6).active();

        assert_eq!(start(IpPolicy::V6First, true), IpFamily::V6);
        assert_eq!(start(IpPolicy::V6First, false), IpFamily::V4);
        assert_eq!(start(IpPolicy::V4First, true), IpFamily::V4);
        assert_eq!(start(IpPolicy::V6Only, false), IpFamily::V6);
        assert_eq!(start(IpPolicy::V4Only, true), IpFamily::V4);
    }

    #[test]
    fn failure_moves_to_the_fallback_and_records_why() {
        let mut controller = FamilyController::new(IpPolicy::V6First, true);

        let transition = controller.on_origin_failure(NOW, SwitchReason::PersistentRefusal);

        assert_eq!(transition, Transition::Switched(IpFamily::V4));
        assert_eq!(
            controller.last_switch(),
            Some(FamilySwitch {
                at: NOW,
                from: IpFamily::V6,
                to: IpFamily::V4,
                reason: SwitchReason::PersistentRefusal,
            })
        );
    }

    #[test]
    fn v4_first_swaps_the_roles() {
        let mut controller = FamilyController::new(IpPolicy::V4First, true);

        let transition = controller.on_origin_failure(NOW, SwitchReason::BotCheck);

        assert_eq!(transition, Transition::Switched(IpFamily::V6));
        assert!(controller.probe_due(NOW + BASE_AVOIDANCE_SECONDS));
    }

    #[test]
    fn failure_on_the_fallback_is_exhausted() {
        let mut controller = switched();

        let transition = controller.on_origin_failure(NOW, SwitchReason::BotCheck);

        assert_eq!(transition, Transition::Exhausted);
        assert_eq!(controller.active(), IpFamily::V4);
    }

    #[test]
    fn only_policies_never_switch() {
        for policy in [IpPolicy::V6Only, IpPolicy::V4Only] {
            let mut controller = FamilyController::new(policy, true);

            let transition = controller.on_origin_failure(NOW, SwitchReason::BotCheck);

            assert_eq!(transition, Transition::Exhausted);
            assert_eq!(controller.last_switch(), None);
        }
    }

    #[test]
    fn without_ipv6_there_is_no_fallback() {
        for policy in [IpPolicy::V6First, IpPolicy::V4First] {
            let mut controller = FamilyController::new(policy, false);

            let transition = controller.on_origin_failure(NOW, SwitchReason::BotCheck);

            assert_eq!(transition, Transition::Exhausted);
            assert!(!controller.probe_due(u64::MAX));
        }
    }

    #[test]
    fn probe_waits_for_the_avoidance_period() {
        let controller = switched();

        assert!(!controller.probe_due(NOW + BASE_AVOIDANCE_SECONDS - 1));
        assert!(controller.probe_due(NOW + BASE_AVOIDANCE_SECONDS));
    }

    #[test]
    fn preferred_family_needs_no_probe() {
        assert!(!FamilyController::new(IpPolicy::V6First, true).probe_due(u64::MAX));
    }

    #[test]
    fn successful_probe_returns_to_the_preferred_family() {
        let mut controller = switched();

        controller.on_probe_result(NOW, true);

        assert_eq!(controller.active(), IpFamily::V6);
        assert_eq!(
            controller.last_switch().unwrap().reason,
            SwitchReason::Recovered
        );
        assert!(!controller.probe_due(u64::MAX));
    }

    #[test]
    fn failed_probes_grow_the_avoidance_period_up_to_a_limit() {
        let mut controller = switched();

        controller.on_probe_result(NOW, false);
        assert!(!controller.probe_due(NOW + 2 * BASE_AVOIDANCE_SECONDS - 1));
        assert!(controller.probe_due(NOW + 2 * BASE_AVOIDANCE_SECONDS));

        for _ in 0..10 {
            controller.on_probe_result(NOW, false);
        }
        assert!(controller.probe_due(NOW + MAX_AVOIDANCE_SECONDS));
    }

    #[test]
    fn recovery_resets_the_avoidance_period() {
        let mut controller = switched();
        controller.on_probe_result(NOW, false);
        controller.on_probe_result(NOW, true);

        controller.on_origin_failure(NOW, SwitchReason::BotCheck);

        assert!(controller.probe_due(NOW + BASE_AVOIDANCE_SECONDS));
    }
}
