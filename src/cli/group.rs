//! `agent-of-empires group` subcommands implementation

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::session::{FolderColor, GroupTree, Storage};

#[derive(Subcommand)]
pub enum GroupCommands {
    /// List all groups
    #[command(alias = "ls")]
    List(GroupListArgs),

    /// Create a new group
    Create(GroupCreateArgs),

    /// Delete a group
    Delete(GroupDeleteArgs),

    /// Move session to group
    Move(GroupMoveArgs),

    /// Set or clear a group's color
    Color(GroupColorArgs),
}

#[derive(Args)]
pub struct GroupListArgs {
    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct GroupCreateArgs {
    /// Group name
    name: String,

    /// Parent group for creating subgroups
    #[arg(long)]
    parent: Option<String>,
}

#[derive(Args)]
pub struct GroupDeleteArgs {
    /// Group name
    name: String,

    /// Force delete by moving sessions to default group
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
pub struct GroupMoveArgs {
    /// Session ID or title
    identifier: String,

    /// Target group
    group: String,
}

#[derive(Args)]
pub struct GroupColorArgs {
    /// Group path (slash-separated, e.g. "work/frontend")
    path: String,

    /// Color name: amber, teal, sky, violet, rose, or slate
    #[arg(required_unless_present = "clear")]
    color: Option<String>,

