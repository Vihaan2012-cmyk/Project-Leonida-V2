use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use shader_container::scan::{counts, range_name, wave_name};
use shader_container::{CodeRange, ShaderInfo, WaveSize};

use super::{card, header, later_button, planned, stat};
use crate::app::LeonidaApp;
use crate::theme;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShaderFilter {
    pub search: String,
    pub stage: Option<String>,
    pub wave: Option<WaveSize>,
    pub problems_only: bool,
}

pub fn filter_indices(shaders: &[ShaderInfo], filter: &ShaderFilter) -> Vec<usize> {
    let search = filter.search.trim().to_lowercase();
    shaders
        .iter()
        .enumerate()
        .filter(|(_, info)| search.is_empty() || info.name.to_lowercase().contains(&search))
        .filter(|(_, info)| {
            filter
                .stage
                .as_ref()
                .is_none_or(|stage| &info.stage_name() == stage)
        })
        .filter(|(_, info)| filter.wave.is_none_or(|wave| info.wave == Some(wave)))
        .filter(|(_, info)| !filter.problems_only || info.problem.is_some())
        .map(|(index, _)| index)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Overview,
    Disassembly,
    Ir,
    Spirv,
}

#[derive(Debug, Default)]
pub struct ShaderView {
    pub filter: ShaderFilter,
    filtered_for: Option<ShaderFilter>,
    pub filtered: Vec<usize>,
    pub selected: Option<usize>,
    pub tab: DetailTab,
}

impl ShaderView {
    fn refresh(&mut self, shaders: &[ShaderInfo]) {
        if self.filtered_for.as_ref() != Some(&self.filter) {
            self.filtered = filter_indices(shaders, &self.filter);
            self.filtered_for = Some(self.filter.clone());
        }
    }
}

impl LeonidaApp {
    pub(crate) fn shaders_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Shaders",
            "Every precompiled GPU program in shaders.rpf, read without touching its encrypted index.",
        );
        ui.horizontal(|ui| {
            let can_scan = self.game.path.is_some() && !self.jobs.is_busy();
            if ui
                .add_enabled(can_scan, egui::Button::new("🔎 Scan shaders"))
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.start_scan(&ctx);
            }
            later_button(ui, "⚙ Translate all", "M1.6–M1.8");
            later_button(ui, "💾 Export…", "M1.8");
            later_button(ui, "✏ Edit override…", "M1.8");
        });
        ui.add_space(8.0);

        let Some(summary) = &self.summary else {
            card(ui, |ui| {
                ui.label("No scan yet.");
                ui.weak("Choose your game folder on the Home page, then press Scan shaders.");
            });
            return;
        };

        ui.horizontal(|ui| {
            let problems: usize = summary.problems.values().sum();
            let wave = |name| summary.wave.get(name).copied().unwrap_or(0);
            ui.vertical(|ui| stat(ui, "containers", summary.carved, None));
            ui.vertical(|ui| stat(ui, "wave64", wave("wave64"), None));
            ui.vertical(|ui| stat(ui, "wave32", wave("wave32"), None));
            ui.vertical(|ui| stat(ui, "two payloads", summary.multi_payload, None));
            let color = if problems == 0 {
                theme::GOOD
            } else {
                theme::BAD
            };
            ui.vertical(|ui| stat(ui, "problems", problems, Some(color)));
        });
        ui.weak(format!("Container parts: {}", counts(&summary.parts)));
        ui.add_space(6.0);

        let stages: Vec<String> = summary.stages.keys().cloned().collect();
        let Some(scan) = &self.scan else { return };
        let view = &mut self.shader_view;

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut view.filter.search)
                    .hint_text("Search by name")
                    .desired_width(220.0),
            );
            egui::ComboBox::from_id_salt("stage-filter")
                .selected_text(
                    view.filter
                        .stage
                        .clone()
                        .unwrap_or_else(|| "All stages".into()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut view.filter.stage, None, "All stages");
                    for stage in &stages {
                        ui.selectable_value(&mut view.filter.stage, Some(stage.clone()), stage);
                    }
                });
            egui::ComboBox::from_id_salt("wave-filter")
                .selected_text(view.filter.wave.map_or("All wave sizes", wave_name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut view.filter.wave, None, "All wave sizes");
                    ui.selectable_value(&mut view.filter.wave, Some(WaveSize::Wave64), "wave64");
                    ui.selectable_value(&mut view.filter.wave, Some(WaveSize::Wave32), "wave32");
                });
            ui.checkbox(&mut view.filter.problems_only, "Problems only");
        });
        view.refresh(&scan.shaders);
        ui.weak(format!(
            "{} of {} shown",
            view.filtered.len(),
            scan.shaders.len()
        ));

        egui::Panel::right("shader-details")
            .default_size(360.0)
            .min_size(260.0)
            .show(ui, |ui| details(ui, view, &scan.shaders));

        let mut clicked = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .resizable(true)
            .column(Column::initial(300.0).at_least(140.0).clip(true))
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::remainder().clip(true))
            .header(24.0, |mut row| {
                for title in ["Name", "Stage", "Wave", "Payloads", "Size", "Problem"] {
                    row.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, view.filtered.len(), |mut row| {
                    let index = view.filtered[row.index()];
                    let info = &scan.shaders[index];
                    row.set_selected(view.selected == Some(index));
                    row.col(|ui| {
                        ui.label(&info.name);
                    });
                    row.col(|ui| {
                        ui.label(info.stage_name());
                    });
                    row.col(|ui| {
                        ui.label(info.wave.map_or("—", wave_name));
                    });
                    row.col(|ui| {
                        ui.label(info.payloads.to_string());
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.1} KiB", info.container_size as f32 / 1024.0));
                    });
                    row.col(|ui| {
                        if let Some(problem) = &info.problem {
                            ui.label(RichText::new(problem).color(theme::BAD));
                        }
                    });
                    if row.response().clicked() {
                        clicked = Some(index);
                    }
                });
            });
        if clicked.is_some() {
            view.selected = clicked;
        }
    }
}

