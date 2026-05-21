//! Shared helpers for persistent team and mailbox-backed collaboration tools.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use claude_core::PermissionMode;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::tasks;
use claude_swarm::{
    BackendType, SwarmError, TeamAllowedPath, TeamFile, TeamMember, mailbox, team_helpers,
};
use claude_swarm::{SpawnConfig, in_process_runner::InProcessRunner};

static LIVE_TEAMMATE_RUNNERS: Lazy<Mutex<BTreeMap<String, Arc<InProcessRunner>>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

fn requested_team_name(input: &Value) -> Option<String> {
    input
        .get("team_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn requested_description(input: &Value) -> Option<String> {
    input
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn current_session_team_name() -> Result<Option<String>> {
    if let Some(team_name) = tasks::leader_team_name()? {
        return Ok(Some(team_name));
    }

    if let Ok(value) = std::env::var(claude_swarm::constants::ENV_TEAM_NAME) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_owned()));
        }
    }

    Ok(None)
}

fn sanitize_team_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_').trim_matches('-').to_owned();
    let candidate = if trimmed.is_empty() {
        "team".to_owned()
    } else {
        trimmed
    };
    let starts_with_alnum = candidate
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric());
    let mut normalized = if starts_with_alnum {
        candidate
    } else {
        format!("team_{candidate}")
    };
    if normalized.len() > 64 {
        normalized.truncate(64);
    }
    normalized
}

fn live_runner_key(team_name: &str, agent_name: &str) -> String {
    format!("{team_name}:{agent_name}")
}

async fn all_team_names() -> Result<Vec<String>> {
    team_helpers::list_teams()
        .await
        .context("failed to list teams")
        .map(|mut teams| {
            teams.sort();
            teams
        })
}

async fn unique_team_name(base: &str) -> Result<String> {
    let taken = all_team_names().await?;
    if !taken.iter().any(|name| name == base) {
        return Ok(base.to_owned());
    }

    for suffix in 2..=999 {
        let max_base_len = 64usize.saturating_sub(suffix.to_string().len() + 1);
        let mut candidate_base = base.to_owned();
        if candidate_base.len() > max_base_len {
            candidate_base.truncate(max_base_len);
        }
        let candidate = format!("{candidate_base}-{suffix}");
        if !taken.iter().any(|name| name == &candidate) {
            return Ok(candidate);
        }
    }

    Ok(format!("team-{}", Uuid::new_v4().simple()))
}

fn objective_from_team(team: &TeamFile) -> Option<&str> {
    team.description.as_deref()
}

async fn unread_count(team_name: &str, agent_name: &str) -> Result<usize> {
    mailbox::count_unread(team_name, agent_name)
        .await
        .map_err(anyhow::Error::from)
}

pub(crate) async fn resolve_single_team_name(explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(sanitize_team_name(name));
    }

    let teams = all_team_names().await?;
    match teams.as_slice() {
        [] => Err(anyhow!(
            "no active team found; create one with team_create or pass team_name explicitly"
        )),
        [single] => Ok(single.clone()),
        _ => Err(anyhow!(
            "multiple teams are available; pass team_name explicitly"
        )),
    }
}

pub(crate) async fn load_team(team_name: &str) -> Result<TeamFile> {
    team_helpers::read_team(team_name)
        .await
        .map_err(|error| match error {
            SwarmError::TeamNotFound(_) => anyhow!("team '{team_name}' was not found"),
            other => anyhow!(other),
        })
}

pub(crate) fn team_name_from_input(input: &Value) -> Option<String> {
    requested_team_name(input)
}

