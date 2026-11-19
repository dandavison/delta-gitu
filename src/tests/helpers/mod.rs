pub mod repo;
#[macro_use]
pub mod ui;

pub use repo::*;
pub use ui::*;

/// Set in the process [`run_alone`] starts.
const RUN_ALONE: &str = "GITU_TEST_RUN_ALONE";

/// A command that runs the ignored test `name` in a process of its own, for a
/// test that changes what every test in a process shares, such as an
/// environment variable or the terminal. The test is to return at once unless
/// [`running_alone`], so that `cargo test -- --ignored` passes over it.
pub fn run_alone(name: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "--ignored", "--nocapture", name])
        .env(RUN_ALONE, "1");
    cmd
}

pub fn running_alone() -> bool {
    std::env::var_os(RUN_ALONE).is_some()
}

/// Run the ignored test `name` alone, and fail with its output if it fails.
pub fn assert_passes_alone(name: &str) {
    let output = run_alone(name).output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