fn details(ui: &mut egui::Ui, view: &mut ShaderView, shaders: &[ShaderInfo]) {
    let Some(info) = view.selected.and_then(|index| shaders.get(index)) else {
        ui.add_space(8.0);
        ui.weak("Select a shader to see its details.");
        return;
    };
    ui.add_space(6.0);
    ui.label(RichText::new(&info.name).strong().size(15.0));
    ui.horizontal(|ui| {
        ui.selectable_value(&mut view.tab, DetailTab::Overview, "Overview");
        ui.selectable_value(&mut view.tab, DetailTab::Disassembly, "Disassembly");
        ui.selectable_value(&mut view.tab, DetailTab::Ir, "IR");
        ui.selectable_value(&mut view.tab, DetailTab::Spirv, "SPIR-V");
    });
    ui.separator();
    match view.tab {
        DetailTab::Overview => overview(ui, info),
        DetailTab::Disassembly => planned(ui, "M1.2", "RDNA2 disassembly of this shader"),
        DetailTab::Ir => planned(ui, "M1.4", "The shader in Leonida's IR"),
        DetailTab::Spirv => planned(ui, "M1.6", "The translated SPIR-V, readable and editable"),
    }
}

fn overview(ui: &mut egui::Ui, info: &ShaderInfo) {
    egui::Grid::new("shader-overview")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, value: String| {
                ui.weak(label);
                ui.label(value);
                ui.end_row();
            };
            row(ui, "Stage", info.stage_name());
            row(
                ui,
                "Shader model",
                info.shader_model.map_or("—".into(), |model| {
                    format!("{}.{}", model.major, model.minor)
                }),
            );
            row(ui, "Archive offset", format!("0x{:X}", info.offset));
            row(ui, "Container", format!("{} bytes", info.container_size));
            row(ui, "Parts", info.parts.join(", "));
            row(ui, "FSSM payloads", info.payloads.to_string());
            row(
                ui,
                "FSSM size",
                info.fssm_size
                    .map_or("—".into(), |size| format!("{size} bytes")),
            );
            row(
                ui,
                "FSSM version",
                info.fssm_version.map_or("—".into(), |v| v.to_string()),
            );
            row(
                ui,
                "Wave size",
                info.wave
                    .map_or("—".into(), |wave| wave_name(wave).to_string()),
            );
            let range = match info.code_range {
                Some(CodeRange::Header { start, end }) => {
                    format!("0x{start:X}..0x{end:X} ({} dwords)", (end - start) / 4)
                }
                Some(CodeRange::Scan { end }) => format!("ends at 0x{end:X} (from scan)"),
                Some(range) => range_name(range).to_string(),
                None => "—".into(),
            };
            row(ui, "Code", range);
            row(
                ui,
                "Problem",
                info.problem.clone().unwrap_or_else(|| "none".into()),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use shader_container::Stage;

    fn info(name: &str, stage: Stage, wave: WaveSize, problem: Option<&str>) -> ShaderInfo {
        ShaderInfo {
            name: name.into(),
            offset: 0,
            stage: Some(stage),
            shader_model: None,
            container_size: 0,
            parts: Vec::new(),
            payloads: 1,
            fssm_size: None,
            fssm_version: None,
            wave: Some(wave),
            code_range: None,
            problem: problem.map(String::from),
        }
    }

    fn shaders() -> Vec<ShaderInfo> {
        vec![
            info("ps_Water", Stage::Pixel, WaveSize::Wave64, None),
            info("cs_Blur", Stage::Compute, WaveSize::Wave32, None),
            info("ps_sky", Stage::Pixel, WaveSize::Wave32, Some("broken")),
        ]
    }

    #[test]
    fn empty_filter_keeps_everything() {
        assert_eq!(
            filter_indices(&shaders(), &ShaderFilter::default()),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn filters_combine() {
        let mut filter = ShaderFilter {
            stage: Some("ps".into()),
            ..ShaderFilter::default()
        };
        assert_eq!(filter_indices(&shaders(), &filter), vec![0, 2]);
        filter.wave = Some(WaveSize::Wave32);
        assert_eq!(filter_indices(&shaders(), &filter), vec![2]);
        filter = ShaderFilter {
            problems_only: true,
            ..ShaderFilter::default()
        };
        assert_eq!(filter_indices(&shaders(), &filter), vec![2]);
    }

    #[test]
    fn search_ignores_case() {
        let filter = ShaderFilter {
            search: "  WATER ".into(),
            ..ShaderFilter::default()
        };
        assert_eq!(filter_indices(&shaders(), &filter), vec![0]);
    }

    #[test]
    fn view_recomputes_only_when_the_filter_changes() {
        let list = shaders();
        let mut view = ShaderView::default();
        view.refresh(&list);
        assert_eq!(view.filtered, vec![0, 1, 2]);
        view.filter.stage = Some("cs".into());
        view.refresh(&list);
        assert_eq!(view.filtered, vec![1]);
    }
}
