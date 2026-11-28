//! DLSS quality modes, how the game's FidelityFX quality modes map onto them, and the feature-creation options.

use super::ffi;
use crate::QualityMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DlssMode {
    UltraPerformance,
    Performance,
    Balanced,
    Quality,
    UltraQuality,
    /// Native resolution anti-aliasing.
    Dlaa,
}

impl DlssMode {
    pub const ALL: [DlssMode; 6] = [
        DlssMode::UltraPerformance,
        DlssMode::Performance,
        DlssMode::Balanced,
        DlssMode::Quality,
        DlssMode::UltraQuality,
        DlssMode::Dlaa,
    ];

    /// The `PerfQualityValue` NGX takes.
    pub fn perf_quality(self) -> ffi::PerfQuality {
        match self {
            DlssMode::UltraPerformance => ffi::PERF_QUALITY_ULTRA_PERFORMANCE,
            DlssMode::Performance => ffi::PERF_QUALITY_MAX_PERF,
            DlssMode::Balanced => ffi::PERF_QUALITY_BALANCED,
            DlssMode::Quality => ffi::PERF_QUALITY_MAX_QUALITY,
            DlssMode::UltraQuality => ffi::PERF_QUALITY_ULTRA_QUALITY,
            DlssMode::Dlaa => ffi::PERF_QUALITY_DLAA,
        }
    }

    /// Output-to-render ratio NVIDIA documents for the mode.
    pub fn ratio(self) -> f32 {
        match self {
            DlssMode::UltraPerformance => 3.0,
            DlssMode::Performance => 2.0,
            DlssMode::Balanced => 1.724,
            DlssMode::Quality => 1.5,
            DlssMode::UltraQuality => 1.3,
            DlssMode::Dlaa => 1.0,
        }
    }

    /// The render size the documented ratio gives, for use when NGX cannot be asked.
    pub fn nominal_render_size(self, output: [u32; 2]) -> [u32; 2] {
        let r = self.ratio();
        [
            ((output[0] as f32 / r).round() as u32).max(1),
            ((output[1] as f32 / r).round() as u32).max(1),
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            DlssMode::UltraPerformance => "Ultra Performance",
            DlssMode::Performance => "Performance",
            DlssMode::Balanced => "Balanced",
            DlssMode::Quality => "Quality",
            DlssMode::UltraQuality => "Ultra Quality",
            DlssMode::Dlaa => "DLAA",
        }
    }

    pub fn from_quality_mode(mode: QualityMode) -> DlssMode {
        match mode {
            QualityMode::Native => DlssMode::Dlaa,
            QualityMode::Quality => DlssMode::Quality,
            QualityMode::Balanced => DlssMode::Balanced,
            QualityMode::Performance => DlssMode::Performance,
            QualityMode::UltraPerformance => DlssMode::UltraPerformance,
        }
    }

    /// From the quality value of the game's FidelityFX calls (0 native AA, 1 quality, 2 balanced, 3 performance,
    /// 4 ultra performance).
    pub fn from_ffx_quality(value: u32) -> Option<DlssMode> {
        Some(match value {
            0 => DlssMode::Dlaa,
            1 => DlssMode::Quality,
            2 => DlssMode::Balanced,
            3 => DlssMode::Performance,
            4 => DlssMode::UltraPerformance,
            _ => return None,
        })
    }

    /// The nearest FidelityFX-style mode, for callers that only know those.
    pub fn quality_mode(self) -> QualityMode {
        match self {
            DlssMode::UltraPerformance => QualityMode::UltraPerformance,
            DlssMode::Performance => QualityMode::Performance,
            DlssMode::Balanced => QualityMode::Balanced,
            DlssMode::Quality | DlssMode::UltraQuality => QualityMode::Quality,
            DlssMode::Dlaa => QualityMode::Native,
        }
    }
}

/// What the game's upscaler context says about its inputs; they decide the feature-creation flags.
#[derive(Debug, Clone, PartialEq)]
pub struct DlssOptions {
    pub mode: DlssMode,
    /// The colour is HDR (linear, above 1.0 possible).
    pub hdr: bool,
    /// The motion vectors are at render resolution (the game's usual layout).
    pub motion_vectors_low_res: bool,
    /// The jitter is already applied to the motion vectors.
    pub motion_vectors_jittered: bool,
    pub depth_inverted: bool,
    /// DLSS derives exposure itself instead of reading an exposure texture.
    pub auto_exposure: bool,
    /// Largest sizes the feature will see (a dynamic-resolution game creates for the maximum).
    pub max_render_size: Option<[u32; 2]>,
    /// The NGX application id. 0 is accepted by the driver for applications without an id from NVIDIA; the driver's
    /// project-id entry point is not used because its argument list is not published.
    pub application_id: u64,
    pub use_project_id: bool,
    pub project_id: String,
    pub engine_version: String,
    /// NGX logging: `LOGGING_OFF`, `LOGGING_ON` or `LOGGING_VERBOSE`. Lines are collected and read with
    /// [`super::ngx::take_log`]; the debug DLSS library (`dev` folder of the SDK) also draws an overlay.
    pub log_level: super::ffi::LoggingLevel,
    /// Folder NGX may write logs and caches to; the system temp folder when `None`.
    pub app_data_path: Option<std::path::PathBuf>,
}

