use serde::{Deserialize, Serialize};

use crate::{
    Res,
    app::{App, State},
    item_data::{ItemData, Rev},
    menu::Menu,
    rebase_todo::TodoAction,
    term::Term,
};
use std::{fmt::Display, rc::Rc};

pub(crate) mod apply;
pub(crate) mod blame;
pub(crate) mod branch;
pub(crate) mod cherry_pick;
pub(crate) mod command;
pub(crate) mod commit;
pub(crate) mod copy_hash;
pub(crate) mod discard;
pub(crate) mod editor;
pub(crate) mod fetch;
pub(crate) mod log;
pub(crate) mod merge;
pub(crate) mod pr;
pub(crate) mod pull;
pub(crate) mod push;
pub(crate) mod rebase;
pub(crate) mod rebase_todo;
pub(crate) mod remote;
pub(crate) mod reset;
pub(crate) mod reverse;
pub(crate) mod revert;
pub(crate) mod search;
pub(crate) mod show;
pub(crate) mod show_bindings;
pub(crate) mod show_refs;
pub(crate) mod stage;
pub(crate) mod stash;
pub(crate) mod status;
pub(crate) mod unstage;

pub(crate) type Action = Rc<dyn FnMut(&mut App, &mut Term) -> Res<()>>;

pub(crate) trait OpTrait {
    /// Get the implementation (which may or may not exist) of the Op given some TargetData.
    /// This indirection allows Gitu to show a contextual menu of applicable actions.
    fn get_action(&self, target: &ItemData) -> Option<Action>;

    /// This indicates whether the Op is meant to read and
    /// act on TargetData. Those are listed differently in the help menu.
    fn is_target_op(&self) -> bool {
        false
    }

    fn display(&self, state: &State) -> String;
}

#[derive(Clone, PartialOrd, Ord, PartialEq, Eq, Debug, Serialize, Deserialize, strum::AsRefStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Op {
    AddRemote,
    Checkout,
    CheckoutNewBranch,
    Spinoff,
    Delete,
    Commit,
    CommitAmend,
    CommitExtend,
    FetchAll,
    FetchPushRemote,
    FetchUpstream,
    FetchElsewhere,
    LogCurrent,
    PullFromPushRemote,
    PullFromUpstream,
    PullFromElsewhere,
    PushToPushRemote,
    PushToUpstream,
    PushToElsewhere,
    PrCurrent,
    PrOther,
    ToggleViewed,
    ShowOnGithub,
    RebaseAbort,
    RebaseContinue,
    RebaseElsewhere,
    RemoveRemote,
    Rename,
    RenameRemote,
    ShowRefs,
    ShowBindings,
    Status,
    Stash,
    StashApply,
    StashIndex,
    StashWorktree,
    StashKeepIndex,
    StashPop,
    StashDrop,
    CommitFixup,
    CommitInstantFixup,
    LogOther,
    RebaseAutosquash,
    RebaseInteractive,
    RebaseTodoMoveUp,
    RebaseTodoMoveDown,
    RebaseTodoPick,
    RebaseTodoReword,
    RebaseTodoEdit,
    RebaseTodoSquash,
    RebaseTodoFixup,
    RebaseTodoDrop,
    RebaseTodoStart,
    RendererFeatures,
    RendererThemes,
    SelectCommit,
    ToggleMenu,
    ResetSoft,
    ResetMixed,
    ResetHard,
    RevertAbort,
    RevertContinue,
    RevertCommit,
    CherryPickAbort,
    CherryPickContinue,
    CherryPick,
    Merge,
    MergeAbort,
    MergeContinue,

    Stage,
    Unstage,
    Show,
    ShowOrUnfold,
    ShowInAltEditor,
    Discard,
    Apply,
    Reverse,
    CopyHash,
    Blame,

    ToggleSection,
    ToggleAllSections,
    DiffContext,
    WiderDiffContext,
    NarrowerDiffContext,
    EditGitCommand,
    FilePatterns,
    HideFile,
    MoveUpLine,
    MoveDownLine,
    MoveUp,
    MoveDown,
    ExtendSelectionUp,
    ExtendSelectionDown,
    MovePrevSection,
    MoveNextSection,
    MoveParentSection,
    MoveTop,
    MoveBottom,
    HalfPageUp,
    HalfPageDown,
    FullPageUp,
    FullPageDown,
    ScrollViewUp,
    ScrollViewDown,

    Search,
    SearchBackward,
    SearchNext,
    SearchPrevious,

    Refresh,
    Suspend,
    Quit,
    FoldOrBack,

    #[serde(untagged)]
    OpenMenu(Menu),
    #[serde(untagged)]
    ToggleArg(String),
    #[serde(untagged)]
    MoveToScreenLine(usize),
}

