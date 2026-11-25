//! The editor's text view: TextKit 2, with the document model following every edit it makes.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSEvent, NSSelectionAffinity, NSTextContentManager, NSTextContentManagerDelegate,
    NSTextContentManagerEnumerationOptions, NSTextContentStorageDelegate, NSTextElement, NSTextStorage,
    NSTextStorageDelegate, NSTextStorageEditActions, NSTextView,
};
use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRange, NSValue};

use ivars::ViewIvars;

define_class!(
    // SAFETY: NSTextView has no subclassing requirements and `EditorTextView` does not implement Drop.
    #[unsafe(super(NSTextView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermEditorTextView"]
    #[ivars = ViewIvars]
    pub(crate) struct EditorTextView;

    unsafe impl NSObjectProtocol for EditorTextView {}

    unsafe impl NSTextContentManagerDelegate for EditorTextView {
        #[unsafe(method(textContentManager:shouldEnumerateTextElement:options:))]
        fn should_enumerate(
            &self,
            manager: &NSTextContentManager,
            element: &NSTextElement,
            _options: NSTextContentManagerEnumerationOptions,
        ) -> bool {
            self.guarded(true, || super::fold::shows(self, manager, element))
        }
    }

    unsafe impl NSTextContentStorageDelegate for EditorTextView {}

    unsafe impl NSTextStorageDelegate for EditorTextView {
        #[unsafe(method(textStorage:didProcessEditing:range:changeInLength:))]
        fn storage_edited(&self, storage: &NSTextStorage, mask: NSTextStorageEditActions, range: NSRange, delta: isize) {
            if mask.contains(NSTextStorageEditActions::EditedCharacters) && !self.ivars().applying.get() {
                self.guarded((), || self.follow_edit(storage, range, delta));
            }
        }
    }

    impl EditorTextView {
        #[unsafe(method(insertNewline:))]
        fn insert_newline(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::editing::newline(self));
        }

        #[unsafe(method(insertTab:))]
        fn insert_tab(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::editing::tab(self));
        }

        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, text: &AnyObject, range: NSRange) {
            let _interval = term_base::signpost::begin(term_base::signpost::name!("keystroke"));
            if self.guarded(false, || super::vim::insert_text(self, text)) {
                return;
            }
            if self.guarded(false, || super::multi::insert(self, text)) {
                return;
            }
            let handled = self.guarded(false, || super::editing::typed(self, text, range));
            if !handled {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), insertText: text, replacementRange: range] };
            }
        }

        #[unsafe(method(setSelectedRanges:affinity:stillSelecting:))]
        fn set_selected_ranges(&self, ranges: &NSArray<NSValue>, affinity: NSSelectionAffinity, still: bool) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), setSelectedRanges: ranges, affinity: affinity, stillSelecting: still] };
            if !still {
                self.guarded((), || self.selection_moved());
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.guarded(false, || super::vim::control_key(self, event)) {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            if self.guarded(false, || super::vim::control_key(self, event)) {
                true
            } else {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.guarded((), || super::vim::focus_lost(self));
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            unsafe { msg_send![super(self), resignFirstResponder] }
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            unsafe { msg_send![super(self), becomeFirstResponder] }
        }

        #[unsafe(method(paste:))]
        fn paste(&self, sender: Option<&AnyObject>) {
            if !self.guarded(false, || super::multi::paste(self) || crate::markdown::paste(self)) {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), paste: sender] };
            }
        }

        #[unsafe(method(copy:))]
        fn copy(&self, sender: Option<&AnyObject>) {
            if !self.guarded(false, || super::multi::copy(self)) {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), copy: sender] };
            }
        }

        #[unsafe(method(cut:))]
        fn cut(&self, sender: Option<&AnyObject>) {
            if !self.guarded(false, || super::multi::cut(self)) {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), cut: sender] };
            }
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, command: Sel) {
            #[cfg(feature = "dictation")]
            if (command == objc2::sel!(cancelOperation:) || command == objc2::sel!(complete:))
                && self.guarded(false, || term_ui_dictation::cancel_in(self))
            {
                return;
            }
            if self.guarded(false, || super::multi::command(self, command) || super::vim::command(self, command)) {
                return;
            }
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), doCommandBySelector: command] };
        }


        #[unsafe(method(textViewportLayoutControllerDidLayout:))]
        fn viewport_did_layout(&self, controller: &AnyObject) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), textViewportLayoutControllerDidLayout: controller] };
            self.guarded((), || {
                super::viewport::laid_out(self);
                super::overlay::laid_out(self);
                super::highlight::laid_out(self);
            });
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            self.guarded((), || super::opening::fit_width_due(self));
            // SAFETY: The overridden method without arguments, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), layout] };
        }

        #[unsafe(method(viewDidEndLiveResize))]
        fn did_end_live_resize(&self) {
            // SAFETY: The overridden method without arguments, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), viewDidEndLiveResize] };
            self.guarded((), || super::opening::resize_ended(self));
        }

        #[unsafe(method(termFitWidthDue))]
        fn fit_width_due(&self) {
            self.guarded((), || super::opening::fit_width_due(self));
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: objc2_foundation::NSSize) {
            let _interval = term_ui::kit::live::dragging(self)
                .then(|| term_base::signpost::begin(term_base::signpost::name!("resize")));
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
        }


        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<objc2_app_kit::NSMenu>> {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let menu: Option<Retained<objc2_app_kit::NSMenu>> = unsafe { msg_send![super(self), menuForEvent: event] };
            if let Some(menu) = &menu {
                self.guarded((), || super::context_menu::extend(self, menu));
            }
            menu
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let handled = self.guarded(false, || {
                let point = self.convertPoint_fromView(event.locationInWindow(), None);
                super::fold::click(self, point) || super::multi::option_mouse(self, event)
            });
            if handled {
                return;
            }
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), mouseDown: event] };
        }



        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &objc2_app_kit::NSMenuItem) -> bool {
            let answer = self.guarded(None, || {
                super::whitespace::validate(item)
                    .or_else(|| super::saving::validate(item))
                    .or_else(|| super::vim::validate(item))
                    .or_else(|| crate::markdown::validate(self, item))
            });
            match answer {
                Some(enabled) => enabled,
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                None => unsafe { msg_send![super(self), validateMenuItem: item] },
            }
        }


        // TextKit 2's own `textContainerOrigin` lays out the whole text on every call, and the caret asks for it at
        // every keystroke; for this view it is always the inset.
        #[unsafe(method(textContainerOrigin))]
        fn text_container_origin(&self) -> NSPoint {
            let inset = self.textContainerInset();
            NSPoint::new(inset.width, inset.height)
        }
    }

    impl EditorTextView {
        #[unsafe(method(termFold:))]
        fn fold_action(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::fold::fold_at_cursor(self));
        }

        #[unsafe(method(termUnfold:))]
        fn unfold_action(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::fold::unfold_at_cursor(self));
        }

        #[unsafe(method(termFoldAll:))]
        fn fold_all_action(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::fold::fold_all(self));
        }

        #[unsafe(method(termUnfoldAll:))]
        fn unfold_all_action(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::fold::unfold_all(self));
        }

        #[unsafe(method(termAddCaretAbove:))]
        fn add_caret_above(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::multi::add_vertical(self, true));
        }

        #[unsafe(method(termAddCaretBelow:))]
        fn add_caret_below(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::multi::add_vertical(self, false));
        }

        #[unsafe(method(termAddNextOccurrence:))]
        fn add_next_occurrence(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::multi::add_next_occurrence(self));
        }

        #[unsafe(method(termSaveAs:))]
        fn save_as_action(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::save_soon(self, true));
        }

        #[unsafe(method(termToggleComment:))]
        fn toggle_comment(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::editing::toggle_comment(self));
        }

        #[unsafe(method(termGoToLine:))]
        fn go_to_line(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::editing::go_to_line(self));
        }

        #[unsafe(method(termMarkdown:))]
        fn markdown(&self, sender: Option<&AnyObject>) {
            let tag = sender
                .and_then(|s| s.downcast_ref::<objc2_app_kit::NSMenuItem>())
                .map_or(0, |item| item.tag());
            self.guarded((), || crate::markdown::perform(self, tag));
        }


        #[unsafe(method(termRevertHunk:))]
        fn revert_hunk(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::changes::revert(self));
        }




        #[unsafe(method(termIndentMenu:))]
        fn indent_menu(&self, sender: Option<&AnyObject>) {
            self.guarded((), || super::indent::show_menu(self, sender));
        }

        #[unsafe(method(termSetIndent:))]
        fn set_indent(&self, sender: Option<&AnyObject>) {
            self.guarded((), || {
                let tag = sender
                    .and_then(|s| s.downcast_ref::<objc2_app_kit::NSMenuItem>())
                    .map_or(0, |item| item.tag());
                super::indent::choose(self, tag);
            });
        }

    }

    impl EditorTextView {




        #[unsafe(method(termSave:))]
        fn save_action(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || crate::save_soon(self, false));
        }
























    }

    impl EditorTextView {










    }

    impl EditorTextView {
        #[unsafe(method(termConflictDiff:))]
        fn conflict_diff(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::show_differences(self));
        }

        #[unsafe(method(termConflictMerge:))]
        fn conflict_merge(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::merge(self));
        }

        #[unsafe(method(termConflictLoad:))]
        fn conflict_load(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::load(self));
        }

        #[unsafe(method(termConflictKeep:))]
        fn conflict_keep(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::keep(self));
        }

        #[unsafe(method(termConflictSave:))]
        fn conflict_save(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::save(self));
        }

        #[unsafe(method(termConflictClose:))]
        fn conflict_close(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::close(self));
        }

        #[unsafe(method(termConflictNext:))]
        fn conflict_next(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::go_to_conflict(self, true));
        }

        #[unsafe(method(termConflictPrevious:))]
        fn conflict_previous(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::conflict::go_to_conflict(self, false));
        }

        #[unsafe(method(termConflictRecount))]
        fn conflict_recount(&self) {
            self.guarded((), || super::conflict::recount(self));
        }

        #[unsafe(method(termGitMarks))]
        fn git_marks(&self) {
            self.guarded((), || super::changes::compare(self));
        }

        #[unsafe(method(termGitHead))]
        fn git_head(&self) {
            self.guarded((), || super::changes::reload(self, false));
        }
    }

    impl EditorTextView {
        #[unsafe(method(termRelayoutFolds))]
        fn relayout_folds(&self) {
            self.guarded((), || super::fold::relayout(self));
        }

        #[unsafe(method(termToggleWrap:))]
        fn toggle_wrap(&self, _sender: Option<&AnyObject>) {
            self.guarded((), || super::toggle_wrap(self));
        }

        #[unsafe(method(termResync))]
        fn resync(&self) {
            // Not the editor's own guard: a panic here must not ask for the resync again.
            crate::util::guarded((), || self.resync_due());
        }

        #[unsafe(method(termLayOutViewport))]
        fn lay_out_viewport(&self) {
            self.guarded((), || {
                if let Some(layout) = self.textLayoutManager() {
                    layout.textViewportLayoutController().layoutViewport();
                }
            });
        }

        #[unsafe(method(termColorStep))]
        fn color_step(&self) {
            self.guarded((), || super::highlight::step(self));
        }

        #[unsafe(method(termToggleVimMode:))]
        fn toggle_vim(&self, _sender: Option<&AnyObject>) {
            self.guarded((), super::vim::toggle);
        }

        #[unsafe(method(termVimClose))]
        fn vim_close(&self) {
            self.guarded((), || super::vim::close_due(self));
        }





        #[unsafe(method(termWrapLongLines))]
        fn wrap_long_lines(&self) {
            self.guarded((), || super::long_lines::wrap(self));
        }





        #[unsafe(method(termToggleWhitespace:))]
        fn toggle_whitespace(&self, _sender: Option<&AnyObject>) {
            self.guarded((), super::whitespace::toggle);
        }
    }
);

mod found;
mod ivars;
mod selection;
mod sync;
mod system;
