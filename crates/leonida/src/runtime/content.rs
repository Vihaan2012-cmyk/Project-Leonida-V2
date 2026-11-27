//! The nudity filter: built from `Output/content-blocklist.json` when nudity is set to block, and installed where the
//! D3D12 layer finds it. Only nudity is ever filtered; gore and violence are not touched.

use anyhow::{Context, Result, bail};
use content_filter::global::install;
use content_filter::{Blocklist, ContentFilter, ContentSettings, NudityMode};

use super::plan::RuntimePlan;

/// The filter the plan asks for and a line to print about it.
pub fn build(plan: &RuntimePlan) -> Result<(ContentFilter, String)> {
    if !plan.block_nudity {
        return Ok((
            ContentFilter::disabled(),
            "nudity filter off (nudity is allowed)".to_string(),
        ));
    }
    if !plan.blocklist.is_file() {
        bail!(
            "nudity is set to block, but {} does not exist. Make it with `leonida content scan`, or set nudity = \"allow\" in leonida.toml",
            plan.blocklist.display()
        );
    }
    let list = Blocklist::load(&plan.blocklist)
        .with_context(|| format!("reading the nudity blocklist {}", plan.blocklist.display()))?;
    let filter = ContentFilter::new(
        ContentSettings {
            nudity: NudityMode::Block,
        },
        &list,
    );
    let line = if filter.is_active() {
        format!(
            "nudity filter on, {} resources blocked, {} catalogue rules",
            list.entries.len(),
            list.catalogue_rules.len()
        )
    } else {
        "nudity filter has nothing to block: the blocklist has no entries".to_string()
    };
    Ok((filter, line))
}

/// Builds the filter and installs it for the D3D12 layer, which finds it with `content_filter::global::get`.
pub fn configure(plan: &RuntimePlan) -> Result<String> {
    let (filter, line) = build(plan)?;
    if install(filter).is_err() {
        bail!("a content filter is already installed in this process");
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use content_filter::Replacement;
    use content_filter::blocklist::{BlockEntry, ResourceKind};
    use content_filter::names::Category;

    use super::*;
    use crate::cli::RunArgs;
    use crate::runtime::plan::Settings;

    fn plan_in(home: &Path, block: bool) -> RuntimePlan {
        let args = RunArgs {
            game: None,
            file: None,
            dry_run: false,
            start_the_game: true,
            all: false,
            trace: false,
            output: Some(home.join("Output")),
            config: home.join("Config"),
            frame_limit: None,
        };
        let mut settings = Settings::default();
        settings.general.content.nudity = if block {
            config::Nudity::Block
        } else {
            config::Nudity::Allow
        };
        RuntimePlan::from(&args, &settings, &home.join("Game")).unwrap()
    }

    fn entry() -> BlockEntry {
        BlockEntry {
            kind: ResourceKind::Drawable,
            category: Category::Genitals,
            name: Some("bodygenitalsgen_fr1_008_regular_ws00_full".into()),
            name_hash32: Some(0x1234_5678),
            keys64: vec![0x1111_2222_3333_4444],
            content: Vec::new(),
            replacement: Replacement::Hide,
        }
    }

    #[test]
    fn allowed_nudity_needs_no_blocklist() {
        let home = tempfile::tempdir().unwrap();
        let (filter, line) = build(&plan_in(home.path(), false)).unwrap();
        assert!(!filter.is_active());
        assert!(line.contains("off"), "{line}");
    }

    #[test]
    fn blocked_nudity_without_a_blocklist_stops_with_the_command_to_make_one() {
        let home = tempfile::tempdir().unwrap();
        let error = build(&plan_in(home.path(), true))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("content-blocklist.json"), "{error}");
        assert!(error.contains("leonida content scan"), "{error}");
    }

    #[test]
    fn a_blocklist_with_entries_makes_an_active_filter() {
        let home = tempfile::tempdir().unwrap();
        let plan = plan_in(home.path(), true);
        let mut list = Blocklist::new("1.0.2.1");
        list.entries.push(entry());
        list.save(&plan.blocklist).unwrap();
        let (filter, line) = build(&plan).unwrap();
        assert!(filter.is_active());
        assert!(line.contains("1 resources blocked"), "{line}");
    }

    #[test]
    fn an_empty_blocklist_is_reported_and_a_broken_one_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let plan = plan_in(home.path(), true);
        Blocklist::new("1.0.2.1").save(&plan.blocklist).unwrap();
        let (filter, line) = build(&plan).unwrap();
        assert!(!filter.is_active());
        assert!(line.contains("no entries"), "{line}");
        std::fs::write(&plan.blocklist, "{ not json").unwrap();
        let error = build(&plan).err().unwrap();
        assert!(
            format!("{error:#}").contains("content-blocklist.json"),
            "{error:#}"
        );
    }
}
