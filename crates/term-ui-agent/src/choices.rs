//! What the user picked last for each agent: model, reasoning effort and permission mode, which the agent's next
//! sessions start with, also those that come back after a restart. The app keeps them (`Host::agent_choice`).

use crate::AgentChoice;

/// What the user picked last for the agent with the id `agent`.
pub(crate) fn of(agent: &str) -> AgentChoice {
    crate::host::host().agent_choice(agent)
}

/// Keeps what `change` sets for the agent with the id `agent`.
pub(crate) fn remember(agent: &str, change: impl FnOnce(&mut AgentChoice)) {
    let host = crate::host::host();
    let mut choice = host.agent_choice(agent);
    change(&mut choice);
    host.remember_choice(agent, choice);
}
