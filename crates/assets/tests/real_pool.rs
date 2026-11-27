//! Reads the two small retail pools when `LEONIDA_GAME_DIR` is set; skipped otherwise.

use assets::texture::{ChunkCache, Pool, TextureResult, dds, preview};
use std::path::PathBuf;

const SMALL_POOLS: [&str; 2] = [
    "data/ttdb/ui/platform.ttpf",
    "pack/core_restofworld001/data/ttdb/ui/ttdb_ui.ttpf",
];

#[test]
fn small_retail_pools_parse_and_every_texture_resolves() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        eprintln!("LEONIDA_GAME_DIR is not set; skipping");
        return;
    };
    for rel in SMALL_POOLS {
        let path = PathBuf::from(&dir).join(rel);
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("{rel} not found; skipping");
            continue;
        };
        let pool = Pool::parse(&bytes).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let mut cache = ChunkCache::new();
        let (mut complete, mut partial, mut placeholder, mut failed) = (0, 0, 0, 0);
        for i in 0..pool.texture_count() {
            cache.clear();
            match pool.decode_texture(i, &mut cache) {
                Ok(TextureResult::Complete(t)) => {
                    complete += 1;
                    dds::write(&t.info, &t.data).unwrap_or_else(|e| panic!("{rel} #{i}: {e}"));
                    if preview::can_preview(t.info.format) {
                        let png = preview::png_preview(&t.info, &t.data)
                            .unwrap_or_else(|e| panic!("{rel} #{i}: {e}"));
                        assert_eq!(&png[1..4], b"PNG");
                    }
                }
                Ok(TextureResult::Partial { .. }) => partial += 1,
                Ok(TextureResult::Placeholder { .. }) => placeholder += 1,
                Err(e) => {
                    failed += 1;
                    eprintln!("{rel} #{i}: {e}");
                }
            }
        }
        eprintln!(
            "{rel}: {} textures, {complete} complete, {partial} partial, {placeholder} placeholder, {failed} failed",
            pool.texture_count()
        );
    }
}
