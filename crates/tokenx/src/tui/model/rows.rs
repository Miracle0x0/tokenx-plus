//! Borrowed report rows and cached render order for one installed projection.

use std::ops::Index;
use std::sync::Arc;

use chrono::NaiveDate;

use super::{ModelDetailSelection, PeriodDetailSelection, SortDirection, SortField, TuiModel};
use crate::date_display::format_period_label;
use crate::tui::data::{
    AgentEntry, DailyUsage, HourlyUsage, PeriodKind, PeriodUsage, UsageModelEntry,
};
use crate::tui::local_usage::DetailRow;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelOrderKey {
    usage_revision: u64,
    sort_field: SortField,
    sort_direction: SortDirection,
    detail: Option<ModelDetailSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsageOrderKey {
    usage_revision: u64,
    sort_field: SortField,
    sort_direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailOrderSelection {
    Daily(NaiveDate),
    Period(PeriodDetailSelection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetailOrderKey {
    usage_revision: u64,
    selection: DetailOrderSelection,
    sort_field: SortField,
    sort_direction: SortDirection,
}

#[derive(Debug)]
struct CachedRenderOrder<K> {
    key: K,
    order: Arc<[usize]>,
}

#[derive(Debug, Default)]
pub(super) struct RenderOrderCache {
    models: Option<CachedRenderOrder<ModelOrderKey>>,
    daily: Option<CachedRenderOrder<UsageOrderKey>>,
    hourly: Option<CachedRenderOrder<UsageOrderKey>>,
    detail: Option<CachedRenderOrder<DetailOrderKey>>,
}

fn sort_detail_order(
    order: &mut [usize],
    rows: &[DetailRow],
    field: SortField,
    direction: SortDirection,
) {
    let tie_breaker = |a: &DetailRow, b: &DetailRow| {
        a.clients
            .cmp(&b.clients)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.provider.cmp(&b.provider))
    };

    match (field, direction) {
        (SortField::Cost, SortDirection::Descending) => order.sort_by(|a, b| {
            rows[*b]
                .cost
                .total_cmp(&rows[*a].cost)
                .then_with(|| tie_breaker(&rows[*a], &rows[*b]))
        }),
        (SortField::Cost, SortDirection::Ascending) => order.sort_by(|a, b| {
            rows[*a]
                .cost
                .total_cmp(&rows[*b].cost)
                .then_with(|| tie_breaker(&rows[*a], &rows[*b]))
        }),
        (SortField::Tokens, SortDirection::Descending) => order.sort_by(|a, b| {
            rows[*b]
                .tokens
                .total()
                .cmp(&rows[*a].tokens.total())
                .then_with(|| tie_breaker(&rows[*a], &rows[*b]))
        }),
        (SortField::Tokens, SortDirection::Ascending) => order.sort_by(|a, b| {
            rows[*a]
                .tokens
                .total()
                .cmp(&rows[*b].tokens.total())
                .then_with(|| tie_breaker(&rows[*a], &rows[*b]))
        }),
        (SortField::Date, _) => {
            order.sort_by(|a, b| tie_breaker(&rows[*a], &rows[*b]));
        }
    }
}

pub(crate) struct OrderedDetailRows<'a> {
    pub(super) rows: &'a [DetailRow],
    pub(super) order: Arc<[usize]>,
}

impl<'a> OrderedDetailRows<'a> {
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&'a DetailRow> {
        self.rows.get(*self.order.get(index)?)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'a DetailRow> + '_ {
        self.order.iter().map(|index| &self.rows[*index])
    }
}

impl Index<usize> for OrderedDetailRows<'_> {
    type Output = DetailRow;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
            .expect("ordered detail row index is out of bounds")
    }
}

impl TuiModel {
    fn model_order_source(&self) -> &[UsageModelEntry] {
        if self.selected_model_detail.is_some() {
            self.model_detail_models.as_deref().unwrap_or_default()
        } else {
            &self.usage().models
        }
    }

