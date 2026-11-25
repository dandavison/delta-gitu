use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Default, Debug, Parser)]
#[command(name = "gitu")]
#[command(flatten_help = true)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Send keys on startup (eg: `gitu -k ll`).
    ///     It is possible to send:
    ///     - single char-keys: a, b, c, ...
    ///     - special keys: <backspace>, <enter>, <up>, <tab>, <delete>, <esc>, ...
    ///     - modifiers: <ctrl+a>, <ctrl+shift+alt+a>, <shift+delete>
    #[clap(short, long, verbatim_doc_comment)]
    pub keys: Option<String>,

    /// Print one frame and exit. Useful for debugging.
    #[clap(long, action)]
    pub print: bool,

    /// Paging a patch piped to gitu, as git's pager, print it and exit if it
    /// fits on one screen, as less's --quit-if-one-screen.
    #[clap(long, action)]
    pub quit_if_one_screen: bool,

    /// Enable logging to 'gitu.log'
    #[clap(long, action)]
    pub log: bool,

    /// Show external command output in the message area.
    #[clap(long, action)]
    pub debug: bool,

    #[clap(long, action)]
    /// Print version
    pub version: bool,

    /// Print instructions for an AI agent on using delta to review code with a person, as a
    /// skill: markdown with YAML front matter, which can be saved as a SKILL.md.
    #[clap(long, action)]
    pub agent_instructions: bool,

    /// Print a diagram of the states a PR's reviews go through, locally and on GitHub, in the
    /// format given: d2, to be laid out by d2's TALA engine.
    #[clap(long, value_enum, value_name = "FORMAT")]
    pub pr_state_machine_diagram: Option<DiagramFormat>,

    /// Print every setting the TUI takes from git config's [delta] section, with what it does
    /// and its value here, as git config (the default), or as JSON with `--show-config=json`.
    #[clap(
        long,
        value_enum,
        value_name = "FORMAT",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "text"
    )]
    pub show_config: Option<ConfigFormat>,

    /// Config file to use
    #[clap(short, long)]
    pub config: Option<PathBuf>,

    /// Print the settings in git config's old [delta-tui] sections as they go in [delta], and
    /// exit. This option is temporary and will be removed.
    #[clap(long, action)]
    pub migrate_old_config: bool,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Show {
        reference: String,
    },
    /// Interactively rebase onto <upstream>, editing the instruction list in Gitu.
    Rebase {
        upstream: String,
    },
    /// Edit a `git rebase -i` instruction list, for use as GIT_SEQUENCE_EDITOR:
    ///     GIT_SEQUENCE_EDITOR='gitu sequence-editor' git rebase -i <upstream>
    #[clap(verbatim_doc_comment)]
    SequenceEditor {
        file: PathBuf,
    },
    Blame {
        file: String,
        #[clap(short, long)]
        rev: Option<String>,
    },
    /// Read and write a local PR's reviews, for scripts and agents. The PR is the current branch,
    /// unless --head says otherwise, into the base given by --base, or else into the base gh
    /// gives: the base of the branch's GitHub PR, or the default branch of the repository gh
    /// would open a PR against. Without --base, gh needs the network.
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
}

/// The side of a PR's diff a line is on: the new file's, or the old file's, for a removed line.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ReviewSide {
    New,
    Old,
}

/// The formats `--show-config` can print.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ConfigFormat {
    Text,
    Json,
}

/// The formats `--pr-state-machine-diagram` can print.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum DiagramFormat {
    D2,
}

/// How `delta tui review show` prints a review.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ShowFormat {
    Markdown,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    /// Copy the reviews of the branch's GitHub PR, as you see them, into the local PR, replacing
    /// the copy fetched before. Local comments, draft or local-only, are not changed.
    Fetch {
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Edit a comment, with its new text read from stdin: a local comment, draft or local-only, or
    /// your own comment in a review on GitHub, which is edited there and fetched again.
    Edit {
        /// The comment's id, as `delta tui review show --format json` prints it.
        comment: String,
        /// Change it although it is in a submitted review, which others see.
        #[clap(long)]
        publish: bool,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Delete a local comment, draft or local-only, or your own reply in a review on GitHub. A
    /// thread's first comment can't be deleted while the thread has replies.
    Delete {
        /// The comment's id, as `delta tui review show --format json` prints it.
        comment: String,
        /// Delete it although it is in a submitted review, which others see.
        #[clap(long)]
        publish: bool,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Print the PR's review threads as markdown: each thread's file and line, the lines it is
    /// on, and its comments.
    Show {
        #[clap(long, value_enum, default_value_t = ShowFormat::Markdown)]
        format: ShowFormat,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Comment on a line of the PR's diff, with the comment read from stdin. The comment starts a
    /// thread, in your draft review, which is started if you have none, unless --local.
    Comment {
        /// The file's path, from the top of the repository.
        #[clap(long)]
        path: String,
        /// The line, numbered as in the PR's head for the new side, or as where it branched from
        /// its base for the old side.
        #[clap(long)]
        line: u32,
        #[clap(long, value_enum, default_value_t = ReviewSide::New)]
        side: ReviewSide,
        /// The name of the agent writing the comment, shown with it.
        #[clap(long)]
        agent: Option<String>,
        /// Make the comment local-only: sync never posts it to GitHub.
        #[clap(long, conflicts_with = "draft")]
        local: bool,
        /// Put the comment in your draft review, which sync posts to GitHub. This is the default.
        #[clap(long)]
        draft: bool,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Sync the PR with GitHub: copy GitHub's reviews into the local PR, bring the branch up to
    /// the PR's head, and add every draft comment to your pending review there. With no draft
    /// comments, it only copies GitHub's reviews. Local-only comments are never posted.
    Sync {
        /// Create a draft PR on GitHub if the branch has none. A PR makes the branch visible to
        /// everyone who can see the repository.
        #[clap(long)]
        create_pr: bool,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Reply to a thread, with the reply read from stdin. The reply is local-only if the thread's
    /// first comment is, and otherwise goes into your draft review, which is started if you have
    /// none.
    Reply {
        /// The thread's id, as `delta tui review show` prints it.
        thread: String,
        /// The name of the agent writing the reply, shown with it.
        #[clap(long)]
        agent: Option<String>,
        /// Make the reply local-only: sync never posts it to GitHub.
        #[clap(long, conflicts_with = "draft")]
        local: bool,
        /// Put the reply in your draft review, which sync posts to GitHub.
        #[clap(long)]
        draft: bool,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Move a local comment to local-only, which sync never posts, or to your draft review, which
    /// sync posts to GitHub.
    Move {
        /// The comment's id, as `delta tui review show --format json` prints it.
        comment: String,
        /// Move it to local-only.
        #[clap(long, conflicts_with = "draft", required_unless_present = "draft")]
        local: bool,
        /// Move it to your draft review, which is started if you have none.
        #[clap(long)]
        draft: bool,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
}
