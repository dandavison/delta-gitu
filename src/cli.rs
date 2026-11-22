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

/// The formats `--pr-state-machine-diagram` can print.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum DiagramFormat {
    D2,
}

/// How `delta --tui review show` prints a review.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ShowFormat {
    Markdown,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    /// Copy the reviews of the branch's GitHub PR, as you see them, into the local PR, replacing
    /// the copy fetched before. Your draft reviews are not changed.
    Fetch {
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Edit a comment, with its new text read from stdin: a comment in a draft review, or your own
    /// comment in a review on GitHub, which is edited there and fetched again.
    Edit {
        /// The comment's id, as `delta --tui review show --format json` prints it.
        comment: String,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Delete a comment in a draft review. A thread's first comment can't be deleted while the
    /// thread has replies.
    Delete {
        /// The comment's id, as `delta --tui review show --format json` prints it.
        comment: String,
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
    /// thread, in your draft review, which is started if you have none.
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
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Push the PR's branch, create a draft PR on GitHub if it has none, and add every draft
    /// comment to your pending review there. A thread whose line can't be found stays a draft.
    Post {
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
    /// Reply to a thread, with the reply read from stdin. The reply goes into your draft review,
    /// which is started if you have none.
    Reply {
        /// The thread's id, as `delta --tui review show` prints it.
        thread: String,
        /// The name of the agent writing the reply, shown with it.
        #[clap(long)]
        agent: Option<String>,
        #[clap(long)]
        head: Option<String>,
        #[clap(long)]
        base: Option<String>,
    },
}
