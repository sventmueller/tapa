use super::{AgentTab, Instant, PARK_AFTER};

impl AgentTab {
    /// Runs the usual parking check as if the grace period had elapsed.
    pub fn check_parking_for_selftest(&self) {
        self.parking.hidden_since.set(Some(Instant::now() - PARK_AFTER));
        self.consider_parking();
    }

    /// Whether a tab released its Claude process and is ready to resume the session.
    pub fn is_parked_for_selftest(&self) -> bool {
        self.parking.process.get().is_some()
    }
}
