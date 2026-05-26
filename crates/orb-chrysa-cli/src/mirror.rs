use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::BytesMut;
use tokio::sync::Semaphore;

use crate::registry::{
    BlobDescriptor, CHUNK_SIZE, ImageRef, ManifestData, RegistryClient, RegistryError, Result,
    extract_blob_descriptors, extract_child_manifests, format_size, is_index_manifest,
};

const MAX_SOURCE_RETRIES: u32 = 5;

pub struct MirrorOptions {
    pub concurrency: usize,
    pub all_tags: bool,
}

struct MirrorStats {
    blobs_transferred: AtomicU64,
    blobs_skipped: AtomicU64,
    bytes_transferred: AtomicU64,
    manifests_pushed: AtomicU64,
}

impl MirrorStats {
    fn new() -> Self {
        Self {
            blobs_transferred: AtomicU64::new(0),
            blobs_skipped: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            manifests_pushed: AtomicU64::new(0),
        }
    }
}

pub async fn mirror(src: ImageRef, dst: ImageRef, opts: MirrorOptions) -> Result<()> {
    let client = Arc::new(RegistryClient::new());

    client
        .ensure_auth(&src, "pull")
        .await
        .map_err(|e| e.context("source auth"))?;
    client
        .ensure_auth(&dst, "push,pull")
        .await
        .map_err(|e| e.context("destination auth"))?;

    let tags = if opts.all_tags {
        let tags = client
            .list_tags(&src)
            .await
            .map_err(|e| e.context("list source tags"))?;
        if tags.is_empty() {
            eprintln!("No tags found at {}", src.display());
            return Ok(());
        }
        eprintln!("Found {} tags to mirror", tags.len());
        tags
    } else {
        let tag = src
            .reference
            .as_deref()
            .ok_or_else(|| {
                RegistryError::Protocol("source must specify a tag or digest (or use --all)".into())
            })?
            .to_string();
        vec![tag]
    };

    let sem = Arc::new(Semaphore::new(opts.concurrency));
    let stats = Arc::new(MirrorStats::new());

    for (i, tag) in tags.iter().enumerate() {
        if opts.all_tags {
            eprintln!("[{}/{}] {}", i + 1, tags.len(), tag);
        }
        mirror_tag(&client, &src, &dst, tag, &sem, &stats)
            .await
            .map_err(|e| e.context(format!("mirror tag {}", tag)))?;
    }

    let transferred = stats.blobs_transferred.load(Ordering::Relaxed);
    let skipped = stats.blobs_skipped.load(Ordering::Relaxed);
    let bytes = stats.bytes_transferred.load(Ordering::Relaxed);
    let manifests = stats.manifests_pushed.load(Ordering::Relaxed);

    eprintln!(
        "\nDone: {} manifests, {} blobs transferred ({}), {} blobs skipped",
        manifests,
        transferred,
        format_size(bytes),
        skipped,
    );

    Ok(())
}

async fn mirror_tag(
    client: &Arc<RegistryClient>,
    src: &ImageRef,
    dst: &ImageRef,
    tag: &str,
    sem: &Arc<Semaphore>,
    stats: &Arc<MirrorStats>,
) -> Result<()> {
    let src_head = client.head_manifest(src, tag).await?;
    let Some((src_digest, _)) = src_head else {
        eprintln!("  tag {} not found at source, skipping", tag);
        return Ok(());
    };

    if let Some((dst_digest, _)) = client.head_manifest(dst, tag).await?
        && src_digest == dst_digest
    {
        eprintln!("  {} up to date ({})", tag, short_digest(&src_digest));
        return Ok(());
    }

    let manifest = client.get_manifest(src, tag).await?;
    mirror_manifest(client, src, dst, &manifest, tag, true, sem, stats).await
}

#[allow(clippy::too_many_arguments)]
async fn mirror_manifest(
    client: &Arc<RegistryClient>,
    src: &ImageRef,
    dst: &ImageRef,
    manifest: &ManifestData,
    reference: &str,
    is_tag: bool,
    sem: &Arc<Semaphore>,
    stats: &Arc<MirrorStats>,
) -> Result<()> {
    let parsed: serde_json::Value = serde_json::from_slice(&manifest.body)
        .map_err(|e| RegistryError::Protocol(format!("invalid manifest JSON: {}", e)))?;

    if is_index_manifest(&manifest.content_type) {
        let children = extract_child_manifests(&parsed);
        for child in &children {
            if client.head_manifest(dst, &child.digest).await?.is_some() {
                continue;
            }

            let child_manifest = client.get_manifest(src, &child.digest).await?;
            Box::pin(mirror_manifest(
                client,
                src,
                dst,
                &child_manifest,
                &child.digest,
                false,
                sem,
                stats,
            ))
            .await?;
        }
    } else {
        let blobs = extract_blob_descriptors(&parsed);
        transfer_blobs(client, src, dst, &blobs, sem, stats).await?;
    }

    let push_ref = if is_tag { reference } else { &manifest.digest };
    client
        .put_manifest(dst, push_ref, &manifest.body, &manifest.content_type)
        .await?;
    stats.manifests_pushed.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "  manifest {} pushed ({})",
        short_digest(&manifest.digest),
        push_ref
    );

    Ok(())
}

