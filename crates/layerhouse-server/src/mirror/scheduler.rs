use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::routes::AppState;
use crate::store::blob::BlobStore;
#[allow(unused_imports)]
use crate::store::metadata::{
    JobStore, MirrorConfigStore, MirrorDirection, MirrorRule, ProxyCache, SchedulerStore, SyncJob,
    SyncJobKind, SyncJobRun, SyncJobStatus, SyncRunEventKind, SyncRunStatus, WarmImage,
    mirror_rule_job, now_epoch, proxy_cache_warm_job,
};

const TICK_INTERVAL_SECS: u64 = 15;
const MAX_CONCURRENT_JOBS: usize = 4;
const STALE_JOB_THRESHOLD_SECS: u64 = 300;
const FALLBACK_SCHEDULE_INTERVAL_SECS: u64 = 3600;

fn schedule_interval_secs(schedule: &str) -> Option<u64> {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }

    let minute = fields[0];
    if minute == "*" {
        return Some(60);
    }
    if let Some(step) = minute.strip_prefix("*/") {
        let step = step.parse::<u64>().ok()?;
        return (step > 0).then_some(step * 60);
    }
    if minute.parse::<u8>().is_ok() && fields[1] == "*" {
        return Some(3600);
    }

    Some(FALLBACK_SCHEDULE_INTERVAL_SECS)
}

fn scheduled_mirror_job(rule: &MirrorRule, now: u64) -> Option<SyncJob> {
    let schedule = rule.schedule.as_deref()?;
    let interval_secs = schedule_interval_secs(schedule)?;
    Some(mirror_rule_job(
        rule,
        format!("mirror-rule-{}", rule.id),
        now,
        interval_secs,
    ))
}

fn scheduled_proxy_cache_job(cache: &ProxyCache, now: u64) -> Option<SyncJob> {
    let schedule = cache.warm_schedule.as_deref()?;
    let interval_secs = schedule_interval_secs(schedule)?;
    let mut job = proxy_cache_warm_job(cache, now);
    job.id = format!("proxy-cache-{}-warm", cache.id);
    job.interval_secs = interval_secs;
    Some(job)
}

fn reconcile_job(existing: &SyncJob, desired: &SyncJob) -> Option<SyncJob> {
    if existing.status == SyncJobStatus::Running {
        return None;
    }
    if existing.kind != desired.kind
        || existing.rule_id != desired.rule_id
        || existing.rule_name != desired.rule_name
        || existing.image != desired.image
        || existing.tags != desired.tags
        || existing.interval_secs != desired.interval_secs
    {
        let mut updated = desired.clone();
        updated.status = existing.status.clone();
        updated.claimed_by = existing.claimed_by.clone();
        updated.claimed_at = existing.claimed_at;
        updated.last_run_at = existing.last_run_at;
        updated.last_error = existing.last_error.clone();
        updated.next_run_at = existing.next_run_at.min(desired.next_run_at);
        return Some(updated);
    }
    None
}

pub async fn run_scheduler<M: SchedulerStore, B: BlobStore>(
    state: Arc<AppState<M, B>>,
    node_id: String,
) {
    tracing::info!(
        "scheduler started (node_id={})",
        &node_id[..node_id.len().min(8)]
    );

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS));

    loop {
        tokio::time::sleep(Duration::from_secs(TICK_INTERVAL_SECS)).await;

        if let Err(e) = reconcile_jobs(&state).await {
            tracing::error!("scheduler: reconcile failed: {}", e);
            continue;
        }

        if let Err(e) = recover_stale_jobs(&state).await {
            tracing::error!("scheduler: stale recovery failed: {}", e);
        }

        execute_due_jobs(&state, &node_id, &semaphore).await;
    }
}

