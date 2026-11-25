//! What only one path offers to the user's session, as Claude's calls are reached through `Driver::claude`: the
//! sessions of Codex, OpenCode and an ACP agent for their overlays, models, modes and management, and permission
//! answers with any option a request offers.

use term_thread::EntryKind;

use super::{AnswerError, Driver};

impl Driver {
    /// The Codex session, for what only Codex offers, such as its overlays, its settings and its stored threads;
    /// `None` on the other paths.
    pub fn codex(&self) -> Option<&term_codex::Session> {
        self.backend.codex()
    }

    /// The OpenCode session, for what only OpenCode offers, such as its overlays, agents, models and reverting its
    /// stored session; `None` on the other paths.
    pub fn opencode(&self) -> Option<&term_opencode::Session> {
        self.backend.opencode()
    }

    /// The ACP session, for what only an ACP agent offers, such as its sign-in, settings and overlays; `None` on the
    /// other paths.
    pub fn acp(&self) -> Option<&term_acp::Session> {
        self.backend.acp()
    }

    /// Answers the permission request `request` with the option at `option` in its list, such as allowing a command
    /// for the whole session, or with none, which dismisses it; on Codex, OpenCode and ACP.
    pub fn answer_option(&mut self, request: &str, option: Option<usize>) -> Result<(), AnswerError> {
        let Some(EntryKind::PermissionRequest(asked)) = self.pending(request) else {
            return Err(AnswerError::NoRequest(request.into()));
        };
        self.backend.option(&asked, option).map_err(AnswerError::Path)
    }
}
