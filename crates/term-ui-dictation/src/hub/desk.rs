//! What dictation does next, decided without AppKit: which field records, which waits for the microphone, and what
//! each of lowspeak's events means for the fields. A dictation belongs to the field it started in, whatever has the
//! focus when its text arrives.

use std::collections::HashMap;

use lowspeak::dictation::{Event, Failure, Nothing, Target};

/// How far the recording of a field has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    /// Lowspeak was asked to open the microphone.
    Opening,
    /// The microphone is open.
    Recording,
    /// Sound arrived.
    Listening,
}

/// What the hub carries out for the desk.
#[derive(Debug)]
pub(super) enum Act {
    Start(Target),
    Stop(Target),
    /// Asks lowspeak again in a moment for a start it refused while the microphone was still busy.
    Retry(Target),
    Insert(Target, String),
    Failed(Target, Failure),
    NoSpeech(Target),
    LimitReached(Target),
    /// The dictation of the field ended, with text or without.
    Ended(Target),
    /// The field's controls show its state anew.
    Show(Target),
}

#[derive(Debug, Default)]
pub(super) struct Desk {
    recording: Option<(Target, Phase)>,
    /// The field whose start waits until the microphone is free.
    pending: Option<Target>,
    transcribing: HashMap<Target, usize>,
}

impl Desk {
    pub(super) fn phase(&self, target: Target) -> Phase {
        match self.recording {
            Some((t, phase)) if t == target => phase,
            _ => Phase::Idle,
        }
    }

    /// The field that records, or waits to.
    pub(super) fn recording(&self) -> Option<Target> {
        self.recording.map(|(t, _)| t).or(self.pending)
    }

    pub(super) fn is_waiting(&self, target: Target) -> bool {
        self.pending == Some(target)
    }

    /// How many recordings of the field are being transcribed.
    pub(super) fn transcribing(&self, target: Target) -> usize {
        self.transcribing.get(&target).copied().unwrap_or(0)
    }

    /// Whether anything of the field runs.
    pub(super) fn is_busy(&self, target: Target) -> bool {
        self.phase(target) != Phase::Idle || self.is_waiting(target) || self.transcribing(target) > 0
    }

    /// The user asked to dictate into `target`, or to end its dictation; a recording of another field ends first and
    /// is still transcribed for that field.
    pub(super) fn toggle(&mut self, target: Target) -> Vec<Act> {
        if self.pending == Some(target) {
            self.pending = None;
            return vec![Act::Show(target)];
        }
        match self.recording {
            Some((t, _)) if t == target => vec![Act::Stop(target)],
            Some((other, _)) => {
                let mut acts = vec![Act::Stop(other)];
                if let Some(waiting) = self.pending.take() {
                    acts.push(Act::Show(waiting));
                }
                acts.push(Act::Start(target));
                acts
            }
            None => {
                let mut acts = Vec::new();
                if let Some(waiting) = self.pending.take() {
                    acts.push(Act::Show(waiting));
                }
                acts.push(Act::Start(target));
                acts
            }
        }
    }

    /// Lowspeak's answer to the start of `target`: taken, or refused while the microphone is still busy.
    pub(super) fn started(&mut self, target: Target, taken: bool) -> Vec<Act> {
        if taken {
            self.pending = self.pending.filter(|t| *t != target);
            let replaced = self.recording.replace((target, Phase::Opening));
            let mut acts = vec![Act::Show(target)];
            if let Some((other, _)) = replaced.filter(|(t, _)| *t != target) {
                acts.push(Act::Show(other));
            }
            return acts;
        }
        self.pending = Some(target);
        vec![Act::Show(target), Act::Retry(target)]
    }

    /// A retry of the waiting start is due; whether it still waits.
    pub(super) fn retry_due(&self, target: Target) -> bool {
        self.pending == Some(target)
    }

    /// The user cancelled the dictation of `target`; whether one ran.
    pub(super) fn cancel(&mut self, target: Target) -> bool {
        let mut ran = false;
        if self.recording.is_some_and(|(t, _)| t == target) {
            self.recording = None;
            ran = true;
        }
        if self.pending == Some(target) {
            self.pending = None;
            ran = true;
        }
        ran |= self.transcribing.remove(&target).is_some_and(|n| n > 0);
        ran
    }

