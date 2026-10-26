//! Text and JSON export preparation. Effects execute outside the model.

use super::{StatusTone, Tab, TuiModel};
use crate::date_display::{format_period_label, format_year_month_day};
use crate::tui::data::PeriodKind;
use crate::tui::effect::TuiEffect;
use tokenx_engine::ClientId;

fn client_ids_text(clients: &[ClientId]) -> String {
    clients
        .iter()
        .map(|client| client.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl TuiModel {
    fn selected_copy_text(&self) -> Option<String> {
        match self.current_tab {
            Tab::Overview | Tab::Models => {
                self.model_at_sorted_index(self.selected_index()).map(|m| {
                    rust_i18n::t!(
                        "tui.model.copy.label_tokens_cost",
                        label = m.display_name,
                        tokens = m.tokens.total(),
                        cost = format!("{:.4}", m.cost)
                    )
                    .into_owned()
                })
            }
            Tab::Agents => self
                .get_sorted_agents()
                .get(self.selected_index())
                .map(|a| {
                    rust_i18n::t!(
                        "tui.model.copy.agent_row",
                        client = a.client,
                        agent = a.agent,
                        tokens = a.tokens.total(),
                        cost = format!("{:.4}", a.cost),
                        instances = a.instance_count
                    )
                    .into_owned()
                }),
            Tab::Daily if self.is_daily_detail_active() => self
                .daily_detail_rows()
                .get(self.selected_index())
                .map(|row| {
                    rust_i18n::t!(
                        "tui.model.copy.detail_row",
                        clients = client_ids_text(&row.clients),
                        model = row.model,
                        tokens = row.tokens.total(),
                        cost = format!("{:.4}", row.cost)
                    )
                    .into_owned()
                }),
            Tab::Monthly | Tab::Weekly if self.is_period_detail_active() => self
                .period_detail_rows()
                .get(self.selected_index())
                .map(|row| {
                    rust_i18n::t!(
                        "tui.model.copy.detail_row",
                        clients = client_ids_text(&row.clients),
                        model = row.model,
                        tokens = row.tokens.total(),
                        cost = format!("{:.4}", row.cost)
                    )
                    .into_owned()
                }),
            Tab::Daily => self.daily_at_sorted_index(self.selected_index()).map(|d| {
                rust_i18n::t!(
                    "tui.model.copy.label_tokens_cost",
                    label = format_year_month_day(d.date),
                    tokens = d.tokens.total(),
                    cost = format!("{:.4}", d.cost)
                )
                .into_owned()
            }),
            Tab::Monthly => self
                .get_sorted_periods(PeriodKind::Monthly)
                .get(self.selected_index())
                .copied()
                .map(|p| {
                    rust_i18n::t!(
                        "tui.model.copy.period_row",
                        section = p.section_label,
                        label = format_period_label(
                            PeriodKind::Monthly,
                            p.start_date,
                            p.end_date,
                            false
                        ),
                        tokens = p.tokens.total(),
                        cost = format!("{:.4}", p.cost)
                    )
                    .into_owned()
                }),
            Tab::Weekly => self
                .get_sorted_periods(PeriodKind::Weekly)
                .get(self.selected_index())
                .copied()
                .map(|p| {
                    rust_i18n::t!(
                        "tui.model.copy.period_row",
                        section = p.section_label,
                        label = format_period_label(
                            PeriodKind::Weekly,
                            p.start_date,
                            p.end_date,
                            false
                        ),
                        tokens = p.tokens.total(),
                        cost = format!("{:.4}", p.cost)
                    )
                    .into_owned()
                }),
            Tab::Hourly => self.hourly_at_sorted_index(self.selected_index()).map(|h| {
                rust_i18n::t!(
                    "tui.model.copy.label_tokens_cost",
                    label = format!(
                        "{} {}",
                        format_year_month_day(h.datetime.date()),
                        h.datetime.format("%H:%M")
                    ),
                    tokens = h.tokens.total(),
                    cost = format!("{:.4}", h.cost)
                )
                .into_owned()
            }),
            Tab::Stats | Tab::Subscription | Tab::Sessions => None,
        }
    }

    pub(super) fn request_copy_selected(&mut self) {
        if let Some(text) = self.selected_copy_text() {
            self.effects.push_back(TuiEffect::CopyText { text });
        }
    }

    pub(super) fn export_group_by(&self) -> tokenx_engine::GroupBy {
        self.group_by()
    }

    pub(super) fn export_to_json(&mut self) {
        let filename = format!(
            "tokenx-export-{}.json",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        );
        let export_dir = self.product_paths.export_dir();
        let path = export_dir.join(filename);
        let group_by = self.export_group_by();

        let Some(health) = self.generation_health() else {
            self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.export_failed_not_loaded"),
                StatusTone::Danger,
            );
            return;
        };
        let health = health.clone();

        match crate::report::build_usage_report_json(self.usage(), &health, &group_by) {
            Ok(json) => {
                self.effects.push_back(TuiEffect::WriteExport {
                    directory: export_dir,
                    path,
                    json,
                });
            }
            Err(e) => self.set_status_with_tone(
                &rust_i18n::t!("tui.model.status.export_failed", error = e),
                StatusTone::Danger,
            ),
        }
    }
}
