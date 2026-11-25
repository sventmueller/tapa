use std::path::PathBuf;

use splat_i18n::t;
use term_git::{EntryKind, FileState, StatusEntry};

use super::menu_entries;
use crate::model::{FileRow, Mark, Model, RowKey, Section};

fn row(path: &str, mark: Mark) -> FileRow {
    FileRow {
        entry: StatusEntry {
            path: PathBuf::from(path),
            kind: EntryKind::Ordinary,
            index: FileState::Modified,
            worktree: FileState::Modified,
            submodule: None,
        },
        mark,
    }
}

fn model() -> Model {
    Model {
        conflicts: vec![row("clash.rs", Mark::Conflict)],
        staged: vec![row("ready.rs", Mark::Modified)],
        changes: vec![row("draft.rs", Mark::Modified), row("other.rs", Mark::Added)],
        ..Model::default()
    }
}

fn file(section: Section, path: &str) -> RowKey {
    RowKey::File(section, PathBuf::from(path))
}

/// The menu's titles with their actions' names, a separator as `-`.
fn menu(keys: &[RowKey]) -> Vec<String> {
    menu_entries(keys, &model())
        .into_iter()
        .map(|(title, action)| match action {
            Some(action) => format!("{title} {}", action.name().to_str().unwrap()),
            None => "-".to_string(),
        })
        .collect()
}

#[test]
fn a_section_offers_its_all_action_and_the_others_offer_none() {
    assert_eq!(
        menu(&[RowKey::Section(Section::Staged)]),
        [format!("{} termGitUnstageAll:", t("Alle zurücknehmen"))]
    );
    assert_eq!(
        menu(&[RowKey::Section(Section::Changes)]),
        [format!("{} termGitStageAll:", t("Alle vormerken"))]
    );
    for section in [Section::Conflicts, Section::Stashes, Section::Commits] {
        assert!(menu(&[RowKey::Section(section)]).is_empty(), "{section:?}");
    }
}

#[test]
fn one_changed_file_can_be_diffed_staged_and_discarded() {
    assert_eq!(
        menu(&[file(Section::Changes, "draft.rs")]),
        [
            format!("{} termGitDiff:", t("Diff zeigen")),
            format!("{} termGitOpen:", t("Im Editor öffnen")),
            "-".to_string(),
            format!("{} termGitStage:", t("Vormerken")),
            "-".to_string(),
            format!("{} termGitDiscard:", t("Verwerfen …")),
        ]
    );
}

#[test]
fn a_staged_file_is_only_taken_back_and_never_discarded() {
    assert_eq!(
        menu(&[file(Section::Staged, "ready.rs")]),
        [
            format!("{} termGitDiff:", t("Diff zeigen")),
            format!("{} termGitOpen:", t("Im Editor öffnen")),
            "-".to_string(),
            format!("{} termGitUnstage:", t("Zurücknehmen")),
        ]
    );
}

#[test]
fn several_files_offer_what_fits_any_of_them_without_a_diff() {
    let keys = [
        file(Section::Staged, "ready.rs"),
        file(Section::Changes, "draft.rs"),
        file(Section::Changes, "other.rs"),
    ];
    assert_eq!(
        menu(&keys),
        [
            format!("{} termGitOpen:", t("Im Editor öffnen")),
            "-".to_string(),
            format!("{} termGitStage:", t("Vormerken")),
            format!("{} termGitUnstage:", t("Zurücknehmen")),
            "-".to_string(),
            format!("{} termGitDiscard:", t("Verwerfen …")),
        ]
    );
}

#[test]
fn conflicts_are_opened_and_marked_resolved() {
    assert_eq!(
        menu(&[file(Section::Conflicts, "clash.rs")]),
        [
            format!("{} termGitOpen:", t("Im Editor öffnen")),
            format!("{} termGitResolve:", t("Als gelöst markieren")),
        ]
    );
}

#[test]
fn commits_get_the_history_menu_and_rows_that_are_gone_get_none() {
    let commit = menu(&[RowKey::Commit(term_git::CommitId::parse(&"a".repeat(40)).unwrap())]);
    assert_eq!(
        commit.first(),
        Some(&format!("{} termGitCopyHash:", t("Commit-ID kopieren")))
    );
    assert_eq!(
        commit.last(),
        Some(&format!("{} termGitBranchHere:", t("Branch hier anlegen …")))
    );
    assert!(menu(&[file(Section::Changes, "gone.rs")]).is_empty());
    assert!(menu(&[RowKey::More]).is_empty());
}
