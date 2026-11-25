//! `delta tui --agent-instructions` prints a skill for an AI agent: markdown with YAML front
//! matter, describing commands that exist.

use crate::{cli::Args, review::AGENT_INSTRUCTIONS};
use clap::Parser;

#[test]
fn agent_instructions_is_a_flag() {
    assert!(Args::parse_from(["gitu", "--agent-instructions"]).agent_instructions);
}

#[test]
fn agent_instructions_are_a_skill_with_a_name_and_description() {
    let front_matter = AGENT_INSTRUCTIONS
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(front_matter, _)| front_matter)
        .unwrap_or_else(|| panic!("no front matter:\n{AGENT_INSTRUCTIONS}"));
    assert!(front_matter.contains("\nname: ") || front_matter.starts_with("name: "));
    assert!(front_matter.contains("\ndescription: "), "{front_matter}");
}

/// Each `delta tui review` command in the instructions' code blocks is one delta accepts, with
/// its placeholders, such as `<thread-id>` or `<line>`, filled in with `1`.
#[test]
fn every_review_command_in_the_agent_instructions_is_accepted() {
    let commands: Vec<&str> = AGENT_INSTRUCTIONS
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("delta tui "))
        .collect();
    assert!(commands.len() >= 4, "{AGENT_INSTRUCTIONS}");
    for command in commands {
        let command = command
            .split(" <<")
            .next()
            .unwrap()
            .split(" |")
            .next()
            .unwrap();
        let words = shell_words::split(command).unwrap();
        let words = words.iter().map(|word| {
            if word.starts_with('<') && word.ends_with('>') {
                "1"
            } else {
                word.as_str()
            }
        });
        let result = Args::try_parse_from(std::iter::once("gitu").chain(words));
        assert!(
            result.is_ok(),
            "delta tui {command}: {}",
            result.unwrap_err()
        );
    }
}

/// The instructions say, first, where they come from, so that an agent or person reading an
/// installed copy knows how to update it.
#[test]
fn agent_instructions_say_first_that_they_are_printed_by_agent_instructions() {
    let (_, body) = AGENT_INSTRUCTIONS
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .unwrap();
    let first = body.lines().find(|line| !line.is_empty()).unwrap();
    assert!(first.contains("delta --agent-instructions"), "{first}");
}
