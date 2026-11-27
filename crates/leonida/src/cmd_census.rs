use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use shader_census::{Census, Selection, census_archive};

use crate::cli::CensusArgs;
use crate::cmd_shaders::with_archive;

pub fn census_command(args: &CensusArgs) -> Result<ExitCode> {
    for stage in &args.stage {
        if !["ps", "vs", "gs", "ms", "cs"].contains(&stage.as_str()) {
            bail!("unknown stage {stage}: use ps, vs, gs, ms or cs");
        }
    }
    let selection = Selection {
        stages: args.stage.clone(),
        limit: args.limit,
        every: args.every,
    };
    with_archive(args.game.as_ref(), |archive| {
        let census = Census::of(census_archive(archive, &selection));
        print!("{}", census.report());
        if args.json {
            std::fs::create_dir_all(&args.output)
                .with_context(|| format!("creating {}", args.output.display()))?;
            let path = args.output.join("census.json");
            let text = serde_json::to_string_pretty(&census)?;
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
            println!("wrote {}", path.display());
        }
        Ok(if census.programs.is_empty() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shader_container::testutil::{container, fssm, rpf_entry};

    fn args(game: &std::path::Path, output: &std::path::Path) -> CensusArgs {
        CensusArgs {
            game: Some(game.to_path_buf()),
            stage: Vec::new(),
            limit: None,
            every: 1,
            json: true,
            output: output.to_path_buf(),
        }
    }

    #[test]
    fn unknown_stage_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(dir.path(), dir.path());
        a.stage = vec!["hs".into()];
        assert!(census_command(&a).unwrap_err().to_string().contains("hs"));
    }

    #[test]
    fn census_of_a_synthetic_archive_is_printed_and_written() {
        let game = tempfile::tempdir().unwrap();
        let code = rdna::assemble("s_add_u32 s20, s8, s9", false).unwrap();
        let words: Vec<u32> = code
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect();
        let dxbc = container(0, &[fssm(false, &words)]);
        std::fs::write(
            game.path().join("shaders.rpf"),
            rpf_entry("PS_Test_01234567_Wrapped", 1, 1, &dxbc),
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        assert_eq!(
            census_command(&args(game.path(), out.path())).unwrap(),
            ExitCode::SUCCESS
        );
        let text = std::fs::read_to_string(out.path().join("census.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["stages"][0]["stage"], "ps");
        assert_eq!(json["programs"][0]["reads"], serde_json::json!([8, 9]));
    }
}