    /// Remove the group's color
    #[arg(long, conflicts_with = "color")]
    clear: bool,
}

#[derive(Serialize)]
struct GroupInfo {
    name: String,
    path: String,
    session_count: usize,
    children: Vec<String>,
}

#[tracing::instrument(target = "cli.session", skip_all, fields(profile = %profile))]
pub async fn run(profile: &str, command: GroupCommands) -> Result<()> {
    match command {
        GroupCommands::List(args) => list_groups(profile, args).await,
        GroupCommands::Create(args) => create_group(profile, args).await,
        GroupCommands::Delete(args) => delete_group(profile, args).await,
        GroupCommands::Move(args) => move_session(profile, args).await,
        GroupCommands::Color(args) => set_group_color(profile, args).await,
    }
}

async fn list_groups(profile: &str, args: GroupListArgs) -> Result<()> {
    let storage = Storage::new_unwatched(profile)?;
    let (instances, groups) = storage.load_with_groups()?;

    let group_tree = GroupTree::new_with_groups(&instances, &groups);

    if args.json {
        let group_list: Vec<GroupInfo> = group_tree
            .get_all_groups()
            .iter()
            .map(|g| {
                let session_count = instances.iter().filter(|i| i.group_path == g.path).count();
                GroupInfo {
                    name: g.name.clone(),
                    path: g.path.clone(),
                    session_count,
                    children: g.children.iter().map(|c| c.name.clone()).collect(),
                }
            })
            .collect();
        super::output::print_json(&group_list)?;
    } else {
        let all_groups = group_tree.get_all_groups();
        if all_groups.is_empty() {
            println!("No groups found.");
            println!("Create one with: aoe group create <name>");
            return Ok(());
        }

        println!("Groups:\n");
        for group in &all_groups {
            let session_count = instances
                .iter()
                .filter(|i| i.group_path == group.path)
                .count();
            let indent = group.path.matches('/').count();
            println!(
                "{}• {} ({} sessions)",
                "  ".repeat(indent),
                group.name,
                session_count
            );
        }
        println!("\nTotal: {} groups", all_groups.len());
    }

    Ok(())
}

async fn create_group(profile: &str, args: GroupCreateArgs) -> Result<()> {
    let storage = Storage::new_unwatched(profile)?;

    let name = args.name.trim().to_string();
    let group_path = if let Some(parent) = &args.parent {
        format!("{}/{}", parent.trim(), name)
    } else {
        name.clone()
    };

    storage.update(|instances, groups| {
        let mut group_tree = GroupTree::new_with_groups(instances, groups);
        if group_tree.group_exists(&group_path) {
            bail!("Group already exists: {}", group_path);
        }
        group_tree.create_group(&group_path);
        *groups = group_tree.get_all_groups();
        Ok(())
    })?;

    println!("✓ Created group: {}", group_path);
    Ok(())
}

async fn delete_group(profile: &str, args: GroupDeleteArgs) -> Result<()> {
    let storage = Storage::new_unwatched(profile)?;
    let name = args.name.trim().to_string();
    let force = args.force;

    let session_count = storage.update(|instances, groups| {
        let mut group_tree = GroupTree::new_with_groups(instances, groups);
        if !group_tree.group_exists(&name) {
            bail!("Group not found: {}", name);
        }

        let session_count = instances
            .iter()
            .filter(|i| i.group_path == name || i.group_path.starts_with(&format!("{}/", name)))
            .count();

        if session_count > 0 {
            if !force {
                bail!(
                    "Group '{}' contains {} sessions. Use --force to move them to default group.",
                    name,
                    session_count
                );
            }

            for inst in instances.iter_mut() {
                if inst.group_path == name || inst.group_path.starts_with(&format!("{}/", name)) {
                    inst.group_path = String::new();
                }
            }
        }

        group_tree.delete_group(&name);
        *groups = group_tree.get_all_groups();
        Ok(session_count)
    })?;

    println!("✓ Deleted group: {}", name);
    if force && session_count > 0 {
        println!("  Moved {} sessions to default group", session_count);
    }

    Ok(())
}

async fn move_session(profile: &str, args: GroupMoveArgs) -> Result<()> {
    let storage = Storage::new_unwatched(profile)?;
    let identifier = args.identifier.trim().to_string();
    let group = args.group.trim().to_string();

    let old_group = storage.update(|instances, groups| {
        let id = super::resolve_session(&identifier, instances)?.id.clone();
        let inst = instances
            .iter_mut()
            .find(|i| i.id == id)
            .expect("resolve_session returned an id that is no longer in instances");
        let old = inst.group_path.clone();
        inst.group_path = group.clone();

        if !group.is_empty() {
            let mut group_tree = GroupTree::new_with_groups(instances, groups);
            group_tree.create_group(&group);
            *groups = group_tree.get_all_groups();
        }
        Ok(old)
    })?;

    if old_group.is_empty() {
        println!("✓ Moved session to group: {}", group);
    } else {
        println!("✓ Moved session from '{}' to '{}'", old_group, group);
    }

    Ok(())
}

async fn set_group_color(profile: &str, args: GroupColorArgs) -> Result<()> {
    let storage = Storage::new_unwatched(profile)?;
    let path = args.path.trim().to_string();

    // Resolve the requested color before touching storage so an invalid
    // name fails fast without a load/save cycle.
    let color = if args.clear {
        None
    } else {
        let name = args
            .color
            .as_deref()
            .expect("clap required_unless_present guarantees color without --clear");
        match FolderColor::from_str_opt(name) {
            Some(c) => Some(c),
            None => bail!(
                "Invalid color '{}'. Valid colors: amber, teal, sky, violet, rose, slate",
                name
            ),
        }
    };

    storage.update(|instances, groups| {
        let mut group_tree = GroupTree::new_with_groups(instances, groups);
        if !group_tree.set_color(&path, color) {
            bail!("Group not found: {}", path);
        }
        *groups = group_tree.get_all_groups();
        Ok(())
    })?;

    if args.clear {
        println!("✓ Cleared color for group: {}", path);
    } else {
        println!(
            "✓ Set group {} color to {}",
            path,
            color.expect("color is Some when not clearing").as_str()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;

    fn setup_test_home(temp: &std::path::Path) {
        std::env::set_var("HOME", temp);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        std::env::set_var("XDG_CONFIG_HOME", temp.join(".config"));
    }

    /// Seed a single group via the storage layer so the CLI handler has a
    /// real groups.json to read back.
    fn seed_group(profile: &str, path: &str) {
        let storage = Storage::new_unwatched(profile).unwrap();
        storage
            .update(|instances, groups| {
                let mut tree = GroupTree::new_with_groups(instances, groups);
                tree.create_group(path);
                *groups = tree.get_all_groups();
                Ok(())
            })
            .unwrap();
    }

    fn group_color(profile: &str, path: &str) -> Option<FolderColor> {
        let storage = Storage::new_unwatched(profile).unwrap();
        let (_, groups) = storage.load_with_groups().unwrap();
        groups.iter().find(|g| g.path == path).and_then(|g| g.color)
    }

    #[tokio::test]
    #[serial]
    async fn test_group_color_set_persists() {
        let temp = tempdir().unwrap();
        setup_test_home(temp.path());
        seed_group("test-color", "work");

        set_group_color(
            "test-color",
            GroupColorArgs {
                path: "work".to_string(),
                color: Some("teal".to_string()),
                clear: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(group_color("test-color", "work"), Some(FolderColor::Teal));
    }

    #[tokio::test]
    #[serial]
    async fn test_group_color_clear() {
        let temp = tempdir().unwrap();
        setup_test_home(temp.path());
        seed_group("test-color", "work");
        set_group_color(
            "test-color",
            GroupColorArgs {
                path: "work".to_string(),
                color: Some("rose".to_string()),
                clear: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(group_color("test-color", "work"), Some(FolderColor::Rose));

        set_group_color(
            "test-color",
            GroupColorArgs {
                path: "work".to_string(),
                color: None,
                clear: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(group_color("test-color", "work"), None);
    }

    #[tokio::test]
    #[serial]
    async fn test_group_color_invalid_name() {
        let temp = tempdir().unwrap();
        setup_test_home(temp.path());
        seed_group("test-color", "work");

        let err = set_group_color(
            "test-color",
            GroupColorArgs {
                path: "work".to_string(),
                color: Some("chartreuse".to_string()),
                clear: false,
            },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("Invalid color"), "got: {err}");
        assert!(err.contains("amber"), "error lists valid names: {err}");
        // Disk is untouched on an invalid color.
        assert_eq!(group_color("test-color", "work"), None);
    }

    #[tokio::test]
    #[serial]
    async fn test_group_color_missing_group() {
        let temp = tempdir().unwrap();
        setup_test_home(temp.path());
        seed_group("test-color", "work");

        let err = set_group_color(
            "test-color",
            GroupColorArgs {
                path: "nope".to_string(),
                color: Some("teal".to_string()),
                clear: false,
            },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("Group not found"), "got: {err}");
    }
}
