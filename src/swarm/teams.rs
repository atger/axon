//! Teams and reusable agent definitions (templates).
//!
//! An [`AgentDef`] is a saved agent configuration; a [`Team`] groups them. User
//! teams/defs are persisted in SQLite (via [`crate::swarm::store`]); the built-in
//! **axon** team is defined in code and is read-only. Spawning resolves a def by
//! id ([`resolve_def`]) and hands it to [`crate::swarm::Swarm::spawn_from_def`].

use color_eyre::eyre::{Result, WrapErr, bail};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::swarm::agent::{AGENT_WRITER_PROMPT, ALL_CODER_TOOLS, ApprovalPolicy};
use crate::swarm::store::{conn, slug};

/// A named group of agent definitions.
#[derive(Clone, Debug, Serialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub builtin: bool,
}

/// A saved, reusable agent configuration (template).
#[derive(Clone, Debug, Serialize)]
pub struct AgentDef {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub model: Option<String>,
    pub instructions: String,
    pub tools: Vec<String>,
    pub policy: ApprovalPolicy,
    pub memory_window: Option<usize>,
    pub max_turns: Option<usize>,
    /// When `Some(n)`, this is a proactive agent that runs every `n` minutes.
    pub schedule_mins: Option<u64>,
    /// The recurring task a proactive (scheduled) agent runs each cycle.
    pub task: Option<String>,
    /// Placeholder hint shown in the spawn task textarea.
    pub task_hint: Option<String>,
    pub builtin: bool,
}

/// A team together with its agent definitions (the listing shape).
#[derive(Clone, Debug, Serialize)]
pub struct TeamWithAgents {
    pub team: Team,
    pub agents: Vec<AgentDef>,
}

/// Editable fields of an agent definition (request body for create/update).
#[derive(Debug, Deserialize)]
pub struct DefForm {
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub policy: ApprovalPolicy,
    #[serde(default)]
    pub memory_window: Option<usize>,
    #[serde(default)]
    pub max_turns: Option<usize>,
    #[serde(default)]
    pub schedule_mins: Option<u64>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub task_hint: Option<String>,
}

// -- built-in axon team ----------------------------------------------------

pub const BUILTIN_TEAM_ID: &str = "axon";

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The read-only built-in teams (currently just **axon**).
pub fn builtin_teams() -> Vec<TeamWithAgents> {
    let def = |id: &str, name: &str, instructions: &str, tools: Vec<String>, policy| AgentDef {
        id: id.to_string(),
        team_id: BUILTIN_TEAM_ID.to_string(),
        name: name.to_string(),
        model: None,
        instructions: instructions.to_string(),
        tools,
        policy,
        memory_window: None,
        max_turns: None,
        schedule_mins: None,
        task: None,
        task_hint: None,
        builtin: true,
    };
    let agents = vec![def(
        "agent-writer",
        "Agent Writer",
        AGENT_WRITER_PROMPT,
        owned(ALL_CODER_TOOLS),
        ApprovalPolicy::AutoApprove,
    )];
    vec![TeamWithAgents {
        team: Team {
            id: BUILTIN_TEAM_ID.to_string(),
            name: "axon".to_string(),
            builtin: true,
        },
        agents,
    }]
}

fn is_builtin_team(id: &str) -> bool {
    builtin_teams().iter().any(|t| t.team.id == id)
}

fn is_builtin_def(id: &str) -> bool {
    builtin_teams()
        .iter()
        .flat_map(|t| &t.agents)
        .any(|a| a.id == id)
}

// -- policy <-> text -------------------------------------------------------

fn policy_to_str(p: ApprovalPolicy) -> &'static str {
    match p {
        ApprovalPolicy::AutoApprove => "auto_approve",
        ApprovalPolicy::DenyDestructive => "deny_destructive",
    }
}

fn policy_from_str(s: &str) -> ApprovalPolicy {
    match s {
        "deny_destructive" => ApprovalPolicy::DenyDestructive,
        _ => ApprovalPolicy::AutoApprove,
    }
}

// -- persistence -----------------------------------------------------------

const DEF_COLS: &str =
    "id, team_id, name, model, instructions, tools, policy, memory_window, max_turns, schedule_mins, task, task_hint";