    pub(super) fn event(&mut self, event: Event) -> Vec<Act> {
        match event {
            Event::Started(t) => self.advance(t, Phase::Opening, Phase::Recording),
            Event::Live(t) => {
                let mut acts = self.advance(t, Phase::Opening, Phase::Listening);
                acts.extend(self.advance(t, Phase::Recording, Phase::Listening));
                acts
            }
            Event::LimitReached(t) => vec![Act::LimitReached(t)],
            Event::Transcribing(t) => {
                *self.transcribing.entry(t).or_default() += 1;
                let mut acts = vec![Act::Show(t)];
                acts.extend(self.recording_ended(t));
                acts
            }
            Event::Text(t, text) => self.finished(t, Act::Insert(t, text)),
            Event::Nothing(t, Nothing::NoSpeech | Nothing::TooShort) => self.finished(t, Act::NoSpeech(t)),
            Event::Failed(t, failure @ Failure::Microphone(_)) => {
                if self.recording.is_none_or(|(r, _)| r != t) {
                    return Vec::new();
                }
                let mut acts = vec![Act::Failed(t, failure), Act::Ended(t), Act::Show(t)];
                acts.extend(self.recording_ended(t));
                acts
            }
            Event::Failed(t, failure) => self.finished(t, Act::Failed(t, failure)),
        }
    }

    fn advance(&mut self, target: Target, from: Phase, to: Phase) -> Vec<Act> {
        match &mut self.recording {
            Some((t, phase)) if *t == target && *phase == from => {
                *phase = to;
                vec![Act::Show(target)]
            }
            _ => Vec::new(),
        }
    }

    /// The recording of `target` ended; a waiting start may go now.
    fn recording_ended(&mut self, target: Target) -> Vec<Act> {
        if self.recording.is_some_and(|(t, _)| t == target) {
            self.recording = None;
        }
        match self.pending.take() {
            Some(waiting) if self.recording.is_none() => vec![Act::Start(waiting)],
            waiting => {
                self.pending = waiting;
                Vec::new()
            }
        }
    }

    /// A transcription of `target` ended with `act`; nothing if the user cancelled it meanwhile.
    fn finished(&mut self, target: Target, act: Act) -> Vec<Act> {
        let Some(count) = self.transcribing.get_mut(&target).filter(|n| **n > 0) else {
            return Vec::new();
        };
        *count -= 1;
        if *count == 0 {
            self.transcribing.remove(&target);
        }
        vec![act, Act::Ended(target), Act::Show(target)]
    }
}

#[cfg(test)]
mod tests {
    use lowspeak::audio::recorder::StartError;
    use lowspeak::transcription::error::TranscriptionError;

    use super::*;

    const A: Target = 1;
    const B: Target = 2;

    fn names(acts: &[Act]) -> Vec<String> {
        acts.iter()
            .filter(|a| !matches!(a, Act::Show(_)))
            .map(|a| match a {
                Act::Start(t) => format!("start {t}"),
                Act::Stop(t) => format!("stop {t}"),
                Act::Retry(t) => format!("retry {t}"),
                Act::Insert(t, text) => format!("insert {t}: {text}"),
                Act::Failed(t, _) => format!("failed {t}"),
                Act::NoSpeech(t) => format!("no speech {t}"),
                Act::LimitReached(t) => format!("limit {t}"),
                Act::Ended(t) => format!("ended {t}"),
                Act::Show(t) => format!("show {t}"),
            })
            .collect()
    }

    fn recording(desk: &mut Desk, target: Target) {
        assert_eq!(names(&desk.toggle(target)), [format!("start {target}")]);
        desk.started(target, true);
        desk.event(Event::Started(target));
        desk.event(Event::Live(target));
        assert_eq!(desk.phase(target), Phase::Listening);
    }

    #[test]
    fn the_text_goes_to_the_field_the_recording_started_in() {
        let mut desk = Desk::default();
        recording(&mut desk, A);
        assert_eq!(names(&desk.toggle(A)), ["stop 1"]);
        desk.event(Event::Transcribing(A));
        assert_eq!(desk.phase(A), Phase::Idle);
        assert_eq!(desk.transcribing(A), 1);
        recording(&mut desk, B);
        assert_eq!(
            names(&desk.event(Event::Text(A, "hello".into()))),
            ["insert 1: hello", "ended 1"],
            "a later recording elsewhere does not move it"
        );
        assert!(!desk.is_busy(A));
        assert!(desk.is_busy(B));
    }