async fn reconcile_jobs<M: SchedulerStore, B: BlobStore>(
    state: &AppState<M, B>,
) -> Result<(), crate::error::LayerhouseError> {
    let warm_images = state.core.metadata.list_warm_images().await?;
    let mirror_rules = state.core.metadata.list_mirror_rules().await?;
    let proxy_caches = state.core.metadata.list_proxy_caches().await?;
    let existing_jobs = state.core.metadata.list_sync_jobs().await?;

    let warm_map: std::collections::HashMap<&str, &WarmImage> =
        warm_images.iter().map(|w| (w.id.as_str(), w)).collect();
    let job_map: std::collections::HashMap<&str, &SyncJob> =
        existing_jobs.iter().map(|j| (j.id.as_str(), j)).collect();
    let now = now_epoch();

    let mut desired_scheduled = std::collections::BTreeSet::new();

    for rule in &mirror_rules {
        let Some(job) = scheduled_mirror_job(rule, now) else {
            continue;
        };
        desired_scheduled.insert(job.id.clone());
        if let Some(existing) = job_map.get(job.id.as_str()) {
            if let Some(updated) = reconcile_job(existing, &job) {
                state.core.metadata.put_sync_job(updated).await?;
            }
        } else {
            state.core.metadata.put_sync_job(job).await?;
        }
    }

    for cache in &proxy_caches {
        let Some(job) = scheduled_proxy_cache_job(cache, now) else {
            continue;
        };
        desired_scheduled.insert(job.id.clone());
        if let Some(existing) = job_map.get(job.id.as_str()) {
            if let Some(updated) = reconcile_job(existing, &job) {
                state.core.metadata.put_sync_job(updated).await?;
            }
        } else {
            state.core.metadata.put_sync_job(job).await?;
        }
    }

    for warm in &warm_images {
        if let Some(existing) = job_map.get(warm.id.as_str()) {
            if existing.status == SyncJobStatus::Running {
                continue;
            }
            if existing.image != warm.image
                || existing.tags != warm.tags
                || existing.interval_secs != warm.interval_secs
            {
                let mut updated = (*existing).clone();
                updated.image = warm.image.clone();
                updated.tags = warm.tags.clone();
                updated.interval_secs = warm.interval_secs;
                state.core.metadata.put_sync_job(updated).await?;
            }
        } else {
            let job = SyncJob {
                id: warm.id.clone(),
                kind: SyncJobKind::LegacyWarm,
                rule_id: Some(warm.id.clone()),
                rule_name: Some(warm.id.clone()),
                image: warm.image.clone(),
                tags: warm.tags.clone(),
                interval_secs: warm.interval_secs,
                status: SyncJobStatus::Idle,
                claimed_by: None,
                claimed_at: None,
                last_run_at: None,
                next_run_at: now_epoch(),
                last_error: None,
            };
            state.core.metadata.put_sync_job(job).await?;
        }
    }

    for job in &existing_jobs {
        let stale_legacy =
            job.kind == SyncJobKind::LegacyWarm && !warm_map.contains_key(job.id.as_str());
        let stale_scheduled = matches!(job.kind, SyncJobKind::Mirror | SyncJobKind::ProxyCache)
            && job.interval_secs > 0
            && !desired_scheduled.contains(&job.id);
        if job.status != SyncJobStatus::Running && (stale_legacy || stale_scheduled) {
            state.core.metadata.delete_sync_job(&job.id).await?;
        }
    }

    Ok(())
}

async fn recover_stale_jobs<M: SchedulerStore, B: BlobStore>(
    state: &AppState<M, B>,
) -> Result<(), crate::error::LayerhouseError> {
    let jobs = state.core.metadata.list_sync_jobs().await?;
    let now = now_epoch();

    for job in jobs {
        if job.status != SyncJobStatus::Running {
            continue;
        }
        let claimed_at = job.claimed_at.unwrap_or(0);
        let threshold = (job.interval_secs * 2).max(STALE_JOB_THRESHOLD_SECS);
        if now.saturating_sub(claimed_at) > threshold {
            tracing::warn!(
                "scheduler: recovering stale job {} (claimed_by={:?})",
                job.id,
                job.claimed_by
            );
            let mut recovered = job;
            recovered.status = SyncJobStatus::Idle;
            recovered.claimed_by = None;
            recovered.claimed_at = None;
            recovered.next_run_at = now;
            state.core.metadata.put_sync_job(recovered).await?;
        }
    }

    Ok(())
}

