//! The user's answers to the server's requests, and switching the session's agent, model and variant.

use std::sync::Arc;

use serde_json::{Value, json};
use term_thread::{Answer, Decision, ErrorKind, Update};

use super::{After, Choice, Event, Session, SessionError, Shared, now, unit};
use crate::capabilities::ModelRef;
use crate::http::encode;
use crate::requests::{self, AskKind, Offer};

/// What a failed switch puts back, and the notice it shows.
fn undo(restore: impl FnOnce(&mut super::State) -> Vec<Update> + Send + 'static) -> After {
    Box::new(move |result, shared: &Arc<Shared>| {
        if let Err(error) = result {
            let updates = {
                let mut state = shared.state();
                let mut updates = restore(&mut state);
                updates.extend(state.mapper.error(ErrorKind::Other, error.to_string(), now()));
                updates
            };
            shared.apply(updates);
            shared.emit(Event::Capabilities);
        }
    })
}

impl Session {
    fn take_ask(&self, request: &str, fits: impl Fn(&Offer) -> bool) -> Result<Offer, SessionError> {
        let mut state = self.shared.state();
        if !state.asks.get(request).is_some_and(fits) {
            return Err(SessionError::UnknownRequest(request.to_string()));
        }
        state.asks.remove(request).ok_or(SessionError::Closed)
    }

    /// Sends `body` to the request's `rest` path and shows `decision` at once; a failed answer becomes a notice.
    fn reply_to(
        &self,
        offer: &Offer,
        method: &'static str,
        rest: String,
        body: Option<Value>,
        decision: Decision,
    ) -> Result<(), SessionError> {
        let path = self.shared.session_path(&rest)?;
        let still_asking = !self.shared.state().asks.is_empty();
        self.shared.apply(requests::decided(offer, decision, still_asking));
        let after: After = Box::new(|result, shared| {
            if let Err(error) = result {
                let updates = shared.state().mapper.error(ErrorKind::Other, error.to_string(), now());
                shared.apply(updates);
            }
        });
        drop(self.shared.call(method, path, body, Some(after), unit));
        Ok(())
    }

    /// Answers the request `request` with `choice`: a permission with one of its options or a rejection, a form
    /// only with `Dismiss`, which cancels it.
    pub fn answer(&self, request: &str, choice: Choice) -> Result<(), SessionError> {
        let offer = self.take_ask(request, |offer| {
            offer.kind == AskKind::Permission || choice == Choice::Dismiss
        })?;
        let id = encode(request);
        if let AskKind::Form(_) = offer.kind {
            return self.reply_to(
                &offer,
                "DELETE",
                format!("/form/{id}"),
                None,
                Decision::Denied { message: None },
            );
        }
        let pick = match choice {
            Choice::Option(n) => match offer.picks.get(n) {
                Some(pick) => *pick,
                None => {
                    self.shared.state().asks.insert(request.to_string(), offer);
                    return Err(SessionError::UnknownOption(n.to_string()));
                }
            },
            Choice::Dismiss => requests::Pick::Reject,
        };
        let body = json!({"decision": pick.wire()});
        self.reply_to(
            &offer,
            "POST",
            format!("/permission/{id}/reply"),
            Some(body),
            pick.decision(),
        )
    }

    /// Answers the questions of the form `request`, matched to its fields by their text.
    pub fn answer_questions(&self, request: &str, answers: Vec<Answer>) -> Result<(), SessionError> {
        let offer = self.take_ask(request, |offer| matches!(offer.kind, AskKind::Form(_)))?;
        let AskKind::Form(fields) = &offer.kind else {
            return Err(SessionError::UnknownRequest(request.to_string()));
        };
        let body = requests::form_reply(fields, &answers);
        let rest = format!("/form/{}/reply", encode(request));
        self.reply_to(&offer, "POST", rest, Some(body), Decision::Answered(answers))
    }

