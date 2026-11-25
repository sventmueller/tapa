//! The user's answers to the server's requests, and the settings the next turn carries.

use serde_json::{Value, json};
use term_thread::{Answer, Decision, Status, Update};

use super::{Ask, Choice, Event, Session, SessionError};
use crate::capabilities::MODES;
use crate::launch::{ApprovalPolicy, SandboxMode};
use crate::requests::{self, AskKind};

impl Session {
    fn reply_to(
        &self,
        request: &str,
        answer: impl FnOnce(&Ask) -> Result<(Value, Decision), SessionError>,
    ) -> Result<(), SessionError> {
        let updates = {
            let mut state = self.shared.state();
            let Some(ask) = state.asks.get(request) else {
                return Err(SessionError::UnknownRequest(request.to_string()));
            };
            let (result, decision) = answer(ask)?;
            self.shared.respond(ask.id.clone(), result)?;
            let still_asking = state.asks.len() > 1;
            let mut updates = requests::decided(&ask.offer, decision, still_asking);
            state.asks.remove(request);
            if !still_asking && state.mapper.turn().is_none() && !state.starting {
                updates.push(Update::Status(Status::Idle));
            }
            updates
        };
        self.shared.apply(updates);
        Ok(())
    }

    /// Answers the permission request `request` with `choice`; for a question, only `Dismiss` fits.
    pub fn answer(&self, request: &str, choice: Choice) -> Result<(), SessionError> {
        self.reply_to(request, |ask| {
            if matches!(ask.offer.kind, AskKind::Questions(_)) && choice != Choice::Dismiss {
                return Err(SessionError::UnknownRequest(request.to_string()));
            }
            let pick = match choice {
                Choice::Option(n) => Some(
                    ask.offer
                        .picks
                        .get(n)
                        .ok_or_else(|| SessionError::UnknownOption(n.to_string()))?,
                ),
                Choice::Dismiss => None,
            };
            let result = requests::response(&ask.offer.kind, pick);
            let decision = pick.map_or(Decision::Denied { message: None }, |p| p.decision());
            Ok((result, decision))
        })
    }

    /// Answers the questions of `request`, matched to them by their text.
    pub fn answer_questions(&self, request: &str, answers: Vec<Answer>) -> Result<(), SessionError> {
        self.reply_to(request, |ask| {
            let AskKind::Questions(ids) = &ask.offer.kind else {
                return Err(SessionError::UnknownRequest(request.to_string()));
            };
            Ok((requests::answers(ids, &answers), Decision::Answered(answers)))
        })
    }

    /// Accepts the form an MCP server asked for in `request` with `content`, an object of the form's fields.
    pub fn answer_form(&self, request: &str, content: Value) -> Result<(), SessionError> {
        self.reply_to(request, |ask| {
            if ask.offer.kind != AskKind::Elicitation {
                return Err(SessionError::UnknownRequest(request.to_string()));
            }
            let result = json!({"action": "accept", "content": content});
            let decision = Decision::Allowed {
                always: false,
                edited: false,
            };
            Ok((result, decision))
        })
    }

    fn change(&self, update: impl FnOnce(&mut super::State) -> Vec<Update>) -> Result<(), SessionError> {
        if self.is_ended() {
            return Err(SessionError::Closed);
        }
        let updates = update(&mut self.shared.state());
        self.shared.apply(updates);
        self.shared.emit(Event::Capabilities);
        Ok(())
    }

    fn mode_update(state: &super::State) -> Vec<Update> {
        state
            .caps
            .mode()
            .map(|mode| Update::Mode(mode.to_string()))
            .into_iter()
            .collect()
    }

    /// Switches to the preset `id` of `capabilities::MODES` from the next turn on.
    pub fn set_mode(&self, id: &str) -> Result<(), SessionError> {
        let mode = MODES
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| SessionError::UnknownOption(id.to_string()))?;
        self.change(|state| {
            state.overrides.approval = Some(mode.approval);
            state.overrides.sandbox = Some(mode.sandbox);
            state.caps.approval = Some(mode.approval);
            state.caps.sandbox = Some(mode.sandbox);
            vec![Update::Mode(mode.id.to_string())]
        })
    }

    /// Sets the option `option` to `value`, as the ACP session names them: `model`, `effort` (or
    /// `reasoning_effort`) and `mode`.
    pub fn set_option(&self, option: &str, value: &str) -> Result<(), SessionError> {
        match option {
            "model" => self.set_model(value),
            "effort" | "reasoning_effort" | "thought_level" => self.set_effort(Some(value)),
            "mode" => self.set_mode(value),
            other => Err(SessionError::UnknownOption(other.to_string())),
        }
    }

    /// Uses `model` from the next turn on.
    pub fn set_model(&self, model: &str) -> Result<(), SessionError> {
        self.change(|state| {
            state.overrides.model = Some(model.to_string());
            state.caps.model = Some(model.to_string());
            vec![Update::Model(model.to_string())]
        })
    }

    /// Uses the reasoning effort `effort`, or the model's default with `None`, from the next turn on.
    pub fn set_effort(&self, effort: Option<&str>) -> Result<(), SessionError> {
        let effort = effort.map(str::to_string);
        self.change(|state| {
            state.overrides.effort = Some(effort.clone());
            state.caps.effort.clone_from(&effort);
            vec![Update::Effort(effort)]
        })
    }

    /// Asks as `policy` says from the next turn on.
    pub fn set_approval(&self, policy: ApprovalPolicy) -> Result<(), SessionError> {
        self.change(|state| {
            state.overrides.approval = Some(policy);
            state.caps.approval = Some(policy);
            Self::mode_update(state)
        })
    }

    /// Runs commands in `sandbox` from the next turn on.
    pub fn set_sandbox(&self, sandbox: SandboxMode) -> Result<(), SessionError> {
        self.change(|state| {
            state.overrides.sandbox = Some(sandbox);
            state.caps.sandbox = Some(sandbox);
            Self::mode_update(state)
        })
    }
}