impl Op {
    /// The op's name in config, as in `root.move_down`.
    pub(crate) fn config_name(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Op::OpenMenu(menu) => menu.as_ref().into(),
            Op::ToggleArg(arg) => arg.as_str().into(),
            op => op.as_ref().into(),
        }
    }

    /// What the op does, for the list of bindings.
    pub(crate) fn description(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Op::AddRemote => "Add a remote",
            Op::Checkout => "Check out a branch",
            Op::CheckoutNewBranch => "Create a branch and check it out",
            Op::Spinoff => "Move the commits not on the upstream to a new branch",
            Op::Delete => "Delete a branch",
            Op::Commit => "Commit",
            Op::CommitAmend => "Amend the last commit",
            Op::CommitExtend => "Add what is staged to the last commit, keeping its message",
            Op::CommitFixup => "Commit what is staged as a fixup of a commit chosen from the log",
            Op::CommitInstantFixup => {
                "Fix up a commit chosen from the log with what is staged, and rebase"
            }
            Op::FetchAll => "Fetch from all remotes",
            Op::FetchPushRemote => "Fetch from the push remote",
            Op::FetchUpstream => "Fetch from the upstream",
            Op::FetchElsewhere => "Fetch from a remote you choose",
            Op::LogCurrent => "Log of the current branch",
            Op::LogOther => "Log of a branch or commit you choose",
            Op::PullFromPushRemote => "Pull from the push remote",
            Op::PullFromUpstream => "Pull from the upstream",
            Op::PullFromElsewhere => "Pull from a branch you choose",
            Op::PushToPushRemote => "Push to the push remote",
            Op::PushToUpstream => "Push to the upstream",
            Op::PushToElsewhere => "Push to a branch you choose",
            Op::PrCurrent => "Check out the current branch's GitHub PR and review it",
            Op::PrOther => "Check out a GitHub PR you choose by number or URL and review it",
            Op::ToggleViewed => "Tick or untick the PR file's Viewed checkbox on GitHub",
            Op::ShowOnGithub => "Open the PR file, hunk, line or comment on GitHub, to comment there",
            Op::RebaseAbort => "Abort the rebase",
            Op::RebaseContinue => "Continue the rebase",
            Op::RebaseElsewhere => "Rebase onto a branch or commit you choose",
            Op::RebaseAutosquash => "Rebase from the commit under the cursor, squashing in fixup commits",
            Op::RebaseInteractive => "Rebase interactively from the commit under the cursor",
            Op::RemoveRemote => "Remove a remote",
            Op::Rename => "Rename a branch",
            Op::RenameRemote => "Rename a remote",
            Op::ShowRefs => "List branches and tags",
            Op::ShowBindings => "List every key binding",
            Op::Status => "Status view, from any view",
            Op::Stash => "Stash the index and worktree",
            Op::StashApply => "Apply a stash",
            Op::StashIndex => "Stash the index",
            Op::StashWorktree => "Stash the worktree",
            Op::StashKeepIndex => "Stash the index and worktree, keeping the index",
            Op::StashPop => "Pop a stash",
            Op::StashDrop => "Drop a stash",
            Op::RebaseTodoMoveUp => "Move the commit up",
            Op::RebaseTodoMoveDown => "Move the commit down",
            Op::RebaseTodoPick => "Pick",
            Op::RebaseTodoReword => "Reword",
            Op::RebaseTodoEdit => "Edit",
            Op::RebaseTodoSquash => "Squash",
            Op::RebaseTodoFixup => "Fixup",
            Op::RebaseTodoDrop => "Drop",
            Op::RebaseTodoStart => "Start the rebase",
            Op::RendererFeatures => "List delta features, shell commands and views, and set their keys",
            Op::RendererThemes => "Step through delta's themes",
            Op::SelectCommit => "Choose the commit under the cursor",
            Op::ToggleMenu => "Show or hide the keys",
            Op::ResetSoft => "Reset to a commit, keeping the index and worktree",
            Op::ResetMixed => "Reset to a commit, keeping the worktree",
            Op::ResetHard => "Reset to a commit, with the index and worktree",
            Op::RevertAbort => "Abort the revert",
            Op::RevertContinue => "Continue the revert",
            Op::RevertCommit => "Revert the commit under the cursor",
            Op::CherryPickAbort => "Abort the cherry-pick",
            Op::CherryPickContinue => "Continue the cherry-pick",
            Op::CherryPick => "Cherry-pick the commit under the cursor",
            Op::Merge => "Merge a branch you choose",
            Op::MergeAbort => "Abort the merge",
            Op::MergeContinue => "Continue the merge",
            Op::Stage => "Stage the file, hunk or line",
            Op::Unstage => "Unstage the file, hunk or line",
            Op::Show => {
                "Open the commit, branch, tag, stash or blame line; open a file, hunk or line in your editor"
            }
            Op::ShowOrUnfold => {
                "Open the commit, branch, tag, stash or blame line; unfold a folded file or hunk"
            }
            Op::ShowInAltEditor => "Open the file, hunk or line in general.alt_editor",
            Op::Discard => "Discard the file, hunk or line",
            Op::Apply => "Apply the file, hunk or line",
            Op::Reverse => "Reverse the file, hunk or line",
            Op::CopyHash => "Copy the commit's hash",
            Op::Blame => "Blame the file of the diff line under the cursor",
            Op::ToggleSection => "Fold or unfold",
            Op::ToggleAllSections => "Fold every file; again, unfold everything",
            Op::DiffContext => "Set the diff context",
            Op::WiderDiffContext => "Wider diff context",
            Op::NarrowerDiffContext => "Narrower diff context",
            Op::EditGitCommand => "Edit the git command",
            Op::FilePatterns => "Set which files to show",
            Op::HideFile => "Hide the file under the cursor",
            Op::MoveUpLine => "Up a line, through a diff's lines too",
            Op::MoveDownLine => "Down a line, through a diff's lines too",
            Op::MoveUp => "Up, stopping only at file headers, hunk headers and commits",
            Op::MoveDown => "Down, stopping only at file headers, hunk headers and commits",
            Op::ExtendSelectionUp => "Select lines in a hunk, upwards",
            Op::ExtendSelectionDown => "Select lines in a hunk, downwards",
            Op::MovePrevSection => "Previous section at the same level",
            Op::MoveNextSection => "Next section at the same level",
            Op::MoveParentSection => "The file or commit containing the cursor",
            Op::MoveTop => "Top",
            Op::MoveBottom => "Bottom",
            Op::HalfPageUp => "Half page up",
            Op::HalfPageDown => "Half page down",
            Op::FullPageUp => "Page up",
            Op::FullPageDown => "Page down",
            Op::ScrollViewUp => "Scroll up without moving the cursor",
            Op::ScrollViewDown => "Scroll down without moving the cursor",
            Op::Search => "Search",
            Op::SearchBackward => "Search backward",
            Op::SearchNext => "Next match",
            Op::SearchPrevious => "Previous match",
            Op::Refresh => "Refresh",
            Op::Suspend => "Suspend; fg resumes",
            Op::Quit => "Close the menu; otherwise go back, or on the first screen quit",
            Op::FoldOrBack => "Fold what you unfolded on this screen, if the cursor is in it; otherwise go back",
            Op::OpenMenu(Menu::Help) => "Help",
            Op::OpenMenu(menu) => {
                return format!("Open the {} menu", menu.to_string().to_lowercase()).into();
            }
            Op::ToggleArg(arg) => return format!("Turn {arg} on or off").into(),
            Op::MoveToScreenLine(_) => "Move to a screen line",
        }
        .into()
    }

    /// Whether the op changes the repository: its refs, index, worktree,
    /// remotes or stash. A menu whose ops all do counts as one.
    pub(crate) fn writes(&self) -> bool {
        match self {
            Op::OpenMenu(menu) => !matches!(
                menu,
                Menu::Root | Menu::Help | Menu::Log | Menu::CommitPicker | Menu::RebaseTodo
            ),
            Op::AddRemote
            | Op::Checkout
            | Op::CheckoutNewBranch
            | Op::Spinoff
            | Op::Delete
            | Op::Commit
            | Op::CommitAmend
            | Op::CommitExtend
            | Op::CommitFixup
            | Op::CommitInstantFixup
            | Op::FetchAll
            | Op::FetchPushRemote
            | Op::FetchUpstream
            | Op::FetchElsewhere
            | Op::PullFromPushRemote
            | Op::PullFromUpstream
            | Op::PullFromElsewhere
            | Op::PushToPushRemote
            | Op::PushToUpstream
            | Op::PushToElsewhere
            | Op::PrCurrent
            | Op::PrOther
            | Op::RebaseAbort
            | Op::RebaseContinue
            | Op::RebaseElsewhere
            | Op::RebaseAutosquash
            | Op::RebaseInteractive
            | Op::RemoveRemote
            | Op::Rename
            | Op::RenameRemote
            | Op::Stash
            | Op::StashApply
            | Op::StashIndex
            | Op::StashWorktree
            | Op::StashKeepIndex
            | Op::StashPop
            | Op::StashDrop
            | Op::ResetSoft
            | Op::ResetMixed
            | Op::ResetHard
            | Op::RevertAbort
            | Op::RevertContinue
            | Op::RevertCommit
            | Op::CherryPickAbort
            | Op::CherryPickContinue
            | Op::CherryPick
            | Op::Merge
            | Op::MergeAbort
            | Op::MergeContinue
            | Op::Stage
            | Op::Unstage
            | Op::Discard
            | Op::Apply
            | Op::Reverse => true,
            _ => false,
        }
    }

    pub fn implementation(self) -> Box<dyn OpTrait> {
        match self {
            Op::Quit => Box::new(editor::Quit),
            Op::FoldOrBack => Box::new(editor::FoldOrBack),
            Op::OpenMenu(menu) => Box::new(editor::OpenMenu(menu)),
            Op::Refresh => Box::new(editor::Refresh),
            Op::Suspend => Box::new(editor::Suspend),
            Op::ToggleArg(name) => Box::new(editor::ToggleArg(name)),
            Op::ToggleSection => Box::new(editor::ToggleSection),
            Op::ToggleAllSections => Box::new(editor::ToggleAllSections),
            Op::DiffContext => Box::new(editor::DiffContext),
            Op::WiderDiffContext => Box::new(editor::StepDiffContext(true)),
            Op::NarrowerDiffContext => Box::new(editor::StepDiffContext(false)),
            Op::EditGitCommand => Box::new(editor::EditGitCommand),
            Op::FilePatterns => Box::new(editor::FilePatterns),
            Op::HideFile => Box::new(editor::HideFile),
            Op::MoveDown => Box::new(editor::MoveDown),
            Op::MoveUp => Box::new(editor::MoveUp),
            Op::MoveDownLine => Box::new(editor::MoveDownLine),
            Op::MoveUpLine => Box::new(editor::MoveUpLine),
            Op::ExtendSelectionUp => Box::new(editor::ExtendSelection(false)),
            Op::ExtendSelectionDown => Box::new(editor::ExtendSelection(true)),
            Op::MoveToScreenLine(screen_line) => Box::new(editor::MoveToScreenLine(screen_line)),
            Op::MoveNextSection => Box::new(editor::MoveNextSection),
            Op::MovePrevSection => Box::new(editor::MovePrevSection),
            Op::MoveParentSection => Box::new(editor::MoveParentSection),
            Op::HalfPageUp => Box::new(editor::HalfPageUp),
            Op::HalfPageDown => Box::new(editor::HalfPageDown),
            Op::FullPageUp => Box::new(editor::FullPageUp),
            Op::FullPageDown => Box::new(editor::FullPageDown),
            Op::MoveTop => Box::new(editor::MoveTop),
            Op::MoveBottom => Box::new(editor::MoveBottom),
            Op::ScrollViewUp => Box::new(editor::ScrollViewUp),
            Op::ScrollViewDown => Box::new(editor::ScrollViewDown),
            Op::Search => Box::new(search::Search),
            Op::SearchBackward => Box::new(search::SearchBackward),
            Op::SearchNext => Box::new(search::SearchNext),
            Op::SearchPrevious => Box::new(search::SearchPrevious),
            Op::Checkout => Box::new(branch::Checkout),
            Op::CheckoutNewBranch => Box::new(branch::CheckoutNewBranch),
            Op::Spinoff => Box::new(branch::Spinoff),
            Op::Delete => Box::new(branch::Delete),
            Op::Rename => Box::new(branch::Rename),
            Op::Commit => Box::new(commit::Commit),
            Op::CommitAmend => Box::new(commit::CommitAmend),
            Op::CommitExtend => Box::new(commit::CommitExtend),
            Op::FetchAll => Box::new(fetch::FetchAll),
            Op::FetchElsewhere => Box::new(fetch::FetchElsewhere),
            Op::FetchPushRemote => Box::new(fetch::FetchPushRemote),
            Op::FetchUpstream => Box::new(fetch::FetchUpstream),
            Op::LogCurrent => Box::new(log::LogCurrent),
            Op::PullFromPushRemote => Box::new(pull::PullFromPushRemote),
            Op::PullFromUpstream => Box::new(pull::PullFromUpstream),
            Op::PullFromElsewhere => Box::new(pull::PullFromElsewhere),
            Op::PushToPushRemote => Box::new(push::PushToPushRemote),
            Op::PushToUpstream => Box::new(push::PushToUpstream),
            Op::PushToElsewhere => Box::new(push::PushToElsewhere),
            Op::PrCurrent => Box::new(pr::PrCurrent),
            Op::PrOther => Box::new(pr::PrOther),
            Op::ToggleViewed => Box::new(pr::ToggleViewed),
            Op::ShowOnGithub => Box::new(pr::ShowOnGithub),
            Op::RebaseAbort => Box::new(rebase::RebaseAbort),
            Op::RebaseContinue => Box::new(rebase::RebaseContinue),
            Op::RebaseElsewhere => Box::new(rebase::RebaseElsewhere),
            Op::ShowRefs => Box::new(show_refs::ShowRefs),
            Op::ShowBindings => Box::new(show_bindings::ShowBindings),
            Op::Status => Box::new(status::Status),
            Op::Stash => Box::new(stash::Stash),
            Op::StashApply => Box::new(stash::StashApply),
            Op::StashIndex => Box::new(stash::StashIndex),
            Op::StashWorktree => Box::new(stash::StashWorktree),
            Op::StashKeepIndex => Box::new(stash::StashKeepIndex),
            Op::StashPop => Box::new(stash::StashPop),
            Op::StashDrop => Box::new(stash::StashDrop),

            Op::CommitFixup => Box::new(commit::CommitFixup),
            Op::CommitInstantFixup => Box::new(commit::CommitInstantFixup),
            Op::Discard => Box::new(discard::Discard),
            Op::LogOther => Box::new(log::LogOther),
            Op::RebaseAutosquash => Box::new(rebase::RebaseAutosquash),
            Op::RebaseInteractive => Box::new(rebase::RebaseInteractive),
            Op::RebaseTodoMoveUp => Box::new(rebase_todo::MoveEntry(-1)),
            Op::RebaseTodoMoveDown => Box::new(rebase_todo::MoveEntry(1)),
            Op::RebaseTodoPick => Box::new(rebase_todo::SetAction(TodoAction::Pick)),
            Op::RebaseTodoReword => Box::new(rebase_todo::SetAction(TodoAction::Reword)),
            Op::RebaseTodoEdit => Box::new(rebase_todo::SetAction(TodoAction::Edit)),
            Op::RebaseTodoSquash => Box::new(rebase_todo::SetAction(TodoAction::Squash)),
            Op::RebaseTodoFixup => Box::new(rebase_todo::SetAction(TodoAction::Fixup)),
            Op::RebaseTodoDrop => Box::new(rebase_todo::SetAction(TodoAction::Drop)),
            Op::RebaseTodoStart => Box::new(rebase_todo::Start),
            Op::RendererFeatures => Box::new(editor::RendererFeatures),
            Op::RendererThemes => Box::new(editor::RendererThemes),
            Op::SelectCommit => Box::new(editor::SelectCommit),
            Op::ToggleMenu => Box::new(editor::ToggleMenu),
            Op::ResetSoft => Box::new(reset::ResetSoft),
            Op::ResetMixed => Box::new(reset::ResetMixed),
            Op::ResetHard => Box::new(reset::ResetHard),
            Op::RevertAbort => Box::new(revert::RevertAbort),
            Op::RevertContinue => Box::new(revert::RevertContinue),
            Op::RevertCommit => Box::new(revert::RevertCommit),
            Op::CherryPickAbort => Box::new(cherry_pick::CherryPickAbort),
            Op::CherryPickContinue => Box::new(cherry_pick::CherryPickContinue),
            Op::CherryPick => Box::new(cherry_pick::CherryPick),
            Op::Show => Box::new(show::Show),
            Op::ShowOrUnfold => Box::new(show::ShowOrUnfold),
            Op::ShowInAltEditor => Box::new(show::ShowInAltEditor),
            Op::Stage => Box::new(stage::Stage),
            Op::Unstage => Box::new(unstage::Unstage),
            Op::Apply => Box::new(apply::Apply),
            Op::Reverse => Box::new(reverse::Reverse),
            Op::CopyHash => Box::new(copy_hash::CopyHash),
            Op::Blame => Box::new(blame::Blame),

            Op::AddRemote => Box::new(remote::AddRemote),
            Op::RemoveRemote => Box::new(remote::RemoveRemote),
            Op::RenameRemote => Box::new(remote::RenameRemote),

            Op::Merge => Box::new(merge::Merge),
            Op::MergeAbort => Box::new(merge::MergeAbort),
            Op::MergeContinue => Box::new(merge::MergeContinue),
        }
    }
}

impl Display for Menu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Menu::Root => "Root",
            Menu::Branch => "Branch",
            Menu::Commit => "Commit",
            Menu::CommitPicker => "Select commit",
            Menu::Fetch => "Fetch",
            Menu::Help => "Help",
            Menu::Log => "Log",
            Menu::Merge => "Merge",
            Menu::Remote => "Remote",
            Menu::Pull => "Pull",
            Menu::Push => "Push",
            Menu::Pr => "PR",
            Menu::Rebase => "Rebase",
            Menu::RebaseTodo => "Rebase todo",
            Menu::Reset => "Reset",
            Menu::Revert => "Revert",
            Menu::CherryPick => "Cherry-pick",
            Menu::Stash => "Stash",
        })
    }
}

pub(crate) fn confirm(app: &mut App, term: &mut Term, prompt: &'static str) -> Res<()> {
    app.confirm(term, prompt)
}

pub(crate) fn selected_rev(app: &App) -> Option<Rev> {
    app.selected_rev()
}
