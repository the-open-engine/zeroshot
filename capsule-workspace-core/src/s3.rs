//! `S3BlobStore` — real S3 behind the sync `BlobStore` trait (feature `s3`).
//!
//! Design (see `planning/plans/0001-production-backends.md`):
//! - The trait is SYNC and the whole publish/materialize pipeline is CPU-bound rayon batch work, so
//!   async (aws-sdk-s3) is CONTAINED here: the adapter holds one bounded **multi-thread** tokio
//!   runtime and `block_on`s the SDK futures inside each sync method. Multi-thread (not
//!   current-thread) because `materialize` calls `get_block` concurrently from `par_iter` → many
//!   concurrent `block_on`s (MF2). A `debug_assert` tripwire catches an accidental call from inside
//!   an async context (the one way this bet would panic: "runtime within a runtime").
//! - Content-addressed keys `blocks/<id>` / `manifests/<digest>`; writes are UNCONDITIONAL idempotent
//!   PutObject (overwrite is byte-identical, and re-upload is what restores a block GC deleted
//!   mid-race — OQ4). `get_*` map S3 `NoSuchKey` to a typed [`StoreError::NotFound`] distinct from a
//!   transient error. Endpoint/region/creds come from the default chain; `S3_ENDPOINT_URL` (present ⇒
//!   path-style) lets the SAME code hit MinIO/localstack locally and real S3 on AWS.

use crate::cas::{BlobStore, BlockId, StoreError};
use anyhow::{anyhow, Result};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use std::future::Future;

pub struct S3BlobStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    rt: tokio::runtime::Runtime,
}

fn block_key(id: &str) -> String {
    format!("blocks/{id}")
}
fn manifest_key(digest: &str) -> String {
    format!("manifests/{digest}")
}

/// Map any SDK error to a redacted `anyhow` error carrying the modeled service code (never the
/// request body or credentials). Mirrors zeroshot-cloud's `kms.rs` convention.
fn redacted<E, R>(ctx: &str, e: &SdkError<E, R>) -> anyhow::Error
where
    SdkError<E, R>: ProvideErrorMetadata,
{
    let code = e.code().unwrap_or("unknown");
    anyhow!("{ctx}: s3 error [{code}]")
}

impl S3BlobStore {
    /// Build from an explicit bucket + optional custom endpoint (MinIO/localstack). When an endpoint
    /// is given, path-style addressing is forced (MinIO can't do virtual-hosted buckets).
    pub fn new(bucket: impl Into<String>, endpoint: Option<String>) -> Result<Self> {
        // bounded so a small (2–4 vCPU) pod doesn't spin excess tokio workers against rayon.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        let client = rt.block_on(async {
            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(ep) = endpoint.as_deref() {
                loader = loader.endpoint_url(ep);
            }
            let sdk = loader.load().await;
            let mut cfg = aws_sdk_s3::config::Builder::from(&sdk);
            if endpoint.is_some() {
                cfg = cfg.force_path_style(true);
            }
            aws_sdk_s3::Client::from_conf(cfg.build())
        });
        Ok(Self {
            client,
            bucket: bucket.into(),
            rt,
        })
    }

    /// Build from the environment contract shared with zeroshot-cloud: `S3_BUCKET` (required),
    /// `S3_ENDPOINT_URL` (optional, MinIO/localstack). Region/creds via the default provider chain.
    pub fn from_env() -> Result<Self> {
        let bucket = std::env::var("S3_BUCKET")
            .map_err(|_| anyhow!("S3_BUCKET not set (required for S3BlobStore)"))?;
        let endpoint = std::env::var("S3_ENDPOINT_URL")
            .ok()
            .filter(|s| !s.is_empty());
        Self::new(bucket, endpoint)
    }

    /// `block_on` with the nested-runtime tripwire (MF2). Called only from sync (rayon/std) threads.
    fn on<F: Future>(&self, fut: F) -> F::Output {
        debug_assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "S3BlobStore method called from inside an async context — would nest-panic block_on"
        );
        self.rt.block_on(fut)
    }

    fn put(&self, key: String, bytes: &[u8], ctx: &str) -> Result<()> {
        let body = ByteStream::from(bytes.to_vec());
        self.on(async {
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .body(body)
                .send()
                .await
                .map_err(|e| redacted(ctx, &e))?;
            Ok(())
        })
    }

    fn get(&self, key: String, ctx: &str) -> Result<Vec<u8>> {
        self.on(async {
            let out = match self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&key)
                .send()
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    if let SdkError::ServiceError(se) = &e {
                        if se.err().is_no_such_key() {
                            return Err(StoreError::NotFound(key).into());
                        }
                    }
                    return Err(redacted(ctx, &e));
                }
            };
            let data = out
                .body
                .collect()
                .await
                .map_err(|e| anyhow!("{ctx}: body read failed: {e}"))?;
            Ok(data.into_bytes().to_vec())
        })
    }

    fn exists(&self, key: String) -> bool {
        self.on(async {
            self.client
                .head_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .is_ok()
        })
    }

    /// Idempotent delete with a faithful "did-remove" bool (HeadObject then DeleteObject). GC calls
    /// this only after winning the atomic `block_ref` claim, so the extra Head is off the hot path.
    /// NOTE: correct on a NON-versioned bucket (our test buckets); a versioned bucket needs
    /// DeleteObjectVersion to truly reclaim — tracked as a prod follow-up in the build log.
    fn del(&self, key: String, ctx: &str) -> Result<bool> {
        self.on(async {
            let existed = self
                .client
                .head_object()
                .bucket(&self.bucket)
                .key(&key)
                .send()
                .await
                .is_ok();
            if existed {
                self.client
                    .delete_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|e| redacted(ctx, &e))?;
            }
            Ok(existed)
        })
    }
}

impl BlobStore for S3BlobStore {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()> {
        self.put(block_key(id), bytes, "put_block")
    }
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>> {
        self.get(block_key(id), "get_block")
    }
    fn put_manifest(&self, digest: &str, bytes: &[u8]) -> Result<()> {
        self.put(manifest_key(digest), bytes, "put_manifest")
    }
    fn get_manifest(&self, digest: &str) -> Result<Vec<u8>> {
        self.get(manifest_key(digest), "get_manifest")
    }
    fn has_block(&self, id: &BlockId) -> bool {
        self.exists(block_key(id))
    }
    fn delete_block(&self, id: &BlockId) -> Result<bool> {
        self.del(block_key(id), "delete_block")
    }
    fn delete_manifest(&self, digest: &str) -> Result<bool> {
        self.del(manifest_key(digest), "delete_manifest")
    }
}