    #[test]
    fn starting_while_another_field_records_stops_that_one_first() {
        let mut desk = Desk::default();
        recording(&mut desk, A);
        assert_eq!(names(&desk.toggle(B)), ["stop 1", "start 2"]);
        desk.started(B, true);
        assert_eq!(desk.recording(), Some(B));
        assert!(names(&desk.event(Event::Transcribing(A))).is_empty());
        assert_eq!(
            desk.recording(),
            Some(B),
            "the end of the earlier one leaves the new one alone"
        );
        assert_eq!(desk.transcribing(A), 1);
    }

    #[test]
    fn a_start_refused_while_the_microphone_opens_waits_for_it() {
        let mut desk = Desk::default();
        assert_eq!(names(&desk.toggle(A)), ["start 1"]);
        desk.started(A, true);
        assert_eq!(names(&desk.toggle(B)), ["stop 1", "start 2"]);
        assert_eq!(names(&desk.started(B, false)), ["retry 2"]);
        assert!(desk.is_waiting(B));
        assert!(desk.retry_due(B));
        desk.event(Event::Started(A));
        assert_eq!(
            names(&desk.event(Event::Transcribing(A))),
            ["start 2"],
            "the waiting start goes once the microphone is free"
        );
        desk.started(B, true);
        assert!(!desk.retry_due(B));
        assert_eq!(desk.phase(B), Phase::Opening);
    }

    #[test]
    fn a_second_press_on_a_waiting_field_gives_up_its_start() {
        let mut desk = Desk::default();
        desk.toggle(A);
        desk.started(A, false);
        assert!(names(&desk.toggle(A)).is_empty());
        assert!(!desk.retry_due(A));
        assert!(!desk.is_busy(A));
    }

    #[test]
    fn cancelling_drops_the_recording_and_texts_still_on_their_way() {
        let mut desk = Desk::default();
        recording(&mut desk, A);
        desk.toggle(A);
        desk.event(Event::Transcribing(A));
        assert!(desk.cancel(A));
        assert!(
            names(&desk.event(Event::Text(A, "late".into()))).is_empty(),
            "a text already sent before the cancel is not inserted"
        );
        assert!(!desk.cancel(A), "nothing runs any more");
        recording(&mut desk, B);
        assert!(desk.cancel(B));
        assert_eq!(desk.recording(), None);
    }

    #[test]
    fn a_failing_microphone_ends_the_recording_and_frees_a_waiting_start() {
        let mut desk = Desk::default();
        desk.toggle(A);
        desk.started(A, true);
        desk.toggle(B);
        desk.started(B, false);
        assert_eq!(
            names(&desk.event(Event::Failed(A, Failure::Microphone(StartError::NoMicrophone)))),
            ["failed 1", "ended 1", "start 2"]
        );
        assert_eq!(desk.phase(A), Phase::Idle);
    }

    #[test]
    fn transcriptions_end_with_text_nothing_or_a_failure_in_their_order() {
        let mut desk = Desk::default();
        for _ in 0..3 {
            recording(&mut desk, A);
            desk.toggle(A);
            desk.event(Event::Transcribing(A));
        }
        assert_eq!(desk.transcribing(A), 3);
        assert_eq!(
            names(&desk.event(Event::Nothing(A, Nothing::TooShort))),
            ["no speech 1", "ended 1"]
        );
        assert_eq!(
            names(&desk.event(Event::Nothing(A, Nothing::NoSpeech))),
            ["no speech 1", "ended 1"]
        );
        assert_eq!(
            names(&desk.event(Event::Failed(
                A,
                Failure::Transcription(TranscriptionError::InvalidResponse)
            ))),
            ["failed 1", "ended 1"]
        );
        assert!(!desk.is_busy(A));
    }

    #[test]
    fn sound_before_the_start_report_still_counts() {
        let mut desk = Desk::default();
        desk.toggle(A);
        desk.started(A, true);
        desk.event(Event::Live(A));
        assert_eq!(desk.phase(A), Phase::Listening);
        desk.event(Event::Started(A));
        assert_eq!(desk.phase(A), Phase::Listening, "the start report does not step back");
    }
}