async fn execute_due_jobs<M: SchedulerStore, B: BlobStore>(
    state: &Arc<AppState<M, B>>,
    node_id: &str,
    semaphore: &Arc<Semaphore>,
) {
    let jobs = match state.core.metadata.list_sync_jobs().await {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("scheduler: list jobs failed: {}", e);
            return;
        }
    };
    let now = now_epoch();

    for job in jobs {
        if job.status != SyncJobStatus::Idle || job.next_run_at > now {
            continue;
        }

        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => break,
        };

        match state.core.metadata.claim_sync_job(&job.id, node_id).await {
            Ok(true) => {}
            _ => {
                drop(permit);
                continue;
            }
        }

        let run = SyncJobRun::running(
            uuid::Uuid::new_v4().to_string(),
            job.id.clone(),
            node_id.to_string(),
            now,
        );
        if let Err(e) = state.core.metadata.put_sync_job_run(run.clone()).await {
            tracing::warn!(err = %e, "scheduler: failed to record sync run start");
        }

        let state = state.clone();
        let job_id = job.id.clone();
        let job_kind = job.kind.clone();
        let rule_id = job.rule_id.clone();
        let rule_name = job.rule_name.clone();
        let planned_image = job.image.clone();
        let planned_tags = job.tags.clone();
        let interval = job.interval_secs;
        let mut run = run;

        tokio::spawn(async move {
            let mut synced = Vec::new();
            let mut failed: Vec<(String, String)> = Vec::new();
            run.mark_resolution_started(now_epoch());
            persist_sync_run(&state.core.metadata, &run, "resolution start").await;

            let (direction, image, tags) = match (&job_kind, rule_id.as_deref()) {
                (SyncJobKind::Mirror, Some(rule_id)) => match state
                    .mirror
                    .resolve_mirror_job(rule_id, &state.core.metadata)
                    .await
                {
                    Ok(resolved) => (resolved.direction, resolved.local_repo, resolved.tags),
                    Err(e) => {
                        let message = e.to_string();
                        run.mark_resolution_failed(&message, now_epoch());
                        persist_sync_run(&state.core.metadata, &run, "resolution failure").await;
                        failed.push(("resolve".to_string(), message));
                        (MirrorDirection::Pull, planned_image.clone(), Vec::new())
                    }
                },
                (SyncJobKind::ProxyCache, Some(cache_id)) => match state
                    .mirror
                    .resolve_proxy_cache_job(cache_id, &state.core.metadata)
                    .await
                {
                    Ok((image, tags)) => (MirrorDirection::Pull, image, tags),
                    Err(e) => {
                        let message = e.to_string();
                        run.mark_resolution_failed(&message, now_epoch());
                        persist_sync_run(&state.core.metadata, &run, "resolution failure").await;
                        failed.push(("resolve".to_string(), message));
                        (MirrorDirection::Pull, planned_image.clone(), Vec::new())
                    }
                },
                _ => (
                    MirrorDirection::Pull,
                    planned_image.clone(),
                    planned_tags.clone(),
                ),
            };
            if failed.is_empty() {
                run.mark_resolution_finished(tags.len(), now_epoch());
                persist_sync_run(&state.core.metadata, &run, "resolution complete").await;
            }

            for tag in &tags {
                let tag_phase = if direction == MirrorDirection::Push {
                    "Pushing tag"
                } else {
                    "Pulling tag"
                };
                run.mark_tag_started(tag, tag_phase, now_epoch());
                persist_sync_run(&state.core.metadata, &run, "tag start").await;

                let result = if direction == MirrorDirection::Push {
                    match rule_id.as_deref() {
                        Some(rule_id) => {
                            state
                                .mirror
                                .push_manifest_for_rule(
                                    rule_id,
                                    tag,
                                    &state.core.metadata,
                                    &state.core.blobs,
                                )
                                .await
                        }
                        None => Err(crate::error::LayerhouseError::NameInvalid(
                            "push mirror job is missing rule_id".to_string(),
                        )),
                    }
                } else {
                    state
                        .mirror
                        .pull_manifest(&image, tag, &state.core.metadata, &state.core.blobs)
                        .await
                        .map(|entry| entry.is_some())
                };

                match result {
                    Ok(true) => {
                        tracing::info!("sync: {}:{} ok", image, tag);
                        synced.push(tag.clone());
                        run.mark_tag_finished(
                            tag,
                            SyncRunEventKind::Success,
                            "Synced tag",
                            synced.len() + failed.len(),
                            now_epoch(),
                        );
                        persist_sync_run(&state.core.metadata, &run, "tag success").await;
                    }
                    Ok(false) => {
                        let message = if direction == MirrorDirection::Push {
                            "not found locally"
                        } else {
                            "not found upstream"
                        };
                        tracing::warn!("sync: {}:{} {}", image, tag, message);
                        failed.push((tag.clone(), message.into()));
                        run.mark_tag_finished(
                            tag,
                            SyncRunEventKind::Warning,
                            message,
                            synced.len() + failed.len(),
                            now_epoch(),
                        );
                        persist_sync_run(&state.core.metadata, &run, "tag missing").await;
                    }
                    Err(e) => {
                        let message = e.to_string();
                        tracing::error!("sync: {}:{} failed: {}", image, tag, message);
                        failed.push((tag.clone(), message.clone()));
                        run.mark_tag_finished(
                            tag,
                            SyncRunEventKind::Error,
                            message,
                            synced.len() + failed.len(),
                            now_epoch(),
                        );
                        persist_sync_run(&state.core.metadata, &run, "tag failure").await;
                    }
                }
            }

            let now = now_epoch();
            let (run_status, last_error) = if failed.is_empty() {
                (SyncRunStatus::Succeeded, None)
            } else if synced.is_empty() {
                (SyncRunStatus::Failed, Some(failed[0].1.clone()))
            } else {
                (SyncRunStatus::PartialFailure, Some(failed[0].1.clone()))
            };

            let next_run_at = if interval == 0 {
                u64::MAX
            } else {
                now + interval
            };
            let updated_job = SyncJob {
                id: job_id.clone(),
                kind: job_kind,
                rule_id,
                rule_name,
                image,
                tags,
                interval_secs: interval,
                status: SyncJobStatus::Idle,
                claimed_by: None,
                claimed_at: None,
                last_run_at: Some(now),
                next_run_at,
                last_error,
            };
            if let Err(e) = state.core.metadata.put_sync_job(updated_job).await {
                tracing::warn!(err = %e, "scheduler: failed to update sync job status");
            }

            run.mark_finished(run_status, synced, failed, now);
            persist_sync_run(&state.core.metadata, &run, "completion").await;

            drop(permit);
        });
    }
}