fn row_to_def(row: &Row) -> rusqlite::Result<AgentDef> {
    let tools: String = row.get(5)?;
    let policy: String = row.get(6)?;
    let mw: Option<i64> = row.get(7)?;
    let mt: Option<i64> = row.get(8)?;
    let sched: Option<i64> = row.get(9)?;
    Ok(AgentDef {
        id: row.get(0)?,
        team_id: row.get(1)?,
        name: row.get(2)?,
        model: row.get(3)?,
        instructions: row.get(4)?,
        tools: tools
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        policy: policy_from_str(&policy),
        memory_window: mw.map(|n| n as usize),
        max_turns: mt.map(|n| n as usize),
        schedule_mins: sched.map(|n| n as u64),
        task: row.get(10)?,
        task_hint: row.get(11)?,
        builtin: false,
    })
}

fn list_user_teams() -> Result<Vec<Team>> {
    let c = conn();
    let mut stmt = c.prepare("SELECT id, name FROM teams ORDER BY created ASC")?;
    let rows = stmt.query_map([], |r| {
        Ok(Team {
            id: r.get(0)?,
            name: r.get(1)?,
            builtin: false,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn list_defs(team_id: &str) -> Result<Vec<AgentDef>> {
    let c = conn();
    let mut stmt = c.prepare(&format!(
        "SELECT {DEF_COLS} FROM agent_defs WHERE team_id = ?1 ORDER BY created ASC"
    ))?;
    let rows = stmt.query_map(params![team_id], row_to_def)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// All teams (built-in first, then user teams), each with its agents.
pub fn all_teams() -> Result<Vec<TeamWithAgents>> {
    let mut out = builtin_teams();
    
    // Load overrides from DB for built-in agents
    let overrides: Vec<AgentDef> = {
        let c = conn();
        let mut stmt = c.prepare(&format!(
            "SELECT {DEF_COLS} FROM agent_defs WHERE team_id = ?1"
        ))?;
        stmt.query_map(params![BUILTIN_TEAM_ID], row_to_def)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    for ovr in overrides {
        if let Some(builtin_team) = out.iter_mut().find(|t| t.team.id == BUILTIN_TEAM_ID) {
            if let Some(target) = builtin_team.agents.iter_mut().find(|a| a.id == ovr.id) {
                // Preserve the builtin flag but overwrite everything else
                let id = target.id.clone();
                *target = ovr;
                target.id = id;
                target.builtin = true;
            }
        }
    }

    for team in list_user_teams()? {
        let agents = list_defs(&team.id)?;
        out.push(TeamWithAgents { team, agents });
    }
    Ok(out)
}

/// Resolve an agent definition by id (built-in first, then the DB).
pub fn resolve_def(id: &str) -> Result<Option<AgentDef>> {
    let mut def = None;

    // Check built-ins first
    if let Some(d) = builtin_teams()
        .into_iter()
        .flat_map(|t| t.agents)
        .find(|a| a.id == id)
    {
        def = Some(d);
    }

    // Check DB for potential override or user agent
    let c = conn();
    let db_def = c
        .query_row(
            &format!("SELECT {DEF_COLS} FROM agent_defs WHERE id = ?1"),
            params![id],
            row_to_def,
        )
        .optional()
        .wrap_err("failed to query agent def")?;

    if let Some(mut db_d) = db_def {
        if let Some(ref mut b_d) = def {
            // It's an override for a built-in
            let id_save = b_d.id.clone();
            *b_d = db_d;
            b_d.id = id_save;
            b_d.builtin = true;
        } else {
            // It's a user-defined agent
            def = Some(db_d);
        }
    }

    Ok(def)
}

pub fn add_team(name: &str) -> Result<Team> {
    if name.trim().is_empty() {
        bail!("team name must not be empty");
    }
    let now = chrono::Local::now().to_rfc3339();
    let id = format!(
        "{}-{}",
        slug(name),
        chrono::Local::now().format("%Y%m%d%H%M%S")
    );
    conn()
        .execute(
            "INSERT INTO teams (id, name, created) VALUES (?1, ?2, ?3)",
            params![id, name, now],
        )
        .wrap_err("failed to insert team")?;
    Ok(Team {
        id,
        name: name.to_string(),
        builtin: false,
    })
}

pub fn rename_team(id: &str, name: &str) -> Result<()> {
    if is_builtin_team(id) {
        bail!("cannot modify the built-in team");
    }
    let n = conn()
        .execute("UPDATE teams SET name = ?2 WHERE id = ?1", params![id, name])
        .wrap_err("failed to rename team")?;
    if n == 0 {
        bail!("team `{id}` not found");
    }
    Ok(())
}

pub fn delete_team(id: &str) -> Result<()> {
    if is_builtin_team(id) {
        bail!("cannot delete the built-in team");
    }
    let c = conn();
    c.execute("DELETE FROM agent_defs WHERE team_id = ?1", params![id])?;
    let n = c
        .execute("DELETE FROM teams WHERE id = ?1", params![id])
        .wrap_err("failed to delete team")?;
    if n == 0 {
        bail!("team `{id}` not found");
    }
    Ok(())
}

pub fn add_def(team_id: &str, form: &DefForm) -> Result<AgentDef> {
    if is_builtin_team(team_id) {
        bail!("cannot add agents to the built-in team");
    }
    if form.name.trim().is_empty() {
        bail!("agent name must not be empty");
    }
    let now = chrono::Local::now().to_rfc3339();
    let id = format!(
        "{}-{}",
        slug(&form.name),
        chrono::Local::now().format("%Y%m%d%H%M%S")
    );
    conn()
        .execute(
            "INSERT INTO agent_defs
                (id, team_id, name, model, instructions, tools, policy, memory_window, max_turns, schedule_mins, task, task_hint, created, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            params![
                id,
                team_id,
                form.name,
                form.model,
                form.instructions,
                form.tools.join(","),
                policy_to_str(form.policy),
                form.memory_window.map(|n| n as i64),
                form.max_turns.map(|n| n as i64),
                form.schedule_mins.map(|n| n as i64),
                form.task,
                form.task_hint,
                now,
            ],
        )
        .wrap_err("failed to insert agent def")?;
    Ok(AgentDef {
        id,
        team_id: team_id.to_string(),
        name: form.name.clone(),
        model: form.model.clone(),
        instructions: form.instructions.clone(),
        tools: form.tools.clone(),
        policy: form.policy,
        memory_window: form.memory_window,
        max_turns: form.max_turns,
        schedule_mins: form.schedule_mins,
        task: form.task.clone(),
        task_hint: form.task_hint.clone(),
        builtin: false,
    })
}

pub fn update_def(id: &str, form: &DefForm) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    let c = conn();
    
    // Check if it already exists in the DB
    let exists: bool = c.query_row(
        "SELECT 1 FROM agent_defs WHERE id = ?1",
        params![id],
        |_| Ok(true)
    ).unwrap_or(false);

    if exists {
        c.execute(
            "UPDATE agent_defs SET
                name = ?2, model = ?3, instructions = ?4, tools = ?5,
                policy = ?6, memory_window = ?7, max_turns = ?8,
                schedule_mins = ?9, task = ?10, task_hint = ?11, updated = ?12
             WHERE id = ?1",
            params![
                id,
                form.name,
                form.model,
                form.instructions,
                form.tools.join(","),
                policy_to_str(form.policy),
                form.memory_window.map(|n| n as i64),
                form.max_turns.map(|n| n as i64),
                form.schedule_mins.map(|n| n as i64),
                form.task,
                form.task_hint,
                now,
            ],
        )
        .wrap_err("failed to update agent def")?;
    } else {
        // For built-ins that aren't in the DB yet, create an override record.
        let team_id = if is_builtin_def(id) {
            BUILTIN_TEAM_ID.to_string()
        } else {
            bail!("agent `{id}` not found");
        };

        c.execute(
            "INSERT INTO agent_defs
                (id, team_id, name, model, instructions, tools, policy, memory_window, max_turns, schedule_mins, task, task_hint, created, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            params![
                id,
                team_id,
                form.name,
                form.model,
                form.instructions,
                form.tools.join(","),
                policy_to_str(form.policy),
                form.memory_window.map(|n| n as i64),
                form.max_turns.map(|n| n as i64),
                form.schedule_mins.map(|n| n as i64),
                form.task,
                form.task_hint,
                now,
            ],
        )
        .wrap_err("failed to insert agent def override")?;
    }
    Ok(())
}

pub fn delete_def(id: &str) -> Result<()> {
    if is_builtin_def(id) {
        bail!("cannot delete a built-in agent");
    }
    let n = conn()
        .execute("DELETE FROM agent_defs WHERE id = ?1", params![id])
        .wrap_err("failed to delete agent def")?;
    if n == 0 {
        bail!("agent `{id}` not found");
    }
    Ok(())
}
