//! Teams and reusable agent definitions (templates).
//!
//! An [`AgentDef`] is a saved agent configuration; a [`Team`] groups them. User
//! teams/defs are persisted in SQLite (via [`crate::swarm::store`]); the built-in
//! **axon** team is defined in code and is read-only. Spawning resolves a def by
//! id ([`resolve_def`]) and hands it to [`crate::swarm::Swarm::spawn_from_def`].

use color_eyre::eyre::{Result, WrapErr, bail};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::swarm::agent::{
    ALL_CODER_TOOLS, ApprovalPolicy, CODER_PROMPT, READONLY_TOOLS, RESEARCH_AGENT_PROMPT,
    REVIEWER_PROMPT,
};
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
        builtin: true,
    };
    let agents = vec![
        def(
            "axon-coder",
            "Coder",
            CODER_PROMPT,
            owned(ALL_CODER_TOOLS),
            ApprovalPolicy::AutoApprove,
        ),
        def(
            "axon-researcher",
            "Researcher",
            RESEARCH_AGENT_PROMPT,
            owned(&["web_search", "read_file", "list_dir", "search_file"]),
            ApprovalPolicy::AutoApprove,
        ),
        def(
            "axon-reviewer",
            "Reviewer",
            REVIEWER_PROMPT,
            owned(READONLY_TOOLS),
            ApprovalPolicy::DenyDestructive,
        ),
    ];
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
    "id, team_id, name, model, instructions, tools, policy, memory_window, max_turns, schedule_mins, task";

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
    for team in list_user_teams()? {
        let agents = list_defs(&team.id)?;
        out.push(TeamWithAgents { team, agents });
    }
    Ok(out)
}

/// Resolve an agent definition by id (built-in first, then the DB).
pub fn resolve_def(id: &str) -> Result<Option<AgentDef>> {
    if let Some(d) = builtin_teams()
        .into_iter()
        .flat_map(|t| t.agents)
        .find(|a| a.id == id)
    {
        return Ok(Some(d));
    }
    let c = conn();
    let def = c
        .query_row(
            &format!("SELECT {DEF_COLS} FROM agent_defs WHERE id = ?1"),
            params![id],
            row_to_def,
        )
        .optional()
        .wrap_err("failed to query agent def")?;
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
               (id, team_id, name, model, instructions, tools, policy, memory_window, max_turns, schedule_mins, task, created, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
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
        builtin: false,
    })
}

pub fn update_def(id: &str, form: &DefForm) -> Result<()> {
    if is_builtin_def(id) {
        bail!("cannot modify a built-in agent");
    }
    let now = chrono::Local::now().to_rfc3339();
    let n = conn()
        .execute(
            "UPDATE agent_defs SET
               name = ?2, model = ?3, instructions = ?4, tools = ?5,
               policy = ?6, memory_window = ?7, max_turns = ?8,
               schedule_mins = ?9, task = ?10, updated = ?11
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
                now,
            ],
        )
        .wrap_err("failed to update agent def")?;
    if n == 0 {
        bail!("agent `{id}` not found");
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