#[derive(Debug, Clone)]
pub(crate) struct LiveTeammateRegistration {
    pub team_name: String,
    pub agent_name: String,
    pub agent_type: String,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub permission_mode: Option<PermissionMode>,
    pub objective: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct LiveTeammateHandle {
    pub team_name: String,
    pub agent_name: String,
    pub agent_id: String,
    pub pane_id: String,
}

pub(crate) async fn start_live_teammate(
    registration: &LiveTeammateRegistration,
) -> Result<LiveTeammateHandle> {
    let team_name = sanitize_team_name(&registration.team_name);
    team_helpers::validate_team_name(&team_name).map_err(anyhow::Error::from)?;
    team_helpers::validate_agent_name(&registration.agent_name).map_err(anyhow::Error::from)?;

    let mut team = match team_helpers::read_team(&team_name).await {
        Ok(existing) => existing,
        Err(SwarmError::TeamNotFound(_)) => TeamFile::new(&team_name, "lead"),
        Err(other) => return Err(anyhow!(other)),
    };
    if team.name.is_empty() {
        team.name = team_name.clone();
    }
    if team.description.is_none() {
        team.description = registration.objective.clone();
    }
    if team.team_allowed_paths.is_empty() {
        team.team_allowed_paths.push(TeamAllowedPath {
            path: registration.cwd.to_string_lossy().to_string(),
            read_only: false,
        });
    }
    if registration.agent_name == team.lead_agent_id {
        return Err(anyhow!(
            "agent '{}' cannot reuse the team lead name",
            registration.agent_name
        ));
    }

    let agent_id = format!("agent-{}", Uuid::new_v4().simple());
    let spawn_config = SpawnConfig {
        agent_id: agent_id.clone(),
        agent_name: registration.agent_name.clone(),
        team_name: team_name.clone(),
        model: registration.model.clone(),
        cwd: registration.cwd.to_string_lossy().to_string(),
        backend_type: BackendType::InProcess,
        env_vars: Vec::new(),
        permission_mode: registration.permission_mode,
        worktree_path: None,
    };
    let runner = Arc::new(InProcessRunner::new());
    let pane = runner
        .start(&spawn_config)
        .await
        .map_err(|error| anyhow!(error))?;

    let mut member = TeamMember::new(
        agent_id.clone(),
        registration.agent_name.clone(),
        pane.pane_id.clone(),
        registration.cwd.to_string_lossy().to_string(),
    );
    member.agent_type = Some(registration.agent_type.clone());
    member.model = registration.model.clone();
    member.backend_type = Some(BackendType::InProcess);
    member.is_active = Some(true);
    member.mode = registration.permission_mode;

    team.members.retain(|existing| existing.name != member.name);
    team.members.push(member);

    match team_helpers::read_team(&team_name).await {
        Ok(_) => team_helpers::update_team(&team)
            .await
            .with_context(|| format!("failed to update live teammate in team '{team_name}'"))?,
        Err(SwarmError::TeamNotFound(_)) => team_helpers::create_team(&team)
            .await
            .with_context(|| format!("failed to create live teammate team '{team_name}'"))?,
        Err(other) => return Err(anyhow!(other)),
    }

    LIVE_TEAMMATE_RUNNERS
        .lock()
        .expect("live teammate runner lock poisoned")
        .insert(
            live_runner_key(&team_name, &registration.agent_name),
            runner,
        );

    Ok(LiveTeammateHandle {
        team_name,
        agent_name: registration.agent_name.clone(),
        agent_id,
        pane_id: pane.pane_id,
    })
}

pub(crate) async fn finish_live_teammate(handle: &LiveTeammateHandle) -> Result<()> {
    let runner = LIVE_TEAMMATE_RUNNERS
        .lock()
        .expect("live teammate runner lock poisoned")
        .remove(&live_runner_key(&handle.team_name, &handle.agent_name));
    if let Some(runner) = runner {
        let _ = runner.stop().await;
    }

    let mut team = match team_helpers::read_team(&handle.team_name).await {
        Ok(team) => team,
        Err(SwarmError::TeamNotFound(_)) => return Ok(()),
        Err(other) => return Err(anyhow!(other)),
    };
    if let Some(member) = team.find_member_mut(&handle.agent_name) {
        member.is_active = Some(false);
        member.session_id = Some(handle.agent_id.clone());
        member.pane_id = handle.pane_id.clone();
        team_helpers::update_team(&team)
            .await
            .with_context(|| format!("failed to update teammate '{}'", handle.agent_name))?;
    }
    Ok(())
}

pub(crate) async fn create_team(input: &Value, cwd: &Path) -> Result<String> {
    let requested = requested_team_name(input).ok_or_else(|| anyhow!("team_name is required"))?;
    let requested = sanitize_team_name(&requested);
    let description = requested_description(input);
    team_helpers::validate_team_name(&requested).map_err(anyhow::Error::from)?;

    if let Some(existing_team_name) = current_session_team_name()? {
        match team_helpers::read_team(&existing_team_name).await {
            Ok(_) => {
                return Err(anyhow!(
                    "Already leading team \"{existing_team_name}\". A leader can only manage one team at a time. Use TeamDelete to end the current team before creating a new one."
                ));
            }
            Err(SwarmError::TeamNotFound(_)) => {
                tasks::clear_leader_team_name()?;
            }
            Err(other) => return Err(anyhow!(other)),
        }
    }

    let team_name = unique_team_name(&requested).await?;
    let mut team = TeamFile::new(&team_name, claude_swarm::constants::TEAM_LEAD_NAME);
    team.name = team_name.clone();
    team.lead_agent_id = claude_swarm::constants::TEAM_LEAD_NAME.to_owned();
    team.description = description.clone();
    team.members.clear();
    team.hidden_pane_ids.clear();
    team.team_allowed_paths = vec![TeamAllowedPath {
        path: cwd.to_string_lossy().to_string(),
        read_only: false,
    }];
    team_helpers::create_team(&team)
        .await
        .with_context(|| format!("failed to create team '{}'", team.name))?;

    tasks::reset_task_list(&team.name)
        .with_context(|| format!("failed to reset task list for team '{}'", team.name))?;
    tasks::set_leader_team_name(Some(team.name.clone()))
        .with_context(|| format!("failed to set leader team name for '{}'", team.name))?;

    Ok(json!({
        "team_name": team.name,
        "team_file_path": team_helpers::team_file_path(&team.name).to_string_lossy().to_string(),
        "lead_agent_id": team.lead_agent_id,
    })
    .to_string())
}

pub(crate) fn peer_entries(team: &TeamFile) -> Vec<Value> {
    let mut peers = Vec::with_capacity(team.members.len() + 1);
    peers.push(json!({
        "name": team.lead_agent_id,
        "role": "lead",
        "team": team.name,
        "is_lead": true,
        "cwd": Value::Null,
        "active": true,
    }));
    peers.extend(team.members.iter().map(|member| {
        json!({
            "name": member.name,
            "role": member.agent_type.as_deref().unwrap_or("worker"),
            "team": team.name,
            "is_lead": false,
            "cwd": member.cwd,
            "active": member.is_active.unwrap_or(false),
            "model": member.model,
            "color": member.color,
        })
    }));
    peers
}

async fn detail_status(team: &TeamFile) -> Result<Value> {
    let lead_unread = unread_count(&team.name, &team.lead_agent_id).await?;
    let mut members = Vec::with_capacity(team.members.len());
    for member in &team.members {
        members.push(json!({
            "name": member.name,
            "role": member.agent_type.as_deref().unwrap_or("worker"),
            "cwd": member.cwd,
            "active": member.is_active.unwrap_or(false),
            "model": member.model,
            "color": member.color,
            "unread_messages": unread_count(&team.name, &member.name).await?,
        }));
    }
    Ok(json!({
        "team_name": team.name,
        "objective": objective_from_team(team),
        "lead": {
            "name": team.lead_agent_id,
            "unread_messages": lead_unread,
        },
        "members": members,
        "member_count": team.members.len(),
        "active_member_count": team.active_member_count(),
    }))
}

async fn summary_status(team: &TeamFile) -> Result<Value> {
    let mut unread_members = 0usize;
    for member in &team.members {
        unread_members += unread_count(&team.name, &member.name).await?;
    }
    let lead_unread = unread_count(&team.name, &team.lead_agent_id).await?;
    Ok(json!({
        "team_name": team.name,
        "objective": objective_from_team(team),
        "lead": team.lead_agent_id,
        "member_count": team.members.len(),
        "active_member_count": team.active_member_count(),
        "unread_messages": unread_members + lead_unread,
    }))
}

pub(crate) async fn team_status(input: &Value) -> Result<String> {
    if let Some(explicit) = requested_team_name(input) {
        let team = load_team(&explicit).await?;
        return Ok(json!({
            "type": "team_status",
            "count": 1,
            "teams": [detail_status(&team).await?],
        })
        .to_string());
    }

    let teams = all_team_names().await?;
    if teams.is_empty() {
        return Ok(json!({
            "type": "team_status",
            "teams": [],
            "count": 0,
            "message": "No active team in current context. Use team_create to create a team."
        })
        .to_string());
    }

    if teams.len() == 1 {
        let team = load_team(&teams[0]).await?;
        return Ok(json!({
            "type": "team_status",
            "count": 1,
            "teams": [detail_status(&team).await?],
        })
        .to_string());
    }

    let mut summaries = Vec::with_capacity(teams.len());
    for team_name in teams {
        let team = load_team(&team_name).await?;
        summaries.push(summary_status(&team).await?);
    }
    let count = summaries.len();
    Ok(json!({
        "type": "team_status",
        "teams": summaries,
        "count": count,
    })
    .to_string())
}

pub(crate) async fn list_peers(input: &Value) -> Result<String> {
    let peers = if let Some(explicit) = requested_team_name(input) {
        let team = load_team(&explicit).await?;
        peer_entries(&team)
    } else {
        let teams = all_team_names().await?;
        if teams.is_empty() {
            Vec::new()
        } else if teams.len() == 1 {
            let team = load_team(&teams[0]).await?;
            peer_entries(&team)
        } else {
            let mut all_peers = Vec::new();
            for team_name in teams {
                let team = load_team(&team_name).await?;
                all_peers.extend(peer_entries(&team));
            }
            all_peers
        }
    };

    if peers.is_empty() {
        Ok(json!({
            "peers": [],
            "count": 0,
            "message": "No peers registered in current context. Use team_create to create a team."
        })
        .to_string())
    } else {
        let count = peers.len();
        Ok(json!({
            "peers": peers,
            "count": count,
        })
        .to_string())
    }
}
