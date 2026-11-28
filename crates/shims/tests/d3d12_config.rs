//! The device settings `leonida run` hands the assembled device: the player's pipeline policy and the pipeline
//! files kept in the output folder between sessions.

use config::{Graphics, PipelineNotReady};
use shims::presentation::{self, Presentation};
use xbox_d3d12::compile_queue::DrawPolicy;

#[test]
fn the_pipeline_policy_and_files_come_from_the_settings_and_the_output_folder() {
    let output = tempfile::tempdir().unwrap();
    shims::loader::configure(None, output.path().to_path_buf());
    presentation::set(Presentation {
        graphics: Graphics {
            pipeline_not_ready: PipelineNotReady::Skip,
            ..Graphics::default()
        },
        ..Presentation::default()
    });

    let config = shims::d3d12::assembly_config();

    assert_eq!(config.draw_policy, DrawPolicy::Skip);
    if std::env::var_os("LEONIDA_PIPELINE_CACHE").is_none() {
        assert_eq!(
            config.pipeline_cache.as_deref(),
            Some(output.path().join("game-pipelines.cache").as_path())
        );
    }
    if std::env::var_os("LEONIDA_PIPELINES_SEEN").is_none() {
        assert_eq!(
            config.pipelines_seen.map(|seen| seen.path),
            Some(output.path().join("pipelines-seen"))
        );
    }

    presentation::set(Presentation::default());
    assert_eq!(
        shims::d3d12::assembly_config().draw_policy,
        DrawPolicy::Wait
    );
}
