//! The status line under the composer: how full the context is, compaction, retries and the last error on the left,
//! what the session cost or how much of the plan is used on the right.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSAccessibility, NSBezierPath, NSColor, NSStackView, NSTextField, NSView};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_thread::{Activity, EntryKind, Lane, Notice, Thread};

use super::format;
use super::style;
use super::ui;
use crate::AgentKey;
use crate::common::util::{guarded, ns};

#[derive(Default)]
pub(crate) struct RingIvars {
    fraction: Cell<f64>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `ContextRing` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentContextRing"]
    #[ivars = RingIvars]
    pub(crate) struct ContextRing;

    unsafe impl NSObjectProtocol for ContextRing {}

    impl ContextRing {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || {
                let bounds = self.bounds();
                let inset = 1.5;
                let rect = NSRect::new(
                    NSPoint::new(bounds.origin.x + inset, bounds.origin.y + inset),
                    NSSize::new(bounds.size.width - 2.0 * inset, bounds.size.height - 2.0 * inset),
                );
                let track = NSBezierPath::bezierPathWithOvalInRect(rect);
                track.setLineWidth(2.0);
                NSColor::quaternaryLabelColor().setStroke();
                track.stroke();
                let fraction = self.ivars().fraction.get().clamp(0.0, 1.0);
                if fraction <= 0.0 {
                    return;
                }
                let center = NSPoint::new(rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
                let arc = NSBezierPath::bezierPath();
                arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                    center,
                    rect.size.width / 2.0,
                    90.0,
                    90.0 - 360.0 * fraction,
                    true,
                );
                arc.setLineWidth(2.0);
                let color = if fraction > 0.8 {
                    NSColor::systemOrangeColor()
                } else {
                    NSColor::secondaryLabelColor()
                };
                color.setStroke();
                arc.stroke();
            });
        }
    }
);

impl ContextRing {
    fn new(mtm: MainThreadMarker) -> Retained<ContextRing> {
        let this = Self::alloc(mtm).set_ivars(RingIvars::default());
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        let ring: Retained<ContextRing> = unsafe { msg_send![super(this), init] };
        ui::fixed_width(&ring, 13.0);
        ui::fixed_height(&ring, 13.0);
        ring
    }

    fn set(&self, fraction: f64) {
        if (self.ivars().fraction.get() - fraction).abs() > 0.001 {
            self.ivars().fraction.set(fraction);
            self.setNeedsDisplay(true);
        }
    }
}

pub(crate) struct StatusLine {
    pub(crate) view: Retained<NSStackView>,
    /// Opens the context's breakdown from the ring and its text.
    gauge: Retained<objc2_app_kit::NSButton>,
    ring: Retained<ContextRing>,
    context: Retained<NSTextField>,
    cost: Retained<NSTextField>,
    activity: Retained<NSTextField>,
    problem: Retained<NSTextField>,
    /// What dictation into the composer does.
    #[cfg(feature = "dictation")]
    pub(crate) dictation: Retained<NSTextField>,
}

impl StatusLine {
    pub(crate) fn new(mtm: MainThreadMarker, key: AgentKey) -> StatusLine {
        let ring = ContextRing::new(mtm);
        let small = |color: Retained<NSColor>| {
            let label = ui::label(mtm, "", &style::chat_small(), &color);
            ui::hug(&label);
            label
        };
        let context = small(NSColor::secondaryLabelColor());
        let cost = small(NSColor::secondaryLabelColor());
        let activity = small(NSColor::secondaryLabelColor());
        let problem = ui::label(mtm, "", &style::chat_small(), &NSColor::systemRedColor());
        let spacer = ui::spacer(mtm);
        // What the session holds and does on the left, what it costs or how much of the plan is used on the right.
        let view = ui::row(mtm, 6.0, &[&ring, &context, &activity, &problem, &spacer, &cost]);
        #[cfg(feature = "dictation")]
        let dictation = term_ui_dictation::line(mtm);
        #[cfg(feature = "dictation")]
        view.insertArrangedSubview_atIndex(&dictation, 3);
        view.setAccessibilityElement(true);
        view.setAccessibilityRole(Some(crate::common::constant!(objc2_app_kit::NSAccessibilityGroupRole)));
        view.setAccessibilityLabel(Some(&ns(t("Status"))));
        let gauge = super::context_view::gauge(mtm, key, &view, &ring, &context);
        StatusLine {
            view,
            gauge,
            ring,
            context,
            cost,
            activity,
            problem,
            #[cfg(feature = "dictation")]
            dictation,
        }
    }