    /// Answers the form `request` with `answer`, an object of its fields' keys and values (`Form.Answer`).
    pub fn answer_form(&self, request: &str, answer: Value) -> Result<(), SessionError> {
        let offer = self.take_ask(request, |offer| matches!(offer.kind, AskKind::Form(_)))?;
        let rest = format!("/form/{}/reply", encode(request));
        let decision = Decision::Allowed {
            always: false,
            edited: false,
        };
        self.reply_to(&offer, "POST", rest, Some(json!({"answer": answer})), decision)
    }

    /// Switches the session to the agent `id`, which the UI shows as a mode.
    pub fn set_mode(&self, id: &str) -> Result<(), SessionError> {
        let known = {
            let state = self.shared.state();
            state.caps.agents.is_empty() || state.caps.modes().iter().any(|a| a.id == id)
        };
        if !known {
            return Err(SessionError::UnknownOption(id.to_string()));
        }
        let path = self.shared.session_path("/agent")?;
        let previous = {
            let mut state = self.shared.state();
            state.caps.agent.replace(id.to_string())
        };
        self.shared.apply(vec![Update::Mode(id.to_string())]);
        self.shared.emit(Event::Capabilities);
        let after = undo(move |state| {
            state.caps.agent.clone_from(&previous);
            previous.into_iter().map(Update::Mode).collect()
        });
        drop(
            self.shared
                .call("POST", path, Some(json!({"agent": id})), Some(after), unit),
        );
        Ok(())
    }

    /// Sets the option `option` to `value`, as an ACP session names them: `model` (as `provider/model`), `mode`
    /// (the agent) and `effort` (the model's variant, also as `variant`, `reasoning_effort` or `thought_level`).
    pub fn set_option(&self, option: &str, value: &str) -> Result<(), SessionError> {
        match option {
            "model" => self.set_model(value),
            "effort" | "variant" | "reasoning_effort" | "thought_level" => {
                self.set_effort(Some(value).filter(|v| !v.is_empty()))
            }
            "mode" | "agent" => self.set_mode(value),
            other => Err(SessionError::UnknownOption(other.to_string())),
        }
    }

    fn switch_model(&self, model: ModelRef) -> Result<(), SessionError> {
        let path = self.shared.session_path("/model")?;
        let previous = self.shared.state().caps.model.replace(model.clone());
        self.shared
            .apply(vec![Update::Model(model.id()), Update::Effort(model.variant.clone())]);
        self.shared.emit(Event::Capabilities);
        let after = undo(move |state| {
            state.caps.model.clone_from(&previous);
            previous
                .map(|m| vec![Update::Model(m.id()), Update::Effort(m.variant)])
                .unwrap_or_default()
        });
        let body = json!({"model": model.json()});
        drop(self.shared.call("POST", path, Some(body), Some(after), unit));
        Ok(())
    }

    /// Uses `model`, as `provider/model`, from now on; its variant stays when the model offers it too.
    pub fn set_model(&self, model: &str) -> Result<(), SessionError> {
        let mut chosen = ModelRef::parse(model).ok_or_else(|| SessionError::UnknownOption(model.to_string()))?;
        {
            let state = self.shared.state();
            let offered = state
                .caps
                .models
                .iter()
                .find(|m| m.provider == chosen.provider && (m.model_id == chosen.model || m.id == chosen.model));
            if !state.caps.models.is_empty() && offered.is_none() {
                return Err(SessionError::UnknownOption(model.to_string()));
            }
            chosen.variant = state
                .caps
                .variant()
                .filter(|v| offered.is_some_and(|m| m.variants.iter().any(|o| o == v)))
                .map(str::to_string);
        }
        self.switch_model(chosen)
    }

    /// Uses the variant `variant` of the session's model, or the model's default with `None`, from now on.
    pub fn set_effort(&self, variant: Option<&str>) -> Result<(), SessionError> {
        let model = {
            let state = self.shared.state();
            let Some(mut model) = state.caps.effective_model().cloned() else {
                return Err(SessionError::UnknownOption("model".into()));
            };
            if let Some(variant) = variant
                && !state.caps.variants().is_empty()
                && !state.caps.variants().iter().any(|v| v == variant)
            {
                return Err(SessionError::UnknownOption(variant.to_string()));
            }
            model.variant = variant.map(str::to_string);
            model
        };
        self.switch_model(model)
    }
}