async fn transfer_blobs(
    client: &Arc<RegistryClient>,
    src: &ImageRef,
    dst: &ImageRef,
    blobs: &[BlobDescriptor],
    sem: &Arc<Semaphore>,
    stats: &Arc<MirrorStats>,
) -> Result<()> {
    let mut tasks = Vec::new();

    for blob in blobs {
        let sem = sem.clone();
        let client = client.clone();
        let src = src.clone();
        let dst = dst.clone();
        let stats = stats.clone();
        let digest = blob.digest.clone();
        let size = blob.size;

        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            transfer_single_blob(&client, &src, &dst, &digest, size, &stats).await
        }));
    }

    for task in tasks {
        task.await
            .map_err(|e| RegistryError::Http(format!("task join: {}", e)))??;
    }

    Ok(())
}

/// Transfer a single blob using chunked PATCH with per-chunk retry
/// and source-side resume on stream failure.
async fn transfer_single_blob(
    client: &RegistryClient,
    src: &ImageRef,
    dst: &ImageRef,
    digest: &str,
    size: u64,
    stats: &MirrorStats,
) -> Result<()> {
    let short = short_digest(digest);

    if client.head_blob(dst, digest).await? {
        eprintln!("  blob {} ({}) exists", short, format_size(size));
        stats.blobs_skipped.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }

    let upload_url = client.start_upload(dst).await?;
    let mut current_url = upload_url;
    let mut bytes_sent: u64 = 0;
    let mut source_retries: u32 = 0;

    'outer: while bytes_sent < size {
        let resp = client.get_blob_stream(src, digest, bytes_sent).await?;
        let mut stream = resp.into_bytes_stream();
        let mut buffer = BytesMut::new();

        loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    source_retries = 0;
                    buffer.extend_from_slice(&chunk);

                    while buffer.len() >= CHUNK_SIZE {
                        let to_send = buffer.split_to(CHUNK_SIZE).freeze();
                        let (new_url, new_offset) = client
                            .patch_chunk(dst, &current_url, to_send, bytes_sent)
                            .await?;
                        current_url = new_url;
                        bytes_sent = new_offset;
                    }
                }
                Some(Err(e)) => {
                    source_retries += 1;
                    if source_retries > MAX_SOURCE_RETRIES {
                        return Err(RegistryError::Http(format!(
                            "source stream failed after {} retries: {}",
                            MAX_SOURCE_RETRIES, e
                        )));
                    }
                    eprintln!(
                        "    source stream error at {}, retry {}/{}: {}",
                        format_size(bytes_sent + buffer.len() as u64),
                        source_retries,
                        MAX_SOURCE_RETRIES,
                        e
                    );

                    // Flush whatever we have in the buffer before resuming
                    if !buffer.is_empty() {
                        let to_send = buffer.split().freeze();
                        let (new_url, new_offset) = client
                            .patch_chunk(dst, &current_url, to_send, bytes_sent)
                            .await?;
                        current_url = new_url;
                        bytes_sent = new_offset;
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(
                        200 * 2u64.pow(source_retries - 1),
                    ))
                    .await;

                    // Re-open the source stream from where we left off
                    continue 'outer;
                }
                None => {
                    // Stream finished — flush remaining buffer
                    if !buffer.is_empty() {
                        let to_send = buffer.split().freeze();
                        let (new_url, _) = client
                            .patch_chunk(dst, &current_url, to_send, bytes_sent)
                            .await?;
                        current_url = new_url;
                    }
                    break 'outer;
                }
            }
        }
    }

    client.complete_upload(dst, &current_url, digest).await?;

    eprintln!("  blob {} ({}) transferred", short, format_size(size));
    stats.blobs_transferred.fetch_add(1, Ordering::Relaxed);
    stats.bytes_transferred.fetch_add(size, Ordering::Relaxed);
    Ok(())
}

fn short_digest(digest: &str) -> &str {
    if digest.len() > 19 {
        &digest[..19]
    } else {
        digest
    }
}
