//! A diagram of the review lifecycle's state machine ([`super::lifecycle`]), in d2, laid out by
//! TALA, which `delta --tui --pr-state-machine-diagram=d2` prints.

/// A state a review can be in, and where it is drawn.
pub(crate) struct State {
    pub key: &'static str,
    pub label: &'static str,
    pub place: Place,
    /// (top, left) within its place.
    pub at: (u32, u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Local,
    GitHub,
}

/// A transition, drawn as an arrow: solid for one that changes a review, dashed for one that
/// copies it.
pub(crate) struct Edge {
    /// `None` for a review that starts here.
    pub from: Option<&'static str>,
    pub to: &'static str,
    pub label: &'static str,
    pub copies: bool,
}

pub(crate) const STATES: &[State] = &[
    State {
        key: "mine",
        label: "My draft review\n(with my agent's comments)",
        place: Place::Local,
        at: (0, 0),
    },
    State {
        key: "copy",
        label: "Local copy of a review on GitHub\n(your own comments are edited on GitHub)",
        place: Place::Local,
        at: (260, 0),
    },
    State {
        key: "pending",
        label: "Pending review\n(only its author sees it)",
        place: Place::GitHub,
        at: (0, 0),
    },
    State {
        key: "submitted",
        label: "Submitted review\n(Commented, Approved,\nChanges requested)",
        place: Place::GitHub,
        at: (260, 0),
    },
    State {
        key: "dismissed",
        label: "Dismissed review",
        place: Place::GitHub,
        at: (520, 0),
    },
];

pub(crate) const EDGES: &[Edge] = &[
    Edge {
        from: None,
        to: "mine",
        label: "i, or review comment [--agent <name>]",
        copies: false,
    },
    Edge {
        from: Some("mine"),
        to: "pending",
        label: "O p, or review post",
        copies: false,
    },
    Edge {
        from: None,
        to: "pending",
        label: "comment in the browser",
        copies: false,
    },
    Edge {
        from: Some("pending"),
        to: "submitted",
        label: "submit on GitHub",
        copies: false,
    },
    Edge {
        from: Some("submitted"),
        to: "dismissed",
        label: "dismiss on GitHub",
        copies: false,
    },
    Edge {
        from: Some("pending"),
        to: "copy",
        label: "review fetch",
        copies: true,
    },
    Edge {
        from: Some("submitted"),
        to: "copy",
        label: "review fetch",
        copies: true,
    },
    Edge {
        from: Some("dismissed"),
        to: "copy",
        label: "review fetch",
        copies: true,
    },
];

/// The state machine as d2, laid out by TALA: the local PR's reviews on the left, the GitHub
/// PR's on the right.
pub(crate) fn d2() -> String {
    let mut out = String::from(
        "vars: {\n  d2-config: {\n    layout-engine: tala\n  }\n}\n\ndirection: right\n\n",
    );
    for (place, key, title, left) in [
        (Place::Local, "local", "Local PR (this repository)", 0),
        (Place::GitHub, "github", "GitHub PR", 800),
    ] {
        out += &format!("{key}: {title} {{\n  top: 0\n  left: {left}\n");
        out += "  start: \"\" {\n    shape: circle\n    width: 16\n    height: 16\n    style.fill: black\n    top: 0\n    left: 0\n  }\n";
        for state in STATES.iter().filter(|s| s.place == place) {
            let label = state.label.replace('\n', "\\n");
            out += &format!(
                "  {}: \"{label}\" {{\n    top: {}\n    left: {}\n    width: 300\n",
                state.key,
                state.at.0 + 120,
                state.at.1 + 80
            );
            if state.key == "copy" {
                out += "    style.stroke-dash: 3\n";
            }
            out += "  }\n";
        }
        out += "}\n\n";
    }
    let path = |key: &str| {
        let state = STATES.iter().find(|s| s.key == key).expect("a state");
        match state.place {
            Place::Local => format!("local.{key}"),
            Place::GitHub => format!("github.{key}"),
        }
    };
    for edge in EDGES {
        let from = match edge.from {
            Some(from) => path(from),
            None => match STATES.iter().find(|s| s.key == edge.to).unwrap().place {
                Place::Local => "local.start".to_owned(),
                Place::GitHub => "github.start".to_owned(),
            },
        };
        out += &format!("{from} -> {}: \"{}\"", path(edge.to), edge.label);
        if edge.copies {
            out += " {style.stroke-dash: 3}";
        }
        out += "\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::lifecycle::{Action, Person, Repo};

    /// The diagram's edge for each of the state machine's actions, or `None` for one that
    /// changes no review's state. The match has no wildcard, so that a new action can't be added
    /// without deciding where it goes in the diagram.
    fn edge_label(action: &Action) -> Option<&'static str> {
        match action {
            Action::Write { thread: None, .. } => Some("i, or review comment [--agent <name>]"),
            // A reply goes into the same draft review as a comment.
            Action::Write {
                thread: Some(_), ..
            } => None,
            // Deleting a comment leaves its review where it is.
            Action::Delete { .. } => None,
            Action::Post { .. } => Some("O p, or review post"),
            Action::Fetch { .. } => Some("review fetch"),
            Action::Submit { .. } => Some("submit on GitHub"),
            Action::BrowserComment { .. } => Some("comment in the browser"),
            Action::Dismiss { .. } => Some("dismiss on GitHub"),
            // A thread's line going changes where its comments are, not their review's state.
            Action::ChangeCode { .. } => None,
            // Editing or deleting my own comment on GitHub changes what a review says, not its
            // state.
            Action::EditOnGitHub { .. } | Action::DeleteOnGitHub { .. } => None,
        }
    }

    #[test]
    fn every_action_that_changes_a_reviews_state_is_in_the_diagram() {
        let actions = [
            Action::Write {
                repo: Repo::Laptop,
                thread: None,
            },
            Action::Post {
                repo: Repo::Laptop,
                stop_after: None,
            },
            Action::Fetch { repo: Repo::Laptop },
            Action::Submit { user: Person::Me },
            Action::BrowserComment { user: Person::Me },
            Action::Dismiss { review: 1 },
        ];
        for action in actions {
            let label = edge_label(&action).unwrap();
            assert!(
                EDGES.iter().any(|edge| edge.label == label),
                "{action}: no edge {label}"
            );
        }
        for edge in EDGES {
            assert!(STATES.iter().any(|s| s.key == edge.to), "{}", edge.to);
            if let Some(from) = edge.from {
                assert!(STATES.iter().any(|s| s.key == from), "{from}");
            }
        }
    }

    #[test]
    fn the_diagram_is_d2_laid_out_by_tala() {
        let d2 = d2();
        assert!(d2.contains("layout-engine: tala"), "{d2}");
        for edge in EDGES {
            assert!(d2.contains(edge.label), "{d2}");
        }
    }
}
