//! The Objective-C side of an agent tab: the timeline's data source and delegate, the composer's delegate, and the
//! target of every control in the tab. Each call finds the tab by its key and hands over.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSMenuItem, NSTableColumn, NSTableRowView, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTextDelegate, NSTextView, NSTextViewDelegate, NSView,
};
use objc2_foundation::{NSNotification, NSObjectProtocol};
use objc2_quartz_core::CADisplayLink;

use super::cards::{Part, unpack};
use super::tab::AgentTab;
use crate::AgentKey;
use crate::common::util::guarded;

pub(crate) struct TargetIvars {
    key: AgentKey,
}

/// The tag of the control or menu item that sent an action.
pub(crate) fn tag(sender: Option<&AnyObject>) -> isize {
    let Some(sender) = sender else { return -1 };
    if let Some(item) = sender.downcast_ref::<NSMenuItem>() {
        return item.tag();
    }
    sender.downcast_ref::<NSControl>().map_or(-1, |control| control.tag())
}

fn row(row: isize) -> usize {
    usize::try_from(row).unwrap_or(usize::MAX)
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AgentTarget` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentTarget"]
    #[ivars = TargetIvars]
    pub(crate) struct AgentTarget;

    unsafe impl NSObjectProtocol for AgentTarget {}

    unsafe impl NSControlTextEditingDelegate for AgentTarget {}

    unsafe impl NSTableViewDataSource for AgentTarget {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn rows(&self, _table: &NSTableView) -> isize {
            guarded(0, || {
                self.tab().map_or(0, |tab| isize::try_from(tab.timeline().table_rows()).unwrap_or(0))
            })
        }
    }

    unsafe impl NSTableViewDelegate for AgentTarget {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _table: &NSTableView, _column: Option<&NSTableColumn>, index: isize) -> Option<Retained<NSView>> {
            guarded(None, || self.tab()?.timeline().cell(row(index)))
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height_of(&self, _table: &NSTableView, index: isize) -> f64 {
            guarded(20.0, || self.tab().map_or(20.0, |tab| tab.timeline().row_height(row(index))))
        }

        #[unsafe(method_id(tableView:rowViewForRow:))]
        fn row_view(&self, _table: &NSTableView, index: isize) -> Option<Retained<NSTableRowView>> {
            guarded(None, || Some(self.tab()?.timeline().row_view(row(index))))
        }
    }

    unsafe impl NSTextDelegate for AgentTarget {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _notification: &NSNotification) {
            guarded((), || {
                if let Some(tab) = self.tab() {
                    tab.composer_changed();
                }
            });
        }
    }

    unsafe impl NSTextViewDelegate for AgentTarget {
        #[unsafe(method(textView:doCommandBySelector:))]
        fn do_command(&self, _view: &NSTextView, command: Sel) -> bool {
            guarded(false, || self.tab().is_some_and(|tab| tab.composer_command(command)))
        }
    }

    impl AgentTarget {
        #[unsafe(method(ideAgentGlide:))]
        fn glide(&self, link: &CADisplayLink) {
            guarded((), || {
                if let Some(tab) = self.tab() {
                    tab.timeline().glide_frame(link);
                }
            });
        }

        #[unsafe(method(ideAgentToggle:))]
        fn toggle(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, what| {
                if let Some(part) = Part::from_tag(what) {
                    tab.timeline().toggle(id, part);
                }
            });
        }



        #[unsafe(method(ideAgentPermission:))]
        fn permission(&self, sender: Option<&AnyObject>) {
            let view = sender.and_then(|s| s.downcast_ref::<NSView>()).map(NSView::retain);
            self.on_entry(sender, |tab, id, what| tab.permission(id, what, view.as_deref()));
        }

        #[unsafe(method(ideAgentAllowRule:))]
        fn allow_rule(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, what| tab.allow_rule(id, usize::from(what)));
        }

        #[unsafe(method(ideAgentAnswer:))]
        fn answer(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, what| tab.answer(id, what >> 4, what & 0xf));
        }

        #[unsafe(method(ideAgentAnswerOther:))]
        fn answer_other(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, what| tab.answer_other(id, what));
        }

        #[unsafe(method(ideAgentPlan:))]
        fn plan(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, what| tab.plan(id, what));
        }








        #[unsafe(method(ideAgentStopTask:))]
        fn stop_task(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, _| tab.stop_task_of(id));
        }

        #[unsafe(method(ideAgentBackground:))]
        fn background(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, _| tab.move_to_background(id));
        }

        #[unsafe(method(ideAgentShowOutput:))]
        fn show_output(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, _| tab.show_output_of(id));
        }

        #[unsafe(method(ideAgentOpenPath:))]
        fn open_path(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, _| tab.open_path_of(id));
        }

        #[unsafe(method(ideAgentSignIn:))]
        fn sign_in(&self, sender: Option<&AnyObject>) {
            let index = usize::try_from(tag(sender)).unwrap_or(0);
            self.on_tab(|tab| tab.sign_in(index));
        }


        #[unsafe(method(ideAgentSend:))]
        fn send(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::send_or_stop);
        }

        #[unsafe(method(ideAgentAttach:))]
        fn attach(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::attach);
        }

        #[unsafe(method(ideAgentSlash:))]
        fn slash(&self, sender: Option<&AnyObject>) {
            self.on_menu(sender, AgentTab::slash_menu);
        }

        #[unsafe(method(ideAgentPickCommand:))]
        fn pick_command(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::pick_command);
        }

        #[unsafe(method(ideAgentModelMenu:))]
        fn model_menu(&self, sender: Option<&AnyObject>) {
            self.on_menu(sender, AgentTab::model_menu);
        }

        #[unsafe(method(ideAgentPickModel:))]
        fn pick_model(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::pick_model);
        }

        #[unsafe(method(ideAgentPickEffort:))]
        fn pick_effort(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::pick_effort);
        }

        #[unsafe(method(ideAgentModeMenu:))]
        fn mode_menu(&self, sender: Option<&AnyObject>) {
            self.on_menu(sender, AgentTab::mode_menu);
        }

        #[unsafe(method(ideAgentPickMode:))]
        fn pick_mode(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::pick_mode);
        }

        #[unsafe(method(ideAgentSandbox:))]
        fn sandbox(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::toggle_sandbox);
        }

        #[unsafe(method(ideAgentRemoveAttachment:))]
        fn remove_attachment(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::remove_attachment);
        }

        #[unsafe(method(ideAgentRemoveQueued:))]
        fn remove_queued(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::remove_queued);
        }

        #[unsafe(method(ideAgentBookmark:))]
        fn bookmark(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::toggle_bookmark);
        }

        #[unsafe(method(ideAgentHistory:))]
        fn history(&self, sender: Option<&AnyObject>) {
            self.on_menu(sender, AgentTab::history_menu);
        }

        #[unsafe(method(ideAgentOpenPast:))]
        fn open_past(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::open_past);
        }

        #[unsafe(method(ideAgentNewSession:))]
        fn new_session(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::new_session);
        }

        #[unsafe(method(ideAgentRename:))]
        fn rename(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::rename);
        }

        #[unsafe(method(ideAgentSuggest:))]
        fn suggest(&self, sender: Option<&AnyObject>) {
            self.on_index(sender, AgentTab::pick_suggestion);
        }

        #[unsafe(method(ideAgentUseSelection:))]
        fn use_selection(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::use_selection);
        }

        #[unsafe(method(ideAgentSubagentList:))]
        fn subagent_list(&self, sender: Option<&AnyObject>) {
            let view = sender.and_then(|s| s.downcast_ref::<NSView>()).map(NSView::retain);
            self.on_tab(|tab| tab.show_work(crate::work::Part::Subagents, view.as_deref()));
        }

        #[unsafe(method(ideAgentBackgroundList:))]
        fn background_list(&self, sender: Option<&AnyObject>) {
            let view = sender.and_then(|s| s.downcast_ref::<NSView>()).map(NSView::retain);
            self.on_tab(|tab| tab.show_work(crate::work::Part::Background, view.as_deref()));
        }

        #[unsafe(method(ideAgentRemoteControl:))]
        fn remote_control(&self, sender: Option<&AnyObject>) {
            self.on_menu(sender, AgentTab::remote_control);
        }

        #[unsafe(method(ideAgentRemoteControlAction:))]
        fn remote_control_action(&self, sender: Option<&AnyObject>) {
            let action = tag(sender);
            self.on_tab(|tab| tab.remote_control_action(action));
        }

        #[unsafe(method(ideAgentFocusView:))]
        fn focus_view(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::toggle_focus_view);
        }

        #[unsafe(method(ideAgentCopy:))]
        fn copy(&self, sender: Option<&AnyObject>) {
            self.on_entry(sender, |tab, id, _| tab.copy_entry(id));
            if let Some(button) = sender.and_then(|s| s.downcast_ref::<objc2_app_kit::NSButton>()) {
                crate::common::util::acknowledge(button, "checkmark", splat_i18n::t("Kopiert"));
            }
        }

        #[unsafe(method(ideAgentTrust:))]
        fn trust(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::trust_workspace);
        }

        #[unsafe(method(ideAgentRetry:))]
        fn retry(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::retry);
        }

        #[unsafe(method(ideAgentInstall:))]
        fn install(&self, _sender: Option<&AnyObject>) {
            self.on_tab(AgentTab::install_agent);
        }

    }
);

impl AgentTarget {
    pub(crate) fn new(mtm: MainThreadMarker, key: AgentKey) -> Retained<AgentTarget> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { key });
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    fn tab(&self) -> Option<std::rc::Rc<AgentTab>> {
        super::get(self.ivars().key)
    }

    fn on_tab(&self, f: impl FnOnce(&AgentTab)) {
        guarded((), || {
            if let Some(tab) = self.tab() {
                f(&tab);
            }
        });
    }

    fn on_entry(&self, sender: Option<&AnyObject>, f: impl FnOnce(&AgentTab, u64, u8)) {
        guarded((), || {
            if let (Some(tab), Some((id, what))) = (self.tab(), unpack(tag(sender))) {
                f(&tab, id, what);
            }
        });
    }

    fn on_index(&self, sender: Option<&AnyObject>, f: impl FnOnce(&AgentTab, usize)) {
        guarded((), || {
            if let (Some(tab), Ok(index)) = (self.tab(), usize::try_from(tag(sender))) {
                f(&tab, index);
            }
        });
    }

    fn on_menu(&self, sender: Option<&AnyObject>, f: impl FnOnce(&AgentTab, &NSView)) {
        guarded((), || {
            let view = sender.and_then(|s| s.downcast_ref::<NSView>());
            if let (Some(tab), Some(view)) = (self.tab(), view) {
                f(&tab, view);
            }
        });
    }
}
