# Accessibility

The terminal is a text area for VoiceOver: it reads lines, the selection and the cursor's line, and announces new output while the terminal has the keyboard, at most once in a short while so a fast program does not flood the speech. The sidebar, the tabs, the toolbar buttons, the controls of the settings and the cards of the agent timeline carry names. Every menu command has a shortcut, and all shortcuts can be changed in **Settings › Shortcuts**. Details are under [Appearance and accessibility](README.md#appearance-and-accessibility).

- **Sizes.** Four font sizes under **Settings › General** scale the interface, the editor, the terminal and the agents independently; ⌘+ and ⌘− change the terminal in front.
- **Contrast.** A minimum contrast keeps terminal text readable whatever the theme. With Increase Contrast, panels and diffs draw stronger borders and colours.
- **Reduce Motion.** The quick terminal appears without sliding, the bell does not flash the screen, and the agent timeline jumps instead of scrolling.
- **Status in words.** The status of an agent is read out with the tab and the workspace's row as text, such as "Waiting for you", not only as the coloured dot.

Known limits: VoiceOver and Full Keyboard Access have been checked with the self-test's reading of the accessibility tree, not yet end to end by a person. The gutter beside a failed command is red, and only its status over the block under the pointer names the exit code; the marks in the scroll bar for prompts, matches and errors are not announced. Inline pictures in a Markdown preview may be announced only by their alternative text.

If something in Tapa is a barrier for you, [report it as a problem](https://github.com/sventmueller/tapa/issues/new/choose).
