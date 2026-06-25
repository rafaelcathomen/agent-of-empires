use anyhow::Result;
use clap::{Args, Subcommand};

use crate::automation::model::{Automation, LaunchSpec, Trigger};
use crate::automation::store::AutomationStore;
use crate::session::View;

#[derive(Subcommand)]
pub enum AutomationCommands {
    /// Create an automation (trigger + what to launch)
    Add(AutomationAddArgs),
    /// List automations
    #[command(alias = "ls")]
    List(AutomationListArgs),
    /// Remove an automation by id
    Rm(IdArgs),
    /// Enable an automation
    Enable(IdArgs),
    /// Disable an automation
    Disable(IdArgs),
    /// Fire an automation immediately (for testing)
    RunNow(IdArgs),
}

#[derive(Args)]
pub struct AutomationAddArgs {
    #[arg(long)]
    name: String,
    /// 5-field cron expression (local timezone)
    #[arg(long)]
    cron: String,
    #[arg(long, default_value = ".")]
    path: String,
    #[arg(long)]
    tool: Option<String>,
    #[arg(long = "cmd")]
    command: Option<String>,
    #[arg(long)]
    prompt: String,
    /// Reuse one session across runs instead of a fresh session each time
    #[arg(long)]
    persistent: bool,
    /// Do not auto-spawn the scheduler daemon (test/CI use)
    #[arg(long, hide = true)]
    no_launch_daemon: bool,
}

#[derive(Args)]
pub struct AutomationListArgs {}

#[derive(Args)]
pub struct IdArgs {
    /// Automation id or short id
    id: String,
}

pub async fn run(profile: &str, command: AutomationCommands) -> Result<()> {
    let store = AutomationStore::new(profile)?;
    match command {
        AutomationCommands::Add(args) => {
            // Validate the cron expression up front.
            crate::automation::cron::parse(&args.cron)?;
            let spec = LaunchSpec {
                project_path: std::fs::canonicalize(&args.path)?
                    .to_string_lossy()
                    .into_owned(),
                group_path: String::new(),
                tool: args.tool.clone(),
                command: args.command.clone(),
                extra_args: String::new(),
                view: View::Terminal,
                worktree_branch: None,
                sandbox: false,
                auto_approve: true,
                max_runtime_secs: 1800,
                initial_prompt: args.prompt.clone(),
                agent_name: None,
                agent_model: None,
            };
            let mut a = Automation::new(
                &args.name,
                spec,
                Trigger::Cron {
                    expr: args.cron.clone(),
                },
            );
            if args.persistent {
                a.session_mode = crate::automation::model::SessionMode::Persistent;
            }
            let short = a.short_id().to_string();
            store.update(|list| {
                list.push(a.clone());
                Ok(())
            })?;
            if !args.no_launch_daemon
                && crate::automation::lifecycle::ensure_scheduler_running(profile)?
            {
                println!("Started the scheduler daemon (auto-spawn).");
            }
            println!("Created automation {short} ({})", args.name);
            println!(
                "Note: runs unattended with auto-approve enabled and no sandbox (executes on the host)."
            );
        }
        AutomationCommands::List(_) => {
            for a in store.load()? {
                let next = a
                    .state
                    .next_fire
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| "pending".into());
                let Trigger::Cron { expr } = &a.trigger;
                let flag = if a.enabled { "on " } else { "off" };
                println!(
                    "{}  [{flag}]  {expr:<14}  next={next}  {}",
                    a.short_id(),
                    a.name
                );
            }
        }
        AutomationCommands::Rm(args) => {
            let mut removed = false;
            store.update(|list| {
                let before = list.len();
                list.retain(|a| !id_matches(a, &args.id));
                removed = list.len() != before;
                Ok(())
            })?;
            println!(
                "{}",
                if removed {
                    "Removed."
                } else {
                    "No matching automation."
                }
            );
        }
        AutomationCommands::Enable(args) => set_enabled(&store, &args.id, true)?,
        AutomationCommands::Disable(args) => set_enabled(&store, &args.id, false)?,
        AutomationCommands::RunNow(args) => {
            let list = store.load()?;
            let a = list
                .iter()
                .find(|a| id_matches(a, &args.id))
                .ok_or_else(|| anyhow::anyhow!("no matching automation"))?;
            let dispatched = crate::automation::dispatch::launch_run(a, profile).await?;
            println!("Launched run as session {}.", dispatched.session_id);
        }
    }
    Ok(())
}

fn id_matches(a: &Automation, id: &str) -> bool {
    a.id == id || a.short_id() == id
}

fn set_enabled(store: &AutomationStore, id: &str, enabled: bool) -> Result<()> {
    store.update(|list| {
        if let Some(a) = list.iter_mut().find(|a| id_matches(a, id)) {
            a.enabled = enabled;
            if enabled {
                a.state.next_fire = None; // recompute on next tick
            }
        }
        Ok(())
    })?;
    println!("{}", if enabled { "Enabled." } else { "Disabled." });
    Ok(())
}