async fn persist_sync_run<M: JobStore>(metadata: &M, run: &SyncJobRun, context: &str) {
    if let Err(e) = metadata.put_sync_job_run(run.clone()).await {
        tracing::warn!(
            err = %e,
            run_id = %run.id,
            context,
            "scheduler: failed to record sync run progress"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::metadata::{
        MirrorDirection, MirrorStrategy, OutboundProxy, ProxyCache, WarmFilter, WarmSortBy,
    };

    #[test]
    fn parses_common_crontab_intervals() {
        assert_eq!(schedule_interval_secs("*/30 * * * *"), Some(1_800));
        assert_eq!(schedule_interval_secs("* * * * *"), Some(60));
        assert_eq!(schedule_interval_secs("15 * * * *"), Some(3_600));
        assert_eq!(schedule_interval_secs("bad"), None);
    }

    #[test]
    fn builds_scheduled_mirror_job_from_rule() {
        let job = scheduled_mirror_job(
            &MirrorRule {
                id: "docker-nginx".to_string(),
                direction: MirrorDirection::Pull,
                local_prefix: "mirror/docker/nginx".to_string(),
                upstream_registry: "registry-1.docker.io".to_string(),
                upstream_prefix: Some("library/nginx".to_string()),
                schedule: Some("*/30 * * * *".to_string()),
                strategy: MirrorStrategy::Latest { count: 5 },
                plain_http: false,
                insecure_tls: false,
                outbound_proxy: OutboundProxy::default(),
                username: None,
                password: None,
                created_at: 1,
            },
            10,
        )
        .expect("scheduled job");

        assert_eq!(job.id, "mirror-rule-docker-nginx");
        assert_eq!(job.kind, SyncJobKind::Mirror);
        assert_eq!(job.rule_id.as_deref(), Some("docker-nginx"));
        assert_eq!(job.tags, vec!["latest 5"]);
        assert_eq!(job.interval_secs, 1_800);
        assert_eq!(job.next_run_at, 10);
    }

    #[test]
    fn builds_scheduled_proxy_cache_job_from_cache() {
        let job = scheduled_proxy_cache_job(
            &ProxyCache {
                id: "docker".to_string(),
                local_prefix: "cache/docker".to_string(),
                upstream_registry: "registry-1.docker.io".to_string(),
                upstream_prefix: Some("library".to_string()),
                warm_filters: vec![
                    WarmFilter::Latest {
                        count: 3,
                        sort_by: WarmSortBy::Pushed,
                    },
                    WarmFilter::Pattern {
                        pattern: "v2.*".to_string(),
                    },
                ],
                warm_schedule: Some("15 * * * *".to_string()),
                plain_http: false,
                insecure_tls: false,
                outbound_proxy: OutboundProxy::default(),
                username: None,
                password: None,
                created_at: 1,
            },
            20,
        )
        .expect("scheduled warm job");

        assert_eq!(job.id, "proxy-cache-docker-warm");
        assert_eq!(job.kind, SyncJobKind::ProxyCache);
        assert_eq!(job.rule_id.as_deref(), Some("docker"));
        assert_eq!(job.tags, vec!["latest 3", "v2.*"]);
        assert_eq!(job.interval_secs, 3_600);
        assert_eq!(job.next_run_at, 20);
    }
}
