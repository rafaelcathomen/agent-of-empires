//! `aoe fork` command implementation.
//!
//! Forks an existing session: starts a new conversation seeded from a parent
//! session's context. The heavy lifting (cloning the parent's
//! tool/path/group/extra_args, worktree creation, hook trust, persistence) is
//! reused from [`crate::cli::add`] by delegating into [`add::run`] with a
//! fork-shaped [`AddArgs`]; this module only parses the fork-specific flags.

use anyhow::Result;
use clap::Args;

use super::add::{self, AddArgs};

#[derive(Args, Debug)]
pub struct ForkArgs {
    /// Parent session to fork (id, id prefix, or unique title).
    pub parent: String,

    /// Create the forked session in a fresh git worktree on a new branch.
    /// Pass a branch name, or leave empty to auto-generate `fork/<parent-title>`.
    #[arg(long = "branch", num_args = 0..=1, default_missing_value = "")]
    pub branch: Option<String>,

    /// Branch to base the new worktree branch on (use with `--branch`).
    /// Defaults to the repository's default branch.
    #[arg(long = "base")]
    pub base: Option<String>,

    /// Title for the forked session (defaults to `<parent-title>-fork`).
    #[arg(short = 't', long = "title")]
    pub title: Option<String>,

    /// Launch the forked session immediately after creating it.
    #[arg(short = 'l', long)]
    pub launch: bool,
}

pub async fn run(profile: &str, args: ForkArgs) -> Result<()> {
    let add_args = AddArgs::for_fork(args.parent, args.title, args.branch, args.base, args.launch);
    add::run(profile, add_args).await
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    fn parse_fork(argv: &[&str]) -> super::ForkArgs {
        match Cli::try_parse_from(argv).expect("parse").command {
            Some(Commands::Fork(args)) => args,
            _ => panic!("expected Fork command"),
        }
    }

    #[test]
    fn fork_parent_only_parses() {
        let args = parse_fork(&["aoe", "fork", "claude-3"]);
        assert_eq!(args.parent, "claude-3");
        assert_eq!(args.branch, None);
        assert_eq!(args.base, None);
        assert_eq!(args.title, None);
        assert!(!args.launch);
    }

    #[test]
    fn fork_branch_flag_without_value_is_empty_sentinel() {
        // `--branch` with no value yields Some("") so `aoe add` auto-names
        // the branch `fork/<parent-title>`.
        let args = parse_fork(&["aoe", "fork", "claude-3", "--branch"]);
        assert_eq!(args.branch.as_deref(), Some(""));
    }

    #[test]
    fn fork_branch_flag_with_value_parses() {
        let args = parse_fork(&["aoe", "fork", "claude-3", "--branch", "feat-x"]);
        assert_eq!(args.branch.as_deref(), Some("feat-x"));
    }

    #[test]
    fn fork_all_flags_parse() {
        let args = parse_fork(&[
            "aoe", "fork", "claude-3", "--branch", "feat-x", "--base", "main", "--title",
            "my-fork", "--launch",
        ]);
        assert_eq!(args.parent, "claude-3");
        assert_eq!(args.branch.as_deref(), Some("feat-x"));
        assert_eq!(args.base.as_deref(), Some("main"));
        assert_eq!(args.title.as_deref(), Some("my-fork"));
        assert!(args.launch);
    }

    #[test]
    fn for_fork_maps_into_add_args() {
        let add = super::AddArgs::for_fork(
            "parent-1".to_string(),
            Some("t".to_string()),
            Some("".to_string()),
            Some("main".to_string()),
            true,
        );
        assert_eq!(add.fork.as_deref(), Some("parent-1"));
    }
}