impl Default for DlssOptions {
    fn default() -> Self {
        DlssOptions {
            mode: DlssMode::Quality,
            hdr: true,
            motion_vectors_low_res: true,
            motion_vectors_jittered: false,
            depth_inverted: true,
            auto_exposure: false,
            max_render_size: None,
            application_id: 0,
            use_project_id: true,
            project_id: "6f1f8c1b-2d9e-4b5a-9a3a-4c7d0e5b1a10".into(),
            engine_version: env!("CARGO_PKG_VERSION").into(),
            log_level: super::ffi::LOGGING_OFF,
            app_data_path: None,
        }
    }
}

impl DlssOptions {
    pub fn with_mode(mut self, mode: DlssMode) -> Self {
        self.mode = mode;
        self
    }

    /// `NVSDK_NGX_DLSS_Feature_Flags` for feature creation.
    pub fn feature_flags(&self) -> i32 {
        let mut flags = 0;
        if self.hdr {
            flags |= ffi::DLSS_FLAG_IS_HDR;
        }
        if self.motion_vectors_low_res {
            flags |= ffi::DLSS_FLAG_MV_LOW_RES;
        }
        if self.motion_vectors_jittered {
            flags |= ffi::DLSS_FLAG_MV_JITTERED;
        }
        if self.depth_inverted {
            flags |= ffi::DLSS_FLAG_DEPTH_INVERTED;
        }
        if self.auto_exposure {
            flags |= ffi::DLSS_FLAG_AUTO_EXPOSURE;
        }
        flags
    }
}

/// Smallest output NGX accepts: 64 wide, 32 high.
pub const MIN_OUTPUT_SIZE: [u32; 2] = [64, 32];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_modes_map_to_dlss_modes() {
        assert_eq!(
            DlssMode::from_quality_mode(QualityMode::Native),
            DlssMode::Dlaa
        );
        assert_eq!(
            DlssMode::from_quality_mode(QualityMode::Quality),
            DlssMode::Quality
        );
        assert_eq!(
            DlssMode::from_quality_mode(QualityMode::Balanced),
            DlssMode::Balanced
        );
        assert_eq!(
            DlssMode::from_quality_mode(QualityMode::Performance),
            DlssMode::Performance
        );
        assert_eq!(
            DlssMode::from_quality_mode(QualityMode::UltraPerformance),
            DlssMode::UltraPerformance
        );
        let ffx: Vec<_> = (0..5).map(DlssMode::from_ffx_quality).collect();
        assert_eq!(
            ffx,
            [
                Some(DlssMode::Dlaa),
                Some(DlssMode::Quality),
                Some(DlssMode::Balanced),
                Some(DlssMode::Performance),
                Some(DlssMode::UltraPerformance)
            ]
        );
        assert_eq!(DlssMode::from_ffx_quality(5), None);
    }

    #[test]
    fn perf_quality_values_are_the_ngx_ones() {
        let values: Vec<_> = DlssMode::ALL.iter().map(|m| m.perf_quality()).collect();
        assert_eq!(values, [3, 0, 1, 2, 4, 5]);
    }

    #[test]
    fn nominal_sizes_follow_the_documented_ratios() {
        let uhd = [3840, 2160];
        assert_eq!(DlssMode::Dlaa.nominal_render_size(uhd), uhd);
        assert_eq!(DlssMode::Quality.nominal_render_size(uhd), [2560, 1440]);
        assert_eq!(DlssMode::Performance.nominal_render_size(uhd), [1920, 1080]);
        assert_eq!(
            DlssMode::UltraPerformance.nominal_render_size(uhd),
            [1280, 720]
        );
        assert_eq!(DlssMode::Balanced.nominal_render_size(uhd), [2227, 1253]);
        assert_eq!(DlssMode::Quality.nominal_render_size([1, 1]), [1, 1]);
    }

    #[test]
    fn game_modes_round_trip_through_the_nearest_amd_style_mode() {
        for mode in [
            DlssMode::UltraPerformance,
            DlssMode::Performance,
            DlssMode::Balanced,
            DlssMode::Quality,
            DlssMode::Dlaa,
        ] {
            assert_eq!(DlssMode::from_quality_mode(mode.quality_mode()), mode);
        }
        assert_eq!(DlssMode::UltraQuality.quality_mode(), QualityMode::Quality);
    }

    #[test]
    fn creation_flags_follow_the_game_context() {
        let all = DlssOptions {
            hdr: true,
            motion_vectors_low_res: true,
            motion_vectors_jittered: true,
            depth_inverted: true,
            auto_exposure: true,
            ..DlssOptions::default()
        };
        assert_eq!(all.feature_flags(), 0x1 | 0x2 | 0x4 | 0x8 | 0x40);
        let none = DlssOptions {
            hdr: false,
            motion_vectors_low_res: false,
            motion_vectors_jittered: false,
            depth_inverted: false,
            auto_exposure: false,
            ..DlssOptions::default()
        };
        assert_eq!(none.feature_flags(), 0);
        assert_eq!(DlssOptions::default().feature_flags(), 0x1 | 0x2 | 0x8);
    }
}
