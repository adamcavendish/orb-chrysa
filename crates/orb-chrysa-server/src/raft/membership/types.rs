use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) static MEMBERSHIP_CHANGE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum MembershipError {
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

// ── Types ──

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Leader,
    Follower,
    Candidate,
    Learner,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinResult {
    Ok,
    NotLeader,
    AlreadyMember,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeaveResult {
    Ok,
    NotLeader,
    LastVoter,
    NotMember,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_id: u64,
    pub addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinResponse {
    pub result: JoinResult,
    pub leader_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaveRequest {
    pub node_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaveResponse {
    pub result: LeaveResult,
    pub leader_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatus {
    pub node_id: u64,
    pub state: NodeState,
    pub leader_id: Option<u64>,
    pub leader_addr: Option<String>,
    pub voters: Vec<NodeInfo>,
    pub learners: Vec<NodeInfo>,
    pub term: u64,
    pub last_log_index: Option<u64>,
    pub last_applied_log: Option<u64>,
    pub last_membership_log_id: Option<u64>,
    pub millis_since_quorum_ack: Option<u64>,
    pub replication: BTreeMap<u64, Option<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: u64,
    pub addr: String,
}

// ── Server handlers ──
