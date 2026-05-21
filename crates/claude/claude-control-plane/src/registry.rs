//! In-memory registry for runners, sessions, approvals, and artifacts.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use claude_runner::{
    ApprovalCreateRequest, ApprovalDecisionRequest, ApprovalRequestRecord, ApprovalState,
    RunnerHeartbeat, RunnerRegistrationRequest, RunnerSessionRecord, RunnerSnapshot, RunnerState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

use crate::helpers::{
    runner_can_host, runner_rank, sanitize_artifact_component, session_state_after_approval,
    session_state_from_runner,
};
use crate::types::{
    ApiError, ArtifactCreateRequest, ArtifactRecord, BootstrapClaimRequest, CreateSessionRequest,
    DEFAULT_PAIRING_TTL_SECS, DeviceKind, ListSessionsQuery, MAX_PAIRING_TTL_SECS,
    PairingAcceptRequest, PairingOfferCreateRequest, PushPlatform, PushTokenRegistrationRequest,
    RunnerQueuedCommand, RunnerQueuedCommandBody, SessionRecord, SessionState,
    SessionStateTransition, TimelineEvent, TimelineEventDraft, TrustedDeviceRecord,
};

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct Registry {
    pub(crate) runners: BTreeMap<String, RunnerSnapshot>,
    pub(crate) sessions: BTreeMap<Uuid, SessionRecord>,
    pub(crate) approvals: BTreeMap<Uuid, ApprovalRequestRecord>,
    pub(crate) artifacts: BTreeMap<Uuid, ArtifactRecord>,
    #[serde(default)]
    pub(crate) trusted_devices: BTreeMap<Uuid, StoredTrustedDevice>,
    #[serde(default)]
    pub(crate) pairing_offers: BTreeMap<Uuid, StoredPairingOffer>,
    #[serde(default)]
    pub(crate) owner_device_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) queued_runner_commands: BTreeMap<String, VecDeque<RunnerQueuedCommand>>,
    /// Push tokens keyed by device_id for sending push notifications.
    #[serde(default)]
    pub(crate) push_tokens: BTreeMap<Uuid, StoredPushToken>,
    /// Push tokens keyed by user_id for tenant-based push notifications.
    #[serde(default)]
    pub(crate) user_push_tokens: BTreeMap<String, UserPushToken>,
    /// Maps runner_id to owner_user_id for tenant isolation.
    /// Populated when a runner registers via `AuthPrincipal::User`.
    #[serde(default)]
    pub(crate) runner_owners: BTreeMap<String, String>,
}

/// Stored push notification token for a trusted device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredPushToken {
    pub(crate) device_id: Uuid,
    pub(crate) push_token: String,
    pub(crate) platform: PushPlatform,
    pub(crate) registered_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

/// Push notification token keyed by tenant user_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserPushToken {
    pub(crate) user_id: String,
    pub(crate) push_token: String,
    pub(crate) platform: PushPlatform,
    pub(crate) registered_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredTrustedDevice {
    pub(crate) device_id: Uuid,
    pub(crate) name: String,
    pub(crate) kind: DeviceKind,
    pub(crate) owner: bool,
    pub(crate) created_by_device_id: Option<Uuid>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) last_seen_at: DateTime<Utc>,
    /// Hash of the refresh token (long-lived, used to obtain new access tokens).
    pub(crate) token_hash: String,
    /// Hash of the current access token (short-lived, 15 minutes).
    #[serde(default)]
    pub(crate) access_token_hash: Option<String>,
    /// When the current access token expires.
    #[serde(default)]
    pub(crate) access_token_expires_at: Option<DateTime<Utc>>,
}