    pub(crate) fn model_render_order(&self) -> Arc<[usize]> {
        let key = ModelOrderKey {
            usage_revision: self.usage_revision,
            sort_field: self.sort_field,
            sort_direction: self.sort_direction,
            detail: self.selected_model_detail.clone(),
        };
        if let Some(cached) = self
            .render_order_cache
            .borrow()
            .models
            .as_ref()
            .filter(|cached| cached.key == key)
        {
            return Arc::clone(&cached.order);
        }

        let source = self.model_order_source();
        let mut order = match self.selected_model_detail.as_ref() {
            Some(selection) => source
                .iter()
                .enumerate()
                .filter_map(|(index, model)| {
                    Self::model_detail_matches(selection, model).then_some(index)
                })
                .collect::<Vec<_>>(),
            None => (0..source.len()).collect::<Vec<_>>(),
        };

        let tie_breaker = |a: &UsageModelEntry, b: &UsageModelEntry| {
            a.model_id
                .cmp(&b.model_id)
                .then_with(|| a.workspace_label.cmp(&b.workspace_label))
                .then_with(|| a.workspace_key.cmp(&b.workspace_key))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.clients.cmp(&b.clients))
        };

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => order.sort_by(|a, b| {
                let a = &source[*a];
                let b = &source[*b];
                b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b))
            }),
            (SortField::Cost, SortDirection::Ascending) => order.sort_by(|a, b| {
                let a = &source[*a];
                let b = &source[*b];
                a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b))
            }),
            (SortField::Tokens, SortDirection::Descending) => order.sort_by(|a, b| {
                let a = &source[*a];
                let b = &source[*b];
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Tokens, SortDirection::Ascending) => order.sort_by(|a, b| {
                let a = &source[*a];
                let b = &source[*b];
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Date, _) => {
                order.sort_by(|a, b| tie_breaker(&source[*a], &source[*b]));
            }
        }

        let order: Arc<[usize]> = order.into();
        self.render_order_cache.borrow_mut().models = Some(CachedRenderOrder {
            key,
            order: Arc::clone(&order),
        });
        order
    }

    pub(crate) fn model_at_source_index(&self, index: usize) -> Option<&UsageModelEntry> {
        self.model_order_source().get(index)
    }

    pub(crate) fn model_at_sorted_index(&self, index: usize) -> Option<&UsageModelEntry> {
        let order = self.model_render_order();
        self.model_at_source_index(*order.get(index)?)
    }

    pub(crate) fn model_row_count(&self) -> usize {
        match self.selected_model_detail.as_ref() {
            Some(selection) => self
                .model_order_source()
                .iter()
                .filter(|model| Self::model_detail_matches(selection, model))
                .count(),
            None => self.usage().models.len(),
        }
    }

    #[cfg(test)]
    pub fn get_sorted_models(&self) -> Vec<&UsageModelEntry> {
        self.model_render_order()
            .iter()
            .map(|index| {
                self.model_at_source_index(*index)
                    .expect("cached model index must reference the current projection")
            })
            .collect()
    }

    pub fn get_sorted_agents(&self) -> Vec<&AgentEntry> {
        let mut agents: Vec<&AgentEntry> = self.usage().agents.iter().collect();

        let tie_breaker = |a: &&AgentEntry, b: &&AgentEntry| {
            a.agent.cmp(&b.agent).then_with(|| a.client.cmp(&b.client))
        };

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => {
                agents.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b)))
            }
            (SortField::Cost, SortDirection::Ascending) => {
                agents.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b)))
            }
            (SortField::Tokens, SortDirection::Descending) => agents.sort_by(|a, b| {
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Tokens, SortDirection::Ascending) => agents.sort_by(|a, b| {
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Date, _) => {
                agents.sort_by(|a, b| tie_breaker(a, b));
            }
        }

        agents
    }

    pub(crate) fn daily_render_order(&self) -> Arc<[usize]> {
        let key = UsageOrderKey {
            usage_revision: self.usage_revision,
            sort_field: self.sort_field,
            sort_direction: self.sort_direction,
        };
        if let Some(cached) = self
            .render_order_cache
            .borrow()
            .daily
            .as_ref()
            .filter(|cached| cached.key == key)
        {
            return Arc::clone(&cached.order);
        }

        let daily = &self.usage().daily;
        let mut order = (0..daily.len()).collect::<Vec<_>>();

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => order.sort_by(|a, b| {
                let a = &daily[*a];
                let b = &daily[*b];
                b.cost.total_cmp(&a.cost).then_with(|| a.date.cmp(&b.date))
            }),
            (SortField::Cost, SortDirection::Ascending) => order.sort_by(|a, b| {
                let a = &daily[*a];
                let b = &daily[*b];
                a.cost.total_cmp(&b.cost).then_with(|| a.date.cmp(&b.date))
            }),
            (SortField::Tokens, SortDirection::Descending) => order.sort_by(|a, b| {
                let a = &daily[*a];
                let b = &daily[*b];
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| a.date.cmp(&b.date))
            }),
            (SortField::Tokens, SortDirection::Ascending) => order.sort_by(|a, b| {
                let a = &daily[*a];
                let b = &daily[*b];
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| a.date.cmp(&b.date))
            }),
            (SortField::Date, SortDirection::Descending) => {
                order.sort_by_key(|index| std::cmp::Reverse(daily[*index].date))
            }
            (SortField::Date, SortDirection::Ascending) => {
                order.sort_by_key(|index| daily[*index].date)
            }
        }

        let order: Arc<[usize]> = order.into();
        self.render_order_cache.borrow_mut().daily = Some(CachedRenderOrder {
            key,
            order: Arc::clone(&order),
        });
        order
    }

    pub(crate) fn daily_at_sorted_index(&self, index: usize) -> Option<&DailyUsage> {
        let order = self.daily_render_order();
        self.usage().daily.get(*order.get(index)?)
    }

    #[cfg(test)]
    pub fn get_sorted_daily(&self) -> Vec<&DailyUsage> {
        self.daily_render_order()
            .iter()
            .map(|index| &self.usage().daily[*index])
            .collect()
    }

    pub fn is_daily_detail_active(&self) -> bool {
        self.selected_daily_detail_date.is_some()
    }

    pub fn daily_detail_date(&self) -> Option<NaiveDate> {
        self.selected_daily_detail_date
    }

    pub fn is_period_detail_active(&self) -> bool {
        self.selected_period_detail.is_some()
    }

    pub fn is_period_detail_active_for_kind(&self, kind: PeriodKind) -> bool {
        self.selected_period_detail
            .is_some_and(|selection| selection.kind == kind)
    }

    pub fn period_detail_label(&self) -> Option<String> {
        let selection = self.selected_period_detail?;
        self.period_usage(selection.kind)
            .iter()
            .find(|period| {
                period.start_date == selection.start_date && period.end_date == selection.end_date
            })
            .map(|period| {
                format!(
                    "{} {}",
                    period.section_label,
                    format_period_label(selection.kind, period.start_date, period.end_date, false)
                )
            })
    }

    fn detail_render_order(
        &self,
        selection: DetailOrderSelection,
        rows: &[DetailRow],
    ) -> Arc<[usize]> {
        let key = DetailOrderKey {
            usage_revision: self.usage_revision,
            selection,
            sort_field: self.sort_field,
            sort_direction: self.sort_direction,
        };
        if let Some(cached) = self
            .render_order_cache
            .borrow()
            .detail
            .as_ref()
            .filter(|cached| cached.key == key)
        {
            return Arc::clone(&cached.order);
        }

        let mut order = (0..rows.len()).collect::<Vec<_>>();
        sort_detail_order(&mut order, rows, self.sort_field, self.sort_direction);
        let order: Arc<[usize]> = order.into();
        self.render_order_cache.borrow_mut().detail = Some(CachedRenderOrder {
            key,
            order: Arc::clone(&order),
        });
        order
    }

    pub(crate) fn daily_detail_rows(&self) -> OrderedDetailRows<'_> {
        let Some(date) = self.selected_daily_detail_date else {
            return OrderedDetailRows {
                rows: &[],
                order: Arc::from([]),
            };
        };
        let rows = self.require_installed_generation().daily_detail(date);
        OrderedDetailRows {
            rows,
            order: self.detail_render_order(DetailOrderSelection::Daily(date), rows),
        }
    }

    pub(crate) fn daily_detail_row_count(&self) -> usize {
        let Some(date) = self.selected_daily_detail_date else {
            return 0;
        };
        self.require_installed_generation().daily_detail(date).len()
    }

    pub(crate) fn period_detail_rows(&self) -> OrderedDetailRows<'_> {
        let Some(selection) = self.selected_period_detail else {
            return OrderedDetailRows {
                rows: &[],
                order: Arc::from([]),
            };
        };
        let rows = self.require_installed_generation().period_detail(selection);
        OrderedDetailRows {
            rows,
            order: self.detail_render_order(DetailOrderSelection::Period(selection), rows),
        }
    }

    pub(crate) fn period_detail_row_count(&self) -> usize {
        let Some(selection) = self.selected_period_detail else {
            return 0;
        };
        self.require_installed_generation()
            .period_detail(selection)
            .len()
    }

    pub(crate) fn hourly_render_order(&self) -> Arc<[usize]> {
        let key = UsageOrderKey {
            usage_revision: self.usage_revision,
            sort_field: self.sort_field,
            sort_direction: self.sort_direction,
        };
        if let Some(cached) = self
            .render_order_cache
            .borrow()
            .hourly
            .as_ref()
            .filter(|cached| cached.key == key)
        {
            return Arc::clone(&cached.order);
        }

        let hourly = &self.usage().hourly;
        let mut order = (0..hourly.len()).collect::<Vec<_>>();

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => order.sort_by(|a, b| {
                let a = &hourly[*a];
                let b = &hourly[*b];
                b.cost
                    .total_cmp(&a.cost)
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Cost, SortDirection::Ascending) => order.sort_by(|a, b| {
                let a = &hourly[*a];
                let b = &hourly[*b];
                a.cost
                    .total_cmp(&b.cost)
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Tokens, SortDirection::Descending) => order.sort_by(|a, b| {
                let a = &hourly[*a];
                let b = &hourly[*b];
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Tokens, SortDirection::Ascending) => order.sort_by(|a, b| {
                let a = &hourly[*a];
                let b = &hourly[*b];
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Date, SortDirection::Descending) => {
                order.sort_by_key(|index| std::cmp::Reverse(hourly[*index].datetime))
            }
            (SortField::Date, SortDirection::Ascending) => {
                order.sort_by_key(|index| hourly[*index].datetime)
            }
        }

        let order: Arc<[usize]> = order.into();
        self.render_order_cache.borrow_mut().hourly = Some(CachedRenderOrder {
            key,
            order: Arc::clone(&order),
        });
        order
    }

    pub(crate) fn hourly_at_sorted_index(&self, index: usize) -> Option<&HourlyUsage> {
        let order = self.hourly_render_order();
        self.usage().hourly.get(*order.get(index)?)
    }

    pub fn get_sorted_periods(&self, kind: PeriodKind) -> Vec<&PeriodUsage> {
        let mut periods = self.period_usage(kind).iter().collect::<Vec<_>>();

        // Metric sorts keep Year sections newest-first; ordering is metric-based within each year.
        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => periods.sort_by(|a, b| {
                b.section_year
                    .cmp(&a.section_year)
                    .then_with(|| b.cost.total_cmp(&a.cost))
                    .then_with(|| b.start_date.cmp(&a.start_date))
            }),
            (SortField::Cost, SortDirection::Ascending) => periods.sort_by(|a, b| {
                b.section_year
                    .cmp(&a.section_year)
                    .then_with(|| a.cost.total_cmp(&b.cost))
                    .then_with(|| b.start_date.cmp(&a.start_date))
            }),
            (SortField::Tokens, SortDirection::Descending) => periods.sort_by(|a, b| {
                b.section_year
                    .cmp(&a.section_year)
                    .then_with(|| b.tokens.total().cmp(&a.tokens.total()))
                    .then_with(|| b.start_date.cmp(&a.start_date))
            }),
            (SortField::Tokens, SortDirection::Ascending) => periods.sort_by(|a, b| {
                b.section_year
                    .cmp(&a.section_year)
                    .then_with(|| a.tokens.total().cmp(&b.tokens.total()))
                    .then_with(|| b.start_date.cmp(&a.start_date))
            }),
            (SortField::Date, SortDirection::Descending) => {
                periods.sort_by_key(|period| std::cmp::Reverse(period.start_date))
            }
            (SortField::Date, SortDirection::Ascending) => {
                periods.sort_by_key(|period| period.start_date)
            }
        }

        periods
    }
}