    /// Takes the text size the user chose for the agent tabs.
    pub(crate) fn text_size_changed(&self) {
        for label in [&self.context, &self.cost, &self.activity, &self.problem] {
            label.setFont(Some(&style::chat_small()));
        }
    }

    /// Shows the state of `thread`.
    pub(crate) fn show(&self, thread: &Thread) {
        let usage = thread.usage();
        match (usage.context_used, usage.context_size) {
            (Some(used), Some(size)) if size > 0 => {
                let fraction = used as f64 / size as f64;
                self.ring.set(fraction);
                self.ring.setHidden(false);
                self.context.setStringValue(&ns(&tf(
                    "{} % Kontext",
                    &[&format!("{:.0}", (fraction * 100.0).min(100.0))],
                )));
                self.context.setToolTip(Some(&ns(&tf(
                    "{} von {} Tokens",
                    &[&format::tokens(used), &format::tokens(size)],
                ))));
            }
            (Some(used), None) => {
                self.ring.setHidden(true);
                self.context
                    .setStringValue(&ns(&tf("{} Tokens Kontext", &[&format::tokens(used)])));
            }
            _ => {
                self.ring.setHidden(true);
                let reported = super::context_view::token_text(usage);
                let (text, tip) = reported.map_or((String::new(), None), |(text, tip)| (text, Some(tip)));
                self.context.setStringValue(&ns(&text));
                self.context.setToolTip(tip.as_deref().map(ns).as_deref());
            }
        }
        let (cost, details) = super::context_view::spend_text(thread);
        self.cost.setStringValue(&ns(&cost));
        self.cost.setToolTip(details.as_deref().map(ns).as_deref());
        super::context_view::follow(&self.gauge, usage);
        let (activity, problem) = turn_notes(thread);
        self.activity.setStringValue(&ns(&activity));
        self.problem.setStringValue(&ns(&problem));
        let summary = [
            self.context.stringValue().to_string(),
            self.cost.stringValue().to_string(),
            activity,
            problem,
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
        let value = ns(&summary);
        // SAFETY: A string is a valid accessibility value of a group.
        unsafe { self.view.setAccessibilityValue(Some(&value)) };
    }
}

/// What the current turn is doing beyond its answer, and its last error: from the notices since the last message the
/// user sent.
fn turn_notes(thread: &Thread) -> (String, String) {
    let entries = thread.entries(&Lane::Main);
    let start = entries
        .iter()
        .rposition(|e| matches!(e.kind, EntryKind::UserMessage(_)))
        .unwrap_or(0);
    let mut activity = Vec::new();
    let mut problem = String::new();
    if thread.activity() == Some(Activity::Compacting) {
        activity.push(t("Verdichte …").to_string());
    }
    for entry in &entries[start..] {
        let EntryKind::Notice(notice) = &entry.kind else {
            continue;
        };
        match notice {
            Notice::Compaction { .. } => {
                if let Some(freed) = notice.freed_tokens() {
                    activity.push(tf("Verdichtet · {} Tokens frei", &[&format::tokens(freed)]));
                }
            }
            Notice::Retrying {
                attempt, max_attempts, ..
            } if thread.status() != term_thread::Status::Idle => {
                activity.push(tf(
                    "Wiederhole … (Versuch {} von {})",
                    &[&(attempt + 1).to_string(), &max_attempts.to_string()],
                ));
            }
            Notice::Error { .. } => problem = super::cards::summary(thread, entry),
            _ => {}
        }
    }
    (activity.join(" · "), problem)
}

#[cfg(feature = "selftest")]
mod selftest_probe;
