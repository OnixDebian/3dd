//! Bollard isolation for image list + inspect (ENT-04).
//!
//! ONE call to `list_images(all=false)` returns image summaries; for each we
//! do an `inspect_image(id)` to read `root_fs.layers.len()`. Both happen
//! ONCE on startup (Phase 4 v1) — image sets are static enough that
//! per-event subscription is overkill.
//!
//! Bollard isolation invariant: bollard types appear ONLY in this file
//! (and the rest of `docker/*`). The compile-pin stub `_compile_check_signatures`
//! at the bottom of this file forces a compile error if the bollard surface
//! drifts; grep targets it from verify.

#![allow(dead_code)]

use bollard::query_parameters::ListImagesOptionsBuilder;
use bollard::Docker;

/// Bollard-free image record. `id` is the content-addressable digest, `repo_tag`
/// is the first repo tag (or "<untagged>"), `layer_count` is the length of
/// `root_fs.layers` from `inspect_image`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSnapshot {
    pub id: String,
    pub repo_tag: String,
    pub layer_count: usize,
}

/// Best-effort list-and-inspect pass. Returns whatever was successfully read
/// from the daemon — on `list_images` failure returns empty; on per-image
/// `inspect_image` failure skips that image and continues.
///
/// Runs ONCE on startup (called from `docker::streams::spawn_docker_tasks`
/// right after the container seed). The two calls per image (one list, N
/// inspects) are sequential here on purpose: 03-04's pre-tui probe already
/// validated the daemon handshake, and a typical developer machine has at
/// most a few dozen images — sequencing keeps the implementation simple and
/// the request rate gentle.
pub async fn fetch_image_snapshots(docker: &Docker) -> Vec<ImageSnapshot> {
    // all(false) skips intermediate untagged layers — we want only top-level
    // images the user `docker pull`-ed or built (the ones with a repo tag).
    let opts = ListImagesOptionsBuilder::new().all(false).build();
    let summaries = match docker.list_images(Some(opts)).await {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::with_capacity(summaries.len());
    for s in summaries {
        let id = s.id.clone();
        if id.is_empty() {
            continue;
        }
        // ImageSummary.repo_tags is a Vec<String> (non-optional). Empty when
        // the image has no tags ("<untagged>"); first entry otherwise.
        let repo_tag = s
            .repo_tags
            .first()
            .cloned()
            .unwrap_or_else(|| "<untagged>".to_string());
        let layer_count = match docker.inspect_image(&id).await {
            Ok(insp) => insp
                .root_fs
                .as_ref()
                .and_then(|fs| fs.layers.as_ref())
                .map(|v| v.len())
                .unwrap_or(0),
            // Per-image inspect failure: skip this entry, keep going. A
            // stale image id in the list response is the most common cause
            // (a `docker rmi` between list and inspect).
            Err(_) => continue,
        };
        out.push(ImageSnapshot {
            id,
            repo_tag,
            layer_count,
        });
    }
    out
}

/// Compile-pin (B3 closure): forces the bollard surface area (`Docker` handle,
/// `list_images`, `inspect_image`, `ListImagesOptionsBuilder`, `ImageInspect`)
/// to compile. If bollard renames a builder or shifts a return type, this
/// stub breaks the build BEFORE the live integration silently regresses.
///
/// Tagged `dead_code`; never called. Grepped by verify to confirm the
/// invariant is in place (does not depend on runtime tests, which would
/// require a live daemon).
#[allow(dead_code)]
fn _compile_check_signatures() {
    // Reference each bollard surface the live function uses, inside an
    // async fn the type-checker walks without dispatch. The async body
    // captures `docker` + `id` by value so there is no closure lifetime
    // gymnastics — keeps the compile-pin small and unambiguous.
    async fn _walk(docker: Docker, id: String) {
        let opts = ListImagesOptionsBuilder::new().all(false).build();
        let _ = docker.list_images(Some(opts)).await;
        let _ = docker.inspect_image(&id).await;
    }
    // Reference the function symbol so an accidental rename of `_walk` is
    // caught here (cheap immortalization of the bollard surface check).
    let _: fn(Docker, String) -> _ = _walk;
    // Reference our own bollard-free output so a rename of `ImageSnapshot`
    // is caught here too.
    let _: Option<ImageSnapshot> = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the public shape of `ImageSnapshot`. If a field is renamed/removed
    /// downstream (LiveWorld, scene_extras) breaks too — this test breaks first.
    #[test]
    fn image_snapshot_default_is_constructible() {
        let s = ImageSnapshot {
            id: String::new(),
            repo_tag: String::new(),
            layer_count: 0,
        };
        assert!(s.id.is_empty());
        assert!(s.repo_tag.is_empty());
        assert_eq!(s.layer_count, 0);
    }
}