impl StoredTrustedDevice {
    fn public_record(&self) -> TrustedDeviceRecord {
        TrustedDeviceRecord {
            device_id: self.device_id,
            name: self.name.clone(),
            kind: self.kind,
            owner: self.owner,
            created_by_device_id: self.created_by_device_id,
            created_at: self.created_at,
            last_seen_at: self.last_seen_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredPairingOffer {
    pub(crate) offer_id: Uuid,
    pub(crate) created_by_device_id: Option<Uuid>,
    pub(crate) device_name: String,
    pub(crate) device_kind: DeviceKind,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) pairing_secret_hash: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedSession {
    pub(crate) record: SessionRecord,
    pub(crate) owner_runner: Option<RunnerSnapshot>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingSessionDispatch {
    pub(crate) session_id: Uuid,
    pub(crate) workspace_id: String,
    pub(crate) metadata: BTreeMap<String, String>,
    pub(crate) runner: RunnerSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedApproval {
    pub(crate) approval: ApprovalRequestRecord,
    pub(crate) owner_runner: Option<RunnerSnapshot>,
    pub(crate) next_session_state: SessionState,
    pub(crate) transition: Option<SessionStateTransition>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedApprovalDecision {
    pub(crate) approval: ApprovalRequestRecord,
    pub(crate) owner_runner: Option<RunnerSnapshot>,
    pub(crate) next_session_state: Option<SessionState>,
    pub(crate) transition: Option<SessionStateTransition>,
}

// ---------------------------------------------------------------------------
// TimelineStore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct TimelineStore {
    history_limit: usize,
    tx: broadcast::Sender<TimelineEvent>,
    inner: Arc<Mutex<TimelineSnapshot>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TimelineSnapshot {
    next_sequence: u64,
    history: VecDeque<TimelineEvent>,
}

impl TimelineSnapshot {
    /// Return an iterator over the historical timeline events.
    pub(crate) fn history(&self) -> impl Iterator<Item = &TimelineEvent> {
        self.history.iter()
    }
}

impl TimelineStore {
    pub(crate) fn new(history_limit: usize, buffer: usize) -> Self {
        let (tx, _) = broadcast::channel(buffer.max(1));
        Self {
            history_limit: history_limit.max(1),
            tx,
            inner: Arc::new(Mutex::new(TimelineSnapshot {
                next_sequence: 1,
                history: VecDeque::with_capacity(history_limit.max(1)),
            })),
        }
    }

    pub(crate) fn from_snapshot(
        history_limit: usize,
        buffer: usize,
        snapshot: TimelineSnapshot,
    ) -> Self {
        let (tx, _) = broadcast::channel(buffer.max(1));
        Self {
            history_limit: history_limit.max(1),
            tx,
            inner: Arc::new(Mutex::new(snapshot)),
        }
    }

    pub(crate) async fn snapshot(&self) -> TimelineSnapshot {
        self.inner.lock().await.clone()
    }

    pub(crate) async fn publish(&self, draft: TimelineEventDraft) -> TimelineEvent {
        let event = {
            let mut timeline = self.inner.lock().await;
            let event = TimelineEvent {
                sequence: timeline.next_sequence,
                recorded_at: Utc::now(),
                runner_id: draft.runner_id,
                session_id: draft.session_id,
                detail: draft.detail,
            };
            timeline.next_sequence += 1;
            timeline.history.push_back(event.clone());
            while timeline.history.len() > self.history_limit {
                let _ = timeline.history.pop_front();
            }
            event
        };
        let _ = self.tx.send(event.clone());
        event
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<TimelineEvent> {
        self.tx.subscribe()
    }
}

fn sha256_hex(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        #[allow(clippy::format_push_string)]
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn constant_time_hash_eq(actual: &str, expected: &str) -> bool {
    use sha2::{Digest, Sha256};

    let actual_digest: [u8; 32] = Sha256::digest(actual.as_bytes()).into();
    let expected_digest: [u8; 32] = Sha256::digest(expected.as_bytes()).into();
    constant_time_eq::constant_time_eq_32(&actual_digest, &expected_digest)
}

fn mint_secret(prefix: &str) -> String {
    format!(
        "{prefix}_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn normalize_device_name(raw: &str) -> Result<String, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request(
            "device name cannot be empty".to_owned(),
        ));
    }
    let normalized = trimmed.chars().take(128).collect::<String>();
    if normalized.is_empty() {
        return Err(ApiError::bad_request(
            "device name cannot be empty".to_owned(),
        ));
    }
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Registry impl
// ---------------------------------------------------------------------------

impl Registry {
    pub(crate) fn owner_claimed(&self) -> bool {
        self.owner_device_id.is_some()
    }

    pub(crate) fn trusted_device_count(&self) -> usize {
        self.trusted_devices.len()
    }

    pub(crate) fn list_trusted_devices(&self) -> Vec<TrustedDeviceRecord> {
        self.trusted_devices
            .values()
            .map(StoredTrustedDevice::public_record)
            .collect()
    }

    pub(crate) fn revoke_device(
        &mut self,
        device_id: Uuid,
    ) -> Result<TrustedDeviceRecord, ApiError> {
        let removed = self
            .trusted_devices
            .remove(&device_id)
            .ok_or_else(|| ApiError::not_found(format!("device `{device_id}` was not found")))?;
        if removed.owner {
            self.owner_device_id = None;
        }
        // Also remove any push tokens for this device
        self.push_tokens.remove(&device_id);
        Ok(removed.public_record())
    }

    /// Authenticate using either an access token (short-lived) or a refresh token
    /// (long-lived). Access tokens are preferred; refresh tokens are accepted as
    /// a fallback so existing clients aren't locked out during migration.
    pub(crate) fn authenticate_device_token(
        &mut self,
        token: &str,
    ) -> Option<(TrustedDeviceRecord, bool)> {
        let hash = sha256_hex(token);
        let now = Utc::now();

        // Try access token first.
        for device in self.trusted_devices.values() {
            if let Some(ref at_hash) = device.access_token_hash
                && constant_time_hash_eq(&hash, at_hash)
            {
                let expired = device.access_token_expires_at.is_none_or(|exp| now >= exp);
                if !expired {
                    let device_id = device.device_id;
                    let dev = self.trusted_devices.get_mut(&device_id)?;
                    dev.last_seen_at = now;
                    return Some((dev.public_record(), true));
                }
            }
        }

        // Fallback: refresh token.
        let device_id = self
            .trusted_devices
            .values()
            .find(|device| constant_time_hash_eq(&hash, &device.token_hash))
            .map(|device| device.device_id)?;
        let device = self.trusted_devices.get_mut(&device_id)?;
        device.last_seen_at = now;
        Some((device.public_record(), false))
    }

    /// Refresh an access token using a valid refresh token.
    /// Returns `(device_record, new_access_token)` or an error.
    pub(crate) fn refresh_access_token(
        &mut self,
        refresh_token: &str,
    ) -> Result<(TrustedDeviceRecord, String), ApiError> {
        let hash = sha256_hex(refresh_token);
        let now = Utc::now();

        let device_id = self
            .trusted_devices
            .values()
            .find(|device| constant_time_hash_eq(&hash, &device.token_hash))
            .map(|device| device.device_id)
            .ok_or_else(|| ApiError::unauthorized("invalid or expired refresh token".to_owned()))?;

        let new_access_token = mint_secret("rcat");
        let access_hash = sha256_hex(&new_access_token);
        let expires_at = now + Duration::minutes(15);

        let device = self.trusted_devices.get_mut(&device_id).ok_or_else(|| {
            ApiError::internal("device disappeared during token refresh".to_owned())
        })?;
        device.access_token_hash = Some(access_hash);
        device.access_token_expires_at = Some(expires_at);
        device.last_seen_at = now;

        Ok((device.public_record(), new_access_token))
    }
}

/// Dual-token response for bootstrap and pairing.
pub(crate) struct DualTokenResponse {
    pub(crate) device: TrustedDeviceRecord,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
}

impl Registry {
    pub(crate) fn bootstrap_claim(
        &mut self,
        expected_secret_hash: Option<&str>,
        request: BootstrapClaimRequest,
    ) -> Result<DualTokenResponse, ApiError> {
        if self.owner_claimed() {
            return Err(ApiError::conflict(
                "the control plane owner device has already been claimed".to_owned(),
            ));
        }
        let Some(expected_secret_hash) = expected_secret_hash else {
            return Err(ApiError::service_unavailable(
                "bootstrap claiming is disabled because no bootstrap secret is configured"
                    .to_owned(),
            ));
        };
        if !constant_time_hash_eq(
            &sha256_hex(request.bootstrap_secret.trim()),
            expected_secret_hash,
        ) {
            return Err(ApiError::unauthorized(
                "bootstrap secret is missing or invalid".to_owned(),
            ));
        }

        let now = Utc::now();
        let refresh_token = mint_secret("rcrt");
        let access_token = mint_secret("rcat");
        let device_id = Uuid::new_v4();
        let record = StoredTrustedDevice {
            device_id,
            name: normalize_device_name(&request.device_name)?,
            kind: request.device_kind,
            owner: true,
            created_by_device_id: None,
            created_at: now,
            last_seen_at: now,
            token_hash: sha256_hex(&refresh_token),
            access_token_hash: Some(sha256_hex(&access_token)),
            access_token_expires_at: Some(now + Duration::minutes(15)),
        };
        self.owner_device_id = Some(device_id);
        self.trusted_devices.insert(device_id, record.clone());
        Ok(DualTokenResponse {
            device: record.public_record(),
            access_token,
            refresh_token,
        })
    }

    pub(crate) fn create_pairing_offer(
        &mut self,
        created_by_device_id: Option<Uuid>,
        request: PairingOfferCreateRequest,
    ) -> Result<(StoredPairingOffer, String), ApiError> {
        if !self.owner_claimed() {
            return Err(ApiError::conflict(
                "claim the owner device before creating pairing offers".to_owned(),
            ));
        }
        self.prune_expired_pairing_offers();

        let expires_in_secs = request
            .expires_in_secs
            .unwrap_or(DEFAULT_PAIRING_TTL_SECS)
            .clamp(60, MAX_PAIRING_TTL_SECS);
        let created_at = Utc::now();
        let pairing_secret = mint_secret("rcpo");
        let offer = StoredPairingOffer {
            offer_id: Uuid::new_v4(),
            created_by_device_id,
            device_name: normalize_device_name(&request.device_name)?,
            device_kind: request.device_kind,
            created_at,
            expires_at: created_at + Duration::seconds(expires_in_secs as i64),
            pairing_secret_hash: sha256_hex(&pairing_secret),
        };
        self.pairing_offers.insert(offer.offer_id, offer.clone());
        Ok((offer, pairing_secret))
    }

    pub(crate) fn accept_pairing_offer(
        &mut self,
        request: PairingAcceptRequest,
    ) -> Result<DualTokenResponse, ApiError> {
        self.prune_expired_pairing_offers();

        let offer = self
            .pairing_offers
            .remove(&request.offer_id)
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "pairing offer `{}` was not found",
                    request.offer_id
                ))
            })?;
        if offer.expires_at < Utc::now() {
            return Err(ApiError::conflict(format!(
                "pairing offer `{}` has expired",
                request.offer_id
            )));
        }
        if !constant_time_hash_eq(
            &sha256_hex(request.pairing_secret.trim()),
            &offer.pairing_secret_hash,
        ) {
            return Err(ApiError::unauthorized(
                "pairing secret is missing or invalid".to_owned(),
            ));
        }

        let now = Utc::now();
        let refresh_token = mint_secret("rcrt");
        let access_token = mint_secret("rcat");
        let record = StoredTrustedDevice {
            device_id: Uuid::new_v4(),
            name: normalize_device_name(
                request
                    .device_name
                    .as_deref()
                    .unwrap_or(offer.device_name.as_str()),
            )?,
            kind: request.device_kind.unwrap_or(offer.device_kind),
            owner: false,
            created_by_device_id: offer.created_by_device_id,
            created_at: now,
            last_seen_at: now,
            token_hash: sha256_hex(&refresh_token),
            access_token_hash: Some(sha256_hex(&access_token)),
            access_token_expires_at: Some(now + Duration::minutes(15)),
        };
        let public_record = record.public_record();
        let device_id = record.device_id;
        self.trusted_devices.insert(device_id, record);
        Ok(DualTokenResponse {
            device: public_record,
            access_token,
            refresh_token,
        })
    }

    fn prune_expired_pairing_offers(&mut self) {
        let now = Utc::now();
        self.pairing_offers
            .retain(|_, offer| offer.expires_at >= now);
        // Also remove orphaned runner ownership records.
        self.runner_owners
            .retain(|runner_id, _| self.runners.contains_key(runner_id));
    }

    pub(crate) fn queued_runner_command_count(&self) -> usize {
        self.queued_runner_commands
            .values()
            .map(VecDeque::len)
            .sum::<usize>()
    }

    // -----------------------------------------------------------------------
    // Tenant isolation helpers
    // -----------------------------------------------------------------------

    /// Check if a runner belongs to the given user (or has no owner).
    /// Returns `true` when `owner_user_id` is `None` (admin — sees all).
    pub(crate) fn runner_visible_to(&self, runner_id: &str, owner_user_id: Option<&str>) -> bool {
        match owner_user_id {
            None => true,
            Some(uid) => self
                .runner_owners
                .get(runner_id)
                .is_some_and(|owner| owner == uid),
        }
    }

    /// Check if a session belongs to the given user.
    /// Returns `true` when `owner_user_id` is `None` (admin — sees all)
    /// or when the session has no `owner_user_id` (legacy session).
    pub(crate) fn session_visible_to(
        &self,
        session: &SessionRecord,
        owner_user_id: Option<&str>,
    ) -> bool {
        match owner_user_id {
            None => true,
            Some(uid) => session.owner_user_id.as_deref() == Some(uid),
        }
    }

    /// List runners visible to the given user (tenant-filtered).
    pub(crate) fn list_runners_for_user(&self, owner_user_id: Option<&str>) -> Vec<RunnerSnapshot> {
        self.runners
            .values()
            .filter(|snapshot| {
                self.runner_visible_to(&snapshot.registration.runner_id, owner_user_id)
            })
            .cloned()
            .collect()
    }

    /// List approvals visible to the given user (tenant-filtered).
    pub(crate) fn list_approvals_for_user(
        &self,
        owner_user_id: Option<&str>,
    ) -> Vec<ApprovalRequestRecord> {
        self.approvals
            .values()
            .filter(|approval| {
                // Filter by the session's owner_user_id.
                owner_user_id.is_none_or(|uid| {
                    self.sessions
                        .get(&approval.session_id)
                        .is_some_and(|session| session.owner_user_id.as_deref() == Some(uid))
                })
            })
            .cloned()
            .collect()
    }

    /// List artifacts visible to the given user (tenant-filtered).
    pub(crate) fn list_artifacts_for_user(
        &self,
        owner_user_id: Option<&str>,
    ) -> Vec<ArtifactRecord> {
        self.artifacts
            .values()
            .filter(|artifact| {
                owner_user_id.is_none_or(|uid| {
                    self.sessions
                        .get(&artifact.session_id)
                        .is_some_and(|session| session.owner_user_id.as_deref() == Some(uid))
                })
            })
            .cloned()
            .collect()
    }

    /// Verify that a session belongs to the given user (or user is admin).
    /// Returns `ApiError::not_found` if the session doesn't exist or isn't visible.
    pub(crate) fn get_session_for_user(
        &self,
        session_id: Uuid,
        owner_user_id: Option<&str>,
    ) -> Result<SessionRecord, ApiError> {
        let session = self.get_session(session_id)?;
        if !self.session_visible_to(&session, owner_user_id) {
            return Err(ApiError::not_found(format!(
                "session `{session_id}` was not found"
            )));
        }
        Ok(session)
    }

    pub(crate) fn register_runner(
        &mut self,
        request: RunnerRegistrationRequest,
        lease_ttl_secs: u64,
        owner_user_id: Option<&str>,
    ) -> crate::types::RunnerRegistrationResponse {
        let now = Utc::now();
        let runner_id = request.runner_id.clone();
        let snapshot = RunnerSnapshot {
            registration: request.clone(),
            state: RunnerState::Idle,
            active_sessions: 0,
            queued_sessions: 0,
            registered_at: now,
            last_seen_at: now,
        };
        self.runners.insert(runner_id.clone(), snapshot);

        // Track tenant ownership for data isolation.
        if let Some(user_id) = owner_user_id {
            self.runner_owners
                .insert(runner_id.clone(), user_id.to_owned());
        }

        // Recalculate session counts from existing sessions so that a
        // runner re-registering (e.g. after a brief restart) does not
        // appear to have zero sessions, which would allow the dispatch
        // loop to over-assign work beyond its capacity.
        self.refresh_runner_session_counts(&runner_id, now);

        // The runner was inserted above; this is a guaranteed lookup.
        let snapshot = self
            .runners
            .get(&runner_id)
            .cloned()
            .expect("runner was just inserted");
        crate::types::RunnerRegistrationResponse {
            runner_id,
            registered_at: now,
            lease_ttl_secs,
            snapshot,
        }
    }

    pub(crate) fn apply_heartbeat(
        &mut self,
        runner_id: &str,
        heartbeat: RunnerHeartbeat,
    ) -> Result<RunnerSnapshot, ApiError> {
        let snapshot = self
            .runners
            .get_mut(runner_id)
            .ok_or_else(|| ApiError::not_found(format!("runner `{runner_id}` was not found")))?;
        snapshot.state = heartbeat.state;
        snapshot.active_sessions = heartbeat.active_sessions;
        snapshot.queued_sessions = heartbeat.queued_sessions;
        snapshot.last_seen_at = heartbeat.timestamp;
        Ok(snapshot.clone())
    }

    pub(crate) fn plan_session(
        &self,
        request: &CreateSessionRequest,
        lease_ttl_secs: u64,
        owner_user_id: Option<&str>,
    ) -> Result<PlannedSession, ApiError> {
        let session_id = request.session_id.unwrap_or_else(Uuid::new_v4);
        if self.sessions.contains_key(&session_id) {
            return Err(ApiError::conflict(format!(
                "session `{session_id}` already exists"
            )));
        }
        let now = Utc::now();
        let owner_runner_id = self.select_runner(
            &request.workspace_id,
            request.preferred_runner_id.as_deref(),
            lease_ttl_secs,
            owner_user_id,
        )?;
        let state = if owner_runner_id.is_some() {
            SessionState::Assigned
        } else {
            SessionState::Pending
        };
        let record = SessionRecord {
            session_id,
            workspace_id: request.workspace_id.clone(),
            owner_runner_id: owner_runner_id.clone(),
            state,
            metadata: request.metadata.clone(),
            created_at: now,
            updated_at: now,
            owner_user_id: owner_user_id.map(str::to_owned),
        };
        let owner_runner = owner_runner_id
            .as_ref()
            .and_then(|runner_id| self.runners.get(runner_id))
            .cloned();
        Ok(PlannedSession {
            record,
            owner_runner,
        })
    }

    pub(crate) fn commit_session(
        &mut self,
        record: SessionRecord,
    ) -> Result<SessionRecord, ApiError> {
        if self.sessions.contains_key(&record.session_id) {
            return Err(ApiError::conflict(format!(
                "session `{}` already exists",
                record.session_id
            )));
        }
        self.sessions.insert(record.session_id, record.clone());
        if let Some(runner_id) = &record.owner_runner_id {
            self.refresh_runner_session_counts(runner_id, record.updated_at);
        }
        Ok(record)
    }

    pub(crate) fn get_runner_snapshot(&self, runner_id: &str) -> Result<RunnerSnapshot, ApiError> {
        self.runners
            .get(runner_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("runner `{runner_id}` was not found")))
    }

    pub(crate) fn get_session(&self, session_id: Uuid) -> Result<SessionRecord, ApiError> {
        self.sessions
            .get(&session_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))
    }

    pub(crate) fn list_sessions_filtered(
        &self,
        query: &ListSessionsQuery,
        owner_user_id: Option<&str>,
    ) -> Vec<SessionRecord> {
        self.sessions
            .values()
            .filter(|session| {
                query
                    .runner_id
                    .as_deref()
                    .is_none_or(|runner_id| session.owner_runner_id.as_deref() == Some(runner_id))
            })
            .filter(|session| {
                query
                    .workspace_id
                    .as_deref()
                    .is_none_or(|workspace_id| session.workspace_id == workspace_id)
            })
            .filter(|session| query.state.is_none_or(|state| session.state == state))
            // Tenant isolation: only return sessions belonging to the requesting user.
            .filter(|session| {
                owner_user_id.is_none_or(|uid| session.owner_user_id.as_deref() == Some(uid))
            })
            .cloned()
            .collect()
    }

    pub(crate) fn apply_session_state_update(
        &mut self,
        session_id: Uuid,
        state: SessionState,
        metadata: BTreeMap<String, String>,
        updated_at: DateTime<Utc>,
    ) -> Result<(SessionRecord, SessionState), ApiError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))?;
        let previous_state = session.state;
        session.state = state;
        session.updated_at = updated_at;
        session.metadata.extend(metadata);
        let updated = session.clone();
        let owner_runner_id = updated.owner_runner_id.clone();
        if let Some(runner_id) = owner_runner_id.as_deref() {
            self.refresh_runner_session_counts(runner_id, updated_at);
        }
        Ok((updated, previous_state))
    }

    pub(crate) fn refresh_runner_session_counts(
        &mut self,
        runner_id: &str,
        timestamp: DateTime<Utc>,
    ) {
        let (active_sessions, queued_sessions) = self
            .sessions
            .values()
            .filter(|session| session.owner_runner_id.as_deref() == Some(runner_id))
            .fold((0usize, 0usize), |(active, queued), session| {
                let active = if matches!(
                    session.state,
                    SessionState::Assigned | SessionState::Running | SessionState::WaitingApproval
                ) {
                    active + 1
                } else {
                    active
                };
                let queued = if matches!(session.state, SessionState::Pending) {
                    queued + 1
                } else {
                    queued
                };
                (active, queued)
            });

        if let Some(snapshot) = self.runners.get_mut(runner_id) {
            snapshot.active_sessions = active_sessions;
            snapshot.queued_sessions = queued_sessions;
            snapshot.state = if active_sessions > 0 {
                RunnerState::Busy
            } else {
                RunnerState::Idle
            };
            snapshot.last_seen_at = snapshot.last_seen_at.max(timestamp);
        }
    }

    /// Reap sessions stuck in `Assigned` whose runner has gone offline.
    /// Returns a list of (session_id, old_runner_id, previous_state) tuples reverted to `Pending`.
    pub(crate) fn reap_orphaned_assigned_sessions(
        &mut self,
        lease_ttl_secs: u64,
    ) -> Vec<(Uuid, String, SessionState)> {
        let now = Utc::now();
        let cutoff = now - Duration::seconds((lease_ttl_secs * 3) as i64);
        let mut reaped = Vec::new();
        let mut runners_needing_refresh: Vec<String> = Vec::new();

        for session in self.sessions.values_mut() {
            if session.state != SessionState::Assigned {
                continue;
            }
            let Some(runner_id) = session.owner_runner_id.clone() else {
                continue;
            };
            let Some(runner) = self.runners.get(&runner_id) else {
                let previous_state = session.state;
                session.state = SessionState::Pending;
                session.owner_runner_id = None;
                session.updated_at = now;
                reaped.push((session.session_id, runner_id, previous_state));
                continue;
            };
            if runner.last_seen_at < cutoff {
                let previous_state = session.state;
                session.state = SessionState::Pending;
                session.owner_runner_id = None;
                session.updated_at = now;
                runners_needing_refresh.push(runner_id.clone());
                reaped.push((session.session_id, runner_id, previous_state));
            }
        }

        for rid in runners_needing_refresh {
            self.refresh_runner_session_counts(&rid, now);
        }

        reaped
    }

    #[allow(dead_code)]
    pub(crate) fn list_approvals(&self) -> Vec<ApprovalRequestRecord> {
        self.approvals.values().cloned().collect()
    }

    #[allow(dead_code)]
    pub(crate) fn list_artifacts(&self) -> Vec<ArtifactRecord> {
        self.artifacts.values().cloned().collect()
    }

    pub(crate) fn list_runner_approvals(
        &self,
        runner_id: &str,
    ) -> Result<Vec<ApprovalRequestRecord>, ApiError> {
        if !self.runners.contains_key(runner_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
        Ok(self
            .approvals
            .values()
            .filter(|approval| approval.runner_id == runner_id)
            .cloned()
            .collect())
    }

    pub(crate) fn list_runner_artifacts(
        &self,
        runner_id: &str,
    ) -> Result<Vec<ArtifactRecord>, ApiError> {
        if !self.runners.contains_key(runner_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
        Ok(self
            .artifacts
            .values()
            .filter(|artifact| artifact.runner_id.as_deref() == Some(runner_id))
            .cloned()
            .collect())
    }

    pub(crate) fn get_artifact(&self, artifact_id: Uuid) -> Result<ArtifactRecord, ApiError> {
        self.artifacts
            .get(&artifact_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("artifact `{artifact_id}` was not found")))
    }

    pub(crate) fn get_approval(
        &self,
        approval_id: Uuid,
    ) -> Result<ApprovalRequestRecord, ApiError> {
        self.approvals
            .get(&approval_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("approval `{approval_id}` was not found")))
    }

    pub(crate) fn list_session_approvals(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<ApprovalRequestRecord>, ApiError> {
        if !self.sessions.contains_key(&session_id) {
            return Err(ApiError::not_found(format!(
                "session `{session_id}` was not found"
            )));
        }
        Ok(self
            .approvals
            .values()
            .filter(|approval| approval.session_id == session_id)
            .cloned()
            .collect())
    }

    pub(crate) fn list_session_artifacts(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<ArtifactRecord>, ApiError> {
        if !self.sessions.contains_key(&session_id) {
            return Err(ApiError::not_found(format!(
                "session `{session_id}` was not found"
            )));
        }
        Ok(self
            .artifacts
            .values()
            .filter(|artifact| artifact.session_id == session_id)
            .cloned()
            .collect())
    }

    pub(crate) fn register_artifact(
        &mut self,
        session_id: Uuid,
        request: &ArtifactCreateRequest,
        size_bytes: u64,
    ) -> Result<ArtifactRecord, ApiError> {
        let name = request.name.trim();
        if name.is_empty() {
            return Err(ApiError::bad_request(
                "artifact name cannot be empty".to_owned(),
            ));
        }
        let session =
            self.sessions.get(&session_id).cloned().ok_or_else(|| {
                ApiError::not_found(format!("session `{session_id}` was not found"))
            })?;
        let file_name = sanitize_artifact_component(
            request
                .file_name
                .as_deref()
                .unwrap_or(request.name.as_str()),
            "artifact.bin",
        );
        let media_type = request
            .media_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let artifact = ArtifactRecord {
            artifact_id: Uuid::new_v4(),
            session_id,
            runner_id: session.owner_runner_id.clone(),
            name: name.to_owned(),
            file_name,
            media_type,
            size_bytes,
            metadata: request.metadata.clone(),
            created_at: Utc::now(),
        };
        self.artifacts
            .insert(artifact.artifact_id, artifact.clone());
        Ok(artifact)
    }

    pub(crate) fn plan_approval(
        &self,
        session_id: Uuid,
        request: ApprovalCreateRequest,
    ) -> Result<PlannedApproval, ApiError> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))?;
        let now = Utc::now();
        let next_session_state = SessionState::WaitingApproval;
        let owner_runner_id = session.owner_runner_id.clone();
        let approval = ApprovalRequestRecord {
            approval_id: request.approval_id.unwrap_or_else(Uuid::new_v4),
            session_id,
            runner_id: owner_runner_id.clone().unwrap_or_default(),
            state: ApprovalState::Pending,
            title: request.title,
            description: request.description,
            metadata: request.metadata,
            created_at: now,
            updated_at: now,
            responded_at: None,
            responder: None,
            note: None,
        };
        let transition = (session.state != next_session_state).then(|| SessionStateTransition {
            runner_id: owner_runner_id.clone(),
            session_id,
            previous_state: session.state,
            state: next_session_state,
        });
        let owner_runner = owner_runner_id
            .as_ref()
            .and_then(|runner_id| self.runners.get(runner_id))
            .cloned();
        Ok(PlannedApproval {
            approval,
            owner_runner,
            next_session_state,
            transition,
        })
    }

    pub(crate) fn commit_planned_approval(
        &mut self,
        planned: PlannedApproval,
    ) -> Result<(ApprovalRequestRecord, Option<SessionStateTransition>), ApiError> {
        if self.approvals.contains_key(&planned.approval.approval_id) {
            return Err(ApiError::conflict(format!(
                "approval `{}` already exists",
                planned.approval.approval_id
            )));
        }

        let session = self
            .sessions
            .get_mut(&planned.approval.session_id)
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "session `{}` was not found",
                    planned.approval.session_id
                ))
            })?;
        session.state = planned.next_session_state;
        session.updated_at = planned.approval.updated_at;
        let owner_runner_id = session.owner_runner_id.clone();

        self.approvals
            .insert(planned.approval.approval_id, planned.approval.clone());
        if let Some(runner_id) = owner_runner_id.as_deref() {
            self.refresh_runner_session_counts(runner_id, planned.approval.updated_at);
        }

        Ok((planned.approval, planned.transition))
    }

    pub(crate) fn plan_approval_decision(
        &self,
        approval_id: Uuid,
        request: ApprovalDecisionRequest,
    ) -> Result<PlannedApprovalDecision, ApiError> {
        let approval = self.approvals.get(&approval_id).ok_or_else(|| {
            ApiError::not_found(format!("approval `{approval_id}` was not found"))
        })?;
        if !matches!(approval.state, ApprovalState::Pending) {
            return Err(ApiError::conflict(format!(
                "approval `{approval_id}` is already resolved"
            )));
        }

        let now = Utc::now();
        let mut updated = approval.clone();
        updated.state = request.decision.into();
        updated.updated_at = now;
        updated.responded_at = Some(now);
        updated.responder = request.responder;
        updated.note = request.note;

        let has_pending_approvals = self.approvals.values().any(|candidate| {
            candidate.session_id == updated.session_id
                && candidate.approval_id != updated.approval_id
                && matches!(candidate.state, ApprovalState::Pending)
        });

        let (next_session_state, transition, owner_runner) =
            if let Some(session) = self.sessions.get(&updated.session_id) {
                let state = session_state_after_approval(request.decision, has_pending_approvals);
                let owner_runner = session
                    .owner_runner_id
                    .as_ref()
                    .and_then(|runner_id| self.runners.get(runner_id))
                    .cloned();
                let transition = (session.state != state).then(|| SessionStateTransition {
                    runner_id: session.owner_runner_id.clone(),
                    session_id: session.session_id,
                    previous_state: session.state,
                    state,
                });
                (Some(state), transition, owner_runner)
            } else {
                (None, None, None)
            };

        Ok(PlannedApprovalDecision {
            approval: updated,
            owner_runner,
            next_session_state,
            transition,
        })
    }

    pub(crate) fn commit_planned_approval_decision(
        &mut self,
        planned: PlannedApprovalDecision,
    ) -> Result<(ApprovalRequestRecord, Option<SessionStateTransition>), ApiError> {
        let approval = self
            .approvals
            .get_mut(&planned.approval.approval_id)
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "approval `{}` was not found",
                    planned.approval.approval_id
                ))
            })?;
        if !matches!(approval.state, ApprovalState::Pending) {
            return Err(ApiError::conflict(format!(
                "approval `{}` is already resolved",
                planned.approval.approval_id
            )));
        }
        *approval = planned.approval.clone();
        let updated = approval.clone();

        let owner_runner_id = if let Some(session) = self.sessions.get_mut(&updated.session_id) {
            if let Some(next_state) = planned.next_session_state {
                session.state = next_state;
            }
            session.updated_at = updated.updated_at;
            session.owner_runner_id.clone()
        } else {
            None
        };
        if let Some(runner_id) = owner_runner_id.as_deref() {
            self.refresh_runner_session_counts(runner_id, updated.updated_at);
        }

        Ok((updated, planned.transition))
    }

    pub(crate) fn plan_next_pending_session_for_runner(
        &self,
        runner_id: &str,
        lease_ttl_secs: u64,
        skipped_session_ids: &BTreeSet<Uuid>,
    ) -> Result<Option<PendingSessionDispatch>, ApiError> {
        let runner = self.get_runner_snapshot(runner_id)?;
        Ok(self
            .sessions
            .values()
            .filter(|session| matches!(session.state, SessionState::Pending))
            .filter(|session| session.owner_runner_id.is_none())
            .filter(|session| !skipped_session_ids.contains(&session.session_id))
            .filter_map(|session| {
                let selected = self
                    .select_runner(
                        &session.workspace_id,
                        None,
                        lease_ttl_secs,
                        session.owner_user_id.as_deref(),
                    )
                    .ok()?;
                (selected.as_deref() == Some(runner_id)).then(|| PendingSessionDispatch {
                    session_id: session.session_id,
                    workspace_id: session.workspace_id.clone(),
                    metadata: session.metadata.clone(),
                    runner: runner.clone(),
                })
            })
            .min_by_key(|dispatch| {
                self.sessions
                    .get(&dispatch.session_id)
                    .map(|session| (session.created_at, session.session_id))
            }))
    }

    pub(crate) fn commit_pending_session_dispatch(
        &mut self,
        session_id: Uuid,
        runner_id: &str,
        dispatched: &RunnerSessionRecord,
    ) -> Result<(SessionRecord, SessionState), ApiError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` was not found")))?;
        if !matches!(session.state, SessionState::Pending) || session.owner_runner_id.is_some() {
            return Err(ApiError::conflict(format!(
                "session `{session_id}` is no longer pending dispatch"
            )));
        }

        let previous_state = session.state;
        session.owner_runner_id = Some(runner_id.to_owned());
        session.state = session_state_from_runner(dispatched.state);
        session.metadata = dispatched.metadata.clone();
        session.updated_at = dispatched.updated_at;
        let updated = session.clone();
        self.refresh_runner_session_counts(runner_id, updated.updated_at);
        Ok((updated, previous_state))
    }

    pub(crate) fn select_runner(
        &self,
        workspace_id: &str,
        preferred_runner_id: Option<&str>,
        lease_ttl_secs: u64,
        owner_user_id: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        if let Some(runner_id) = preferred_runner_id {
            let snapshot = self.runners.get(runner_id).ok_or_else(|| {
                ApiError::not_found(format!("runner `{runner_id}` was not found"))
            })?;
            if !runner_can_host(snapshot, workspace_id, lease_ttl_secs) {
                return Err(ApiError::conflict(format!(
                    "runner `{runner_id}` is not eligible for workspace `{workspace_id}`"
                )));
            }
            return Ok(Some(runner_id.to_owned()));
        }

        let selected = self
            .runners
            .values()
            .filter(|snapshot| runner_can_host(snapshot, workspace_id, lease_ttl_secs))
            // Tenant isolation: only assign to runners owned by the same user.
            .filter(|snapshot| {
                owner_user_id.is_none_or(|uid| {
                    self.runner_owners
                        .get(&snapshot.registration.runner_id)
                        .is_some_and(|owner| owner == uid)
                })
            })
            .min_by_key(|snapshot| {
                (
                    runner_rank(snapshot.state),
                    snapshot.active_sessions,
                    snapshot.registration.runner_id.as_str(),
                )
            })
            .map(|snapshot| snapshot.registration.runner_id.clone());
        Ok(selected)
    }

    pub(crate) fn enqueue_runner_command(
        &mut self,
        runner_id: &str,
        body: RunnerQueuedCommandBody,
    ) -> Result<RunnerQueuedCommand, ApiError> {
        if !self.runners.contains_key(runner_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
        let command = RunnerQueuedCommand {
            command_id: Uuid::new_v4(),
            runner_id: runner_id.to_owned(),
            created_at: Utc::now(),
            body,
        };
        self.queued_runner_commands
            .entry(runner_id.to_owned())
            .or_default()
            .push_back(command.clone());
        Ok(command)
    }

    pub(crate) fn pull_runner_commands(
        &mut self,
        runner_id: &str,
        limit: usize,
    ) -> Result<Vec<RunnerQueuedCommand>, ApiError> {
        if !self.runners.contains_key(runner_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
        let queue = self
            .queued_runner_commands
            .entry(runner_id.to_owned())
            .or_default();
        let mut commands = Vec::new();
        for _ in 0..limit.max(1) {
            let Some(command) = queue.pop_front() else {
                break;
            };
            commands.push(command);
        }
        if queue.is_empty() {
            self.queued_runner_commands.remove(runner_id);
        }
        Ok(commands)
    }

    /// Remove push token for a device (cascade on device removal).
    #[allow(dead_code)]
    pub(crate) fn remove_push_token(&mut self, device_id: Uuid) {
        self.push_tokens.remove(&device_id);
    }

    /// Register or update a push notification token for a trusted device.
    pub(crate) fn register_push_token(
        &mut self,
        device_id: Uuid,
        request: PushTokenRegistrationRequest,
    ) -> Result<bool, ApiError> {
        if !self.trusted_devices.contains_key(&device_id) {
            return Err(ApiError::not_found(format!(
                "device `{device_id}` is not a trusted device"
            )));
        }
        let now = Utc::now();
        let is_new = !self.push_tokens.contains_key(&device_id);
        self.push_tokens.insert(
            device_id,
            StoredPushToken {
                device_id,
                push_token: request.push_token,
                platform: request.platform,
                registered_at: self
                    .push_tokens
                    .get(&device_id)
                    .map(|t| t.registered_at)
                    .unwrap_or(now),
                updated_at: now,
            },
        );
        Ok(is_new)
    }

    /// Register or update a push notification token for a tenant user.
    pub(crate) fn register_user_push_token(
        &mut self,
        user_id: String,
        request: PushTokenRegistrationRequest,
    ) -> bool {
        let now = Utc::now();
        let is_new = !self.user_push_tokens.contains_key(&user_id);
        self.user_push_tokens.insert(
            user_id.clone(),
            UserPushToken {
                user_id: user_id.clone(),
                push_token: request.push_token,
                platform: request.platform,
                registered_at: self
                    .user_push_tokens
                    .get(&user_id)
                    .map(|t| t.registered_at)
                    .unwrap_or(now),
                updated_at: now,
            },
        );
        is_new
    }
}
