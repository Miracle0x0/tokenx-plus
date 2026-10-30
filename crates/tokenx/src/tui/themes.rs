use ratatui::style::{Color, Modifier, Style};
use tokenx_engine::{projection::ContributionGrade, ClientId};

pub(crate) use crate::theme::ThemeName;

use super::colors::IdentityPalette;
use super::contrast::{ensure_contrast, WCAG_NON_TEXT_CONTRAST};
use super::model_family::ModelFamily;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfacePalette {
    pub(crate) canvas: Color,
    pub(crate) panel: Color,
    pub(crate) row_alt: Color,
    pub(crate) row_current: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextPalette {
    pub(crate) primary: Color,
    pub(crate) secondary: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChromePalette {
    pub(crate) nav_active: Color,
    pub(crate) heading: Color,
    pub(crate) border: Color,
    pub(crate) focus: Color,
    pub(crate) current: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionPalette {
    pub(crate) background: Color,
    pub(crate) foreground: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MetricsPalette {
    pub(crate) tokens: Color,
    pub(crate) cost: Color,
    pub(crate) input: Color,
    pub(crate) output: Color,
    pub(crate) cache_read: Color,
    pub(crate) cache_write: Color,
    pub(crate) rate: Color,
    pub(crate) total: Color,
    pub(crate) secondary_cost: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatusPalette {
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    pub(crate) info: Color,
    pub(crate) pending: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisualizationPalette {
    pub(crate) contribution: ContributionPalette,
    pub(crate) track: Color,
    pub(crate) grid: Color,
    pub(crate) chart_highlight: Color,
    pub(crate) artwork: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContributionPalette {
    pub(crate) empty: Color,
    pub(crate) active: [Color; 4],
}

impl ContributionPalette {
    fn resolve(self, background: Color, blend_target: Color) -> Self {
        let resolve =
            |color| ensure_contrast(color, background, blend_target, WCAG_NON_TEXT_CONTRAST);
        Self {
            empty: resolve(self.empty),
            active: self.active.map(resolve),
        }
    }

    pub(crate) fn color(self, grade: ContributionGrade) -> Color {
        match grade {
            ContributionGrade::Empty => self.empty,
            ContributionGrade::Low => self.active[0],
            ContributionGrade::Medium => self.active[1],
            ContributionGrade::High => self.active[2],
            ContributionGrade::Peak => self.active[3],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: ThemeName,
    pub(crate) surface: SurfacePalette,
    pub(crate) text: TextPalette,
    pub(crate) chrome: ChromePalette,
    pub(crate) selection: SelectionPalette,
    pub(crate) metrics: MetricsPalette,
    pub(crate) status: StatusPalette,
    pub(crate) visualization: VisualizationPalette,
    identities: IdentityPalette,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThemeDefinition {
    surface: SurfacePalette,
    text: TextPalette,
    chrome: ChromePalette,
    selection: SelectionPalette,
    metrics: MetricsPalette,
    status: StatusPalette,
    visualization: VisualizationPalette,
}

impl Theme {
    pub fn from_name(name: ThemeName) -> Self {
        theme_for_name(name)
    }

    pub(crate) fn model_identity_color(&self, family: ModelFamily) -> Color {
        self.identities.model(family)
    }

    pub(crate) fn client_identity_color(&self, client: Option<ClientId>) -> Color {
        self.identities.client(client)
    }

    pub(crate) fn metric_input_style(&self) -> Style {
        Style::default().fg(self.metrics.input)
    }

    pub(crate) fn metric_output_style(&self) -> Style {
        Style::default().fg(self.metrics.output)
    }

    pub(crate) fn metric_cache_read_style(&self) -> Style {
        Style::default().fg(self.metrics.cache_read)
    }

    pub(crate) fn metric_cache_write_style(&self) -> Style {
        Style::default().fg(self.metrics.cache_write)
    }

    pub(crate) fn metric_total_style(&self) -> Style {
        Style::default()
            .fg(self.metrics.total)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn canvas_style(&self) -> Style {
        Style::default()
            .fg(self.text.primary)
            .bg(self.surface.canvas)
    }

    pub(crate) fn panel_style(&self) -> Style {
        Style::default()
            .fg(self.text.primary)
            .bg(self.surface.panel)
    }

    pub(crate) fn selection_style(&self) -> Style {
        Style::default()
            .fg(self.selection.foreground)
            .bg(self.selection.background)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn striped_row_style(&self) -> Style {
        Style::default().bg(self.surface.row_alt)
    }

    pub(crate) fn current_row_style(&self) -> Style {
        Style::default().bg(self.surface.row_current)
    }
}

fn theme_for_name(name: ThemeName) -> Theme {
    let definition = match name {
        ThemeName::Green => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(7, 15, 10),
                panel: rgb(12, 24, 17),
                row_alt: rgb(16, 32, 22),
                row_current: rgb(22, 57, 35),
            },
            text: TextPalette {
                primary: rgb(226, 244, 232),
                secondary: rgb(165, 195, 174),
            },
            chrome: ChromePalette {
                nav_active: rgb(126, 231, 135),
                heading: rgb(155, 233, 168),
                border: rgb(50, 82, 60),
                focus: rgb(86, 211, 100),
                current: rgb(110, 231, 125),
            },
            selection: SelectionPalette {
                background: rgb(24, 65, 36),
                foreground: rgb(240, 255, 244),
            },
            metrics: MetricsPalette {
                tokens: rgb(126, 231, 135),
                cost: rgb(255, 213, 102),
                input: rgb(115, 222, 147),
                output: rgb(255, 145, 145),
                cache_read: rgb(128, 196, 255),
                cache_write: rgb(255, 187, 120),
                rate: rgb(196, 181, 253),
                total: rgb(226, 244, 232),
                secondary_cost: rgb(207, 180, 110),
            },
            status: StatusPalette {
                success: rgb(126, 231, 135),
                warning: rgb(255, 213, 102),
                danger: rgb(255, 128, 128),
                info: rgb(128, 196, 255),
                pending: rgb(191, 201, 194),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(105, 132, 113),
                    active: [
                        rgb(33, 110, 57),
                        rgb(48, 161, 78),
                        rgb(64, 196, 99),
                        rgb(155, 233, 168),
                    ],
                },
                track: rgb(14, 29, 19),
                grid: rgb(40, 68, 49),
                chart_highlight: rgb(155, 233, 168),
                artwork: rgb(90, 205, 111),
            },
        },
        ThemeName::Halloween => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(20, 10, 3),
                panel: rgb(31, 17, 7),
                row_alt: rgb(42, 23, 8),
                row_current: rgb(72, 39, 8),
            },
            text: TextPalette {
                primary: rgb(255, 239, 211),
                secondary: rgb(218, 181, 135),
            },
            chrome: ChromePalette {
                nav_active: rgb(255, 177, 66),
                heading: rgb(255, 210, 108),
                border: rgb(104, 62, 26),
                focus: rgb(255, 145, 28),
                current: rgb(255, 197, 66),
            },
            selection: SelectionPalette {
                background: rgb(88, 40, 5),
                foreground: rgb(255, 246, 225),
            },
            metrics: MetricsPalette {
                tokens: rgb(255, 192, 82),
                cost: rgb(255, 225, 105),
                input: rgb(151, 218, 112),
                output: rgb(255, 132, 95),
                cache_read: rgb(120, 190, 255),
                cache_write: rgb(255, 175, 78),
                rate: rgb(230, 167, 255),
                total: rgb(255, 239, 211),
                secondary_cost: rgb(224, 187, 119),
            },
            status: StatusPalette {
                success: rgb(151, 218, 112),
                warning: rgb(255, 210, 82),
                danger: rgb(255, 120, 91),
                info: rgb(120, 190, 255),
                pending: rgb(218, 181, 135),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(145, 111, 76),
                    active: [
                        rgb(99, 29, 0),
                        rgb(254, 150, 0),
                        rgb(255, 197, 1),
                        rgb(255, 238, 74),
                    ],
                },
                track: rgb(38, 18, 4),
                grid: rgb(83, 45, 16),
                chart_highlight: rgb(255, 210, 82),
                artwork: rgb(255, 126, 35),
            },
        },
        ThemeName::Teal => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(4, 17, 18),
                panel: rgb(7, 27, 29),
                row_alt: rgb(9, 37, 39),
                row_current: rgb(10, 61, 62),
            },
            text: TextPalette {
                primary: rgb(218, 246, 245),
                secondary: rgb(151, 198, 196),
            },
            chrome: ChromePalette {
                nav_active: rgb(83, 224, 217),
                heading: rgb(126, 229, 229),
                border: rgb(34, 90, 91),
                focus: rgb(45, 197, 197),
                current: rgb(91, 218, 213),
            },
            selection: SelectionPalette {
                background: rgb(9, 63, 64),
                foreground: rgb(230, 255, 254),
            },
            metrics: MetricsPalette {
                tokens: rgb(83, 224, 217),
                cost: rgb(245, 216, 112),
                input: rgb(103, 219, 151),
                output: rgb(255, 137, 146),
                cache_read: rgb(116, 195, 255),
                cache_write: rgb(255, 181, 105),
                rate: rgb(202, 174, 255),
                total: rgb(218, 246, 245),
                secondary_cost: rgb(192, 190, 127),
            },
            status: StatusPalette {
                success: rgb(103, 219, 151),
                warning: rgb(245, 216, 112),
                danger: rgb(255, 125, 135),
                info: rgb(116, 195, 255),
                pending: rgb(174, 202, 201),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(91, 134, 133),
                    active: [
                        rgb(14, 109, 109),
                        rgb(13, 158, 158),
                        rgb(45, 197, 197),
                        rgb(126, 229, 229),
                    ],
                },
                track: rgb(7, 34, 35),
                grid: rgb(28, 72, 73),
                chart_highlight: rgb(126, 229, 229),
                artwork: rgb(47, 205, 196),
            },
        },
        ThemeName::Blue => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(6, 13, 24),
                panel: rgb(10, 22, 38),
                row_alt: rgb(13, 30, 50),
                row_current: rgb(17, 48, 82),
            },
            text: TextPalette {
                primary: rgb(224, 239, 255),
                secondary: rgb(155, 190, 225),
            },
            chrome: ChromePalette {
                nav_active: rgb(111, 181, 255),
                heading: rgb(146, 202, 255),
                border: rgb(43, 79, 118),
                focus: rgb(56, 139, 253),
                current: rgb(103, 175, 255),
            },
            selection: SelectionPalette {
                background: rgb(16, 57, 105),
                foreground: rgb(240, 249, 255),
            },
            metrics: MetricsPalette {
                tokens: rgb(111, 181, 255),
                cost: rgb(249, 218, 120),
                input: rgb(105, 220, 148),
                output: rgb(255, 137, 146),
                cache_read: rgb(130, 198, 255),
                cache_write: rgb(255, 181, 105),
                rate: rgb(192, 176, 255),
                total: rgb(224, 239, 255),
                secondary_cost: rgb(196, 190, 137),
            },
            status: StatusPalette {
                success: rgb(105, 220, 148),
                warning: rgb(249, 218, 120),
                danger: rgb(255, 125, 135),
                info: rgb(111, 181, 255),
                pending: rgb(168, 195, 222),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(91, 126, 162),
                    active: [
                        rgb(13, 65, 157),
                        rgb(31, 111, 235),
                        rgb(56, 139, 253),
                        rgb(121, 184, 255),
                    ],
                },
                track: rgb(10, 29, 53),
                grid: rgb(35, 66, 99),
                chart_highlight: rgb(146, 202, 255),
                artwork: rgb(74, 151, 255),
            },
        },
        ThemeName::Pink => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(22, 8, 17),
                panel: rgb(34, 13, 27),
                row_alt: rgb(45, 17, 36),
                row_current: rgb(72, 25, 54),
            },
            text: TextPalette {
                primary: rgb(255, 231, 244),
                secondary: rgb(222, 171, 200),
            },
            chrome: ChromePalette {
                nav_active: rgb(248, 148, 202),
                heading: rgb(255, 181, 220),
                border: rgb(105, 46, 79),
                focus: rgb(232, 104, 173),
                current: rgb(242, 135, 195),
            },
            selection: SelectionPalette {
                background: rgb(101, 32, 70),
                foreground: rgb(255, 240, 248),
            },
            metrics: MetricsPalette {
                tokens: rgb(248, 148, 202),
                cost: rgb(255, 219, 121),
                input: rgb(111, 221, 151),
                output: rgb(255, 139, 157),
                cache_read: rgb(126, 196, 255),
                cache_write: rgb(255, 183, 110),
                rate: rgb(205, 175, 255),
                total: rgb(255, 231, 244),
                secondary_cost: rgb(211, 181, 139),
            },
            status: StatusPalette {
                success: rgb(111, 221, 151),
                warning: rgb(255, 219, 121),
                danger: rgb(255, 127, 145),
                info: rgb(126, 196, 255),
                pending: rgb(207, 177, 194),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(151, 105, 132),
                    active: [
                        rgb(153, 40, 110),
                        rgb(191, 75, 138),
                        rgb(217, 97, 160),
                        rgb(240, 181, 210),
                    ],
                },
                track: rgb(46, 17, 35),
                grid: rgb(82, 37, 63),
                chart_highlight: rgb(255, 181, 220),
                artwork: rgb(229, 103, 173),
            },
        },
        ThemeName::Purple => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(15, 9, 25),
                panel: rgb(24, 14, 39),
                row_alt: rgb(32, 19, 51),
                row_current: rgb(51, 29, 79),
            },
            text: TextPalette {
                primary: rgb(243, 232, 255),
                secondary: rgb(195, 171, 222),
            },
            chrome: ChromePalette {
                nav_active: rgb(185, 142, 255),
                heading: rgb(211, 181, 255),
                border: rgb(79, 52, 110),
                focus: rgb(163, 113, 247),
                current: rgb(205, 174, 255),
            },
            selection: SelectionPalette {
                background: rgb(70, 39, 110),
                foreground: rgb(249, 241, 255),
            },
            metrics: MetricsPalette {
                tokens: rgb(185, 142, 255),
                cost: rgb(249, 218, 120),
                input: rgb(105, 220, 148),
                output: rgb(255, 137, 146),
                cache_read: rgb(126, 196, 255),
                cache_write: rgb(255, 181, 105),
                rate: rgb(214, 174, 255),
                total: rgb(243, 232, 255),
                secondary_cost: rgb(205, 185, 141),
            },
            status: StatusPalette {
                success: rgb(105, 220, 148),
                warning: rgb(249, 218, 120),
                danger: rgb(255, 125, 135),
                info: rgb(126, 196, 255),
                pending: rgb(190, 177, 205),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(126, 101, 153),
                    active: [
                        rgb(110, 64, 201),
                        rgb(137, 87, 229),
                        rgb(163, 113, 247),
                        rgb(205, 180, 255),
                    ],
                },
                track: rgb(32, 20, 53),
                grid: rgb(63, 42, 87),
                chart_highlight: rgb(211, 181, 255),
                artwork: rgb(168, 113, 247),
            },
        },
        ThemeName::Orange => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(21, 11, 5),
                panel: rgb(34, 19, 9),
                row_alt: rgb(45, 25, 11),
                row_current: rgb(73, 39, 13),
            },
            text: TextPalette {
                primary: rgb(255, 238, 219),
                secondary: rgb(221, 181, 145),
            },
            chrome: ChromePalette {
                nav_active: rgb(255, 138, 91),
                heading: rgb(255, 198, 128),
                border: rgb(105, 61, 29),
                focus: rgb(255, 140, 0),
                current: rgb(255, 175, 85),
            },
            selection: SelectionPalette {
                background: rgb(84, 42, 6),
                foreground: rgb(255, 244, 230),
            },
            metrics: MetricsPalette {
                tokens: rgb(255, 175, 85),
                cost: rgb(255, 220, 117),
                input: rgb(119, 219, 142),
                output: rgb(255, 132, 112),
                cache_read: rgb(122, 193, 255),
                cache_write: rgb(255, 177, 91),
                rate: rgb(207, 171, 255),
                total: rgb(255, 238, 219),
                secondary_cost: rgb(216, 182, 127),
            },
            status: StatusPalette {
                success: rgb(119, 219, 142),
                warning: rgb(255, 220, 117),
                danger: rgb(255, 120, 101),
                info: rgb(122, 193, 255),
                pending: rgb(215, 181, 151),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(149, 112, 82),
                    active: [
                        rgb(204, 85, 0),
                        rgb(255, 140, 0),
                        rgb(255, 179, 71),
                        rgb(255, 214, 153),
                    ],
                },
                track: rgb(43, 23, 8),
                grid: rgb(84, 48, 21),
                chart_highlight: rgb(255, 198, 128),
                artwork: rgb(255, 138, 31),
            },
        },
        ThemeName::Monochrome => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(10, 10, 10),
                panel: rgb(20, 20, 20),
                row_alt: rgb(28, 28, 28),
                row_current: rgb(48, 48, 48),
            },
            text: TextPalette {
                primary: rgb(238, 238, 238),
                secondary: rgb(184, 184, 184),
            },
            chrome: ChromePalette {
                nav_active: rgb(238, 238, 238),
                heading: rgb(214, 214, 214),
                border: rgb(76, 76, 76),
                focus: rgb(199, 199, 199),
                current: rgb(225, 225, 225),
            },
            selection: SelectionPalette {
                background: rgb(70, 70, 70),
                foreground: rgb(255, 255, 255),
            },
            metrics: MetricsPalette {
                tokens: rgb(220, 220, 220),
                cost: rgb(204, 204, 204),
                input: rgb(214, 214, 214),
                output: rgb(230, 230, 230),
                cache_read: rgb(194, 194, 194),
                cache_write: rgb(224, 224, 224),
                rate: rgb(201, 201, 201),
                total: rgb(245, 245, 245),
                secondary_cost: rgb(180, 180, 180),
            },
            status: StatusPalette {
                success: rgb(224, 224, 224),
                warning: rgb(207, 207, 207),
                danger: rgb(238, 238, 238),
                info: rgb(194, 194, 194),
                pending: rgb(180, 180, 180),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(119, 119, 119),
                    active: [
                        rgb(56, 56, 56),
                        rgb(82, 82, 82),
                        rgb(117, 117, 117),
                        rgb(158, 158, 158),
                    ],
                },
                track: rgb(22, 22, 22),
                grid: rgb(65, 65, 65),
                chart_highlight: rgb(225, 225, 225),
                artwork: rgb(170, 170, 170),
            },
        },
        ThemeName::YlGnBu => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(7, 15, 24),
                panel: rgb(11, 25, 36),
                row_alt: rgb(14, 34, 46),
                row_current: rgb(25, 56, 61),
            },
            text: TextPalette {
                primary: rgb(235, 246, 223),
                secondary: rgb(181, 205, 176),
            },
            chrome: ChromePalette {
                nav_active: rgb(216, 239, 116),
                heading: rgb(184, 232, 161),
                border: rgb(46, 85, 92),
                focus: rgb(92, 205, 187),
                current: rgb(183, 225, 128),
            },
            selection: SelectionPalette {
                background: rgb(23, 62, 69),
                foreground: rgb(244, 252, 231),
            },
            metrics: MetricsPalette {
                tokens: rgb(161, 218, 180),
                cost: rgb(236, 229, 120),
                input: rgb(116, 220, 145),
                output: rgb(255, 139, 139),
                cache_read: rgb(112, 199, 255),
                cache_write: rgb(255, 188, 104),
                rate: rgb(199, 179, 255),
                total: rgb(235, 246, 223),
                secondary_cost: rgb(194, 199, 139),
            },
            status: StatusPalette {
                success: rgb(116, 220, 145),
                warning: rgb(236, 229, 120),
                danger: rgb(255, 127, 132),
                info: rgb(112, 199, 255),
                pending: rgb(183, 201, 176),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(111, 139, 121),
                    active: [
                        rgb(37, 52, 148),
                        rgb(44, 127, 184),
                        rgb(65, 182, 196),
                        rgb(161, 218, 180),
                    ],
                },
                track: rgb(15, 32, 47),
                grid: rgb(38, 73, 82),
                chart_highlight: rgb(216, 239, 116),
                artwork: rgb(80, 197, 187),
            },
        },
        ThemeName::Graphite => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(8, 10, 14),
                panel: rgb(15, 18, 24),
                row_alt: rgb(21, 25, 33),
                row_current: rgb(27, 43, 48),
            },
            text: TextPalette {
                primary: rgb(226, 232, 240),
                secondary: rgb(166, 178, 194),
            },
            chrome: ChromePalette {
                nav_active: rgb(125, 211, 252),
                heading: rgb(186, 230, 253),
                border: rgb(55, 65, 81),
                focus: rgb(56, 189, 248),
                current: rgb(103, 201, 241),
            },
            selection: SelectionPalette {
                background: rgb(24, 61, 77),
                foreground: rgb(240, 249, 255),
            },
            metrics: MetricsPalette {
                tokens: rgb(125, 211, 252),
                cost: rgb(250, 213, 116),
                input: rgb(110, 220, 151),
                output: rgb(255, 137, 146),
                cache_read: rgb(125, 193, 255),
                cache_write: rgb(255, 183, 105),
                rate: rgb(196, 181, 253),
                total: rgb(226, 232, 240),
                secondary_cost: rgb(198, 188, 139),
            },
            status: StatusPalette {
                success: rgb(110, 220, 151),
                warning: rgb(250, 213, 116),
                danger: rgb(255, 125, 135),
                info: rgb(125, 211, 252),
                pending: rgb(166, 178, 194),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(103, 116, 133),
                    active: [
                        rgb(14, 116, 144),
                        rgb(148, 163, 184),
                        rgb(56, 189, 248),
                        rgb(125, 211, 252),
                    ],
                },
                track: rgb(24, 27, 34),
                grid: rgb(48, 58, 72),
                chart_highlight: rgb(186, 230, 253),
                artwork: rgb(56, 189, 248),
            },
        },
        ThemeName::Lagoon => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(4, 18, 21),
                panel: rgb(7, 28, 31),
                row_alt: rgb(9, 38, 42),
                row_current: rgb(16, 60, 53),
            },
            text: TextPalette {
                primary: rgb(216, 241, 238),
                secondary: rgb(157, 198, 194),
            },
            chrome: ChromePalette {
                nav_active: rgb(104, 240, 186),
                heading: rgb(153, 246, 228),
                border: rgb(31, 83, 88),
                focus: rgb(45, 212, 191),
                current: rgb(107, 230, 209),
            },
            selection: SelectionPalette {
                background: rgb(11, 65, 61),
                foreground: rgb(232, 255, 251),
            },
            metrics: MetricsPalette {
                tokens: rgb(94, 234, 212),
                cost: rgb(248, 220, 119),
                input: rgb(104, 224, 145),
                output: rgb(255, 139, 147),
                cache_read: rgb(117, 198, 255),
                cache_write: rgb(255, 183, 104),
                rate: rgb(197, 181, 253),
                total: rgb(216, 241, 238),
                secondary_cost: rgb(194, 193, 139),
            },
            status: StatusPalette {
                success: rgb(104, 224, 145),
                warning: rgb(248, 220, 119),
                danger: rgb(255, 127, 136),
                info: rgb(117, 198, 255),
                pending: rgb(170, 201, 198),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(93, 134, 132),
                    active: [
                        rgb(15, 118, 110),
                        rgb(45, 212, 191),
                        rgb(94, 234, 212),
                        rgb(153, 246, 228),
                    ],
                },
                track: rgb(6, 32, 36),
                grid: rgb(29, 76, 79),
                chart_highlight: rgb(153, 246, 228),
                artwork: rgb(45, 212, 191),
            },
        },
        ThemeName::Dusk => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(14, 12, 23),
                panel: rgb(23, 20, 34),
                row_alt: rgb(31, 27, 44),
                row_current: rgb(48, 42, 67),
            },
            text: TextPalette {
                primary: rgb(232, 226, 238),
                secondary: rgb(186, 174, 203),
            },
            chrome: ChromePalette {
                nav_active: rgb(224, 180, 255),
                heading: rgb(221, 214, 254),
                border: rgb(63, 57, 82),
                focus: rgb(167, 139, 250),
                current: rgb(192, 171, 252),
            },
            selection: SelectionPalette {
                background: rgb(60, 50, 94),
                foreground: rgb(248, 244, 255),
            },
            metrics: MetricsPalette {
                tokens: rgb(196, 181, 253),
                cost: rgb(249, 218, 120),
                input: rgb(105, 220, 148),
                output: rgb(255, 137, 146),
                cache_read: rgb(126, 196, 255),
                cache_write: rgb(255, 181, 105),
                rate: rgb(218, 175, 255),
                total: rgb(232, 226, 238),
                secondary_cost: rgb(203, 188, 144),
            },
            status: StatusPalette {
                success: rgb(105, 220, 148),
                warning: rgb(249, 218, 120),
                danger: rgb(255, 125, 135),
                info: rgb(126, 196, 255),
                pending: rgb(186, 174, 203),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(118, 105, 137),
                    active: [
                        rgb(109, 40, 217),
                        rgb(139, 92, 246),
                        rgb(167, 139, 250),
                        rgb(196, 181, 253),
                    ],
                },
                track: rgb(27, 24, 38),
                grid: rgb(55, 49, 72),
                chart_highlight: rgb(221, 214, 254),
                artwork: rgb(167, 139, 250),
            },
        },
        // Inkstone surfaces and pigments are adapted to the TUI contrast matrix;
        // the source and its license are recorded in NOTICE.
        ThemeName::InkstoneXuanmo => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(20, 19, 26),
                panel: rgb(26, 25, 34),
                row_alt: rgb(34, 33, 44),
                row_current: rgb(48, 46, 60),
            },
            text: TextPalette {
                primary: rgb(212, 206, 196),
                secondary: rgb(173, 165, 153),
            },
            chrome: ChromePalette {
                nav_active: rgb(107, 159, 195),
                heading: rgb(107, 159, 195),
                border: rgb(52, 50, 64),
                focus: rgb(107, 159, 195),
                current: rgb(107, 159, 195),
            },
            selection: SelectionPalette {
                background: rgb(51, 47, 70),
                foreground: rgb(212, 206, 196),
            },
            metrics: MetricsPalette {
                tokens: rgb(107, 159, 195),
                cost: rgb(214, 160, 29),
                input: rgb(43, 174, 133),
                output: rgb(215, 131, 119),
                cache_read: rgb(143, 178, 201),
                cache_write: rgb(214, 160, 29),
                rate: rgb(192, 142, 175),
                total: rgb(212, 206, 196),
                secondary_cost: rgb(214, 160, 29),
            },
            status: StatusPalette {
                success: rgb(43, 174, 133),
                warning: rgb(242, 142, 22),
                danger: rgb(221, 128, 104),
                info: rgb(107, 159, 195),
                pending: rgb(173, 165, 153),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(75, 106, 131),
                    active: [
                        rgb(75, 106, 131),
                        rgb(86, 124, 152),
                        rgb(96, 141, 174),
                        rgb(107, 159, 195),
                    ],
                },
                track: rgb(15, 14, 20),
                grid: rgb(52, 50, 64),
                chart_highlight: rgb(107, 159, 195),
                artwork: rgb(107, 159, 195),
            },
        },
        ThemeName::InkstoneXuanmoDimmed => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(25, 24, 30),
                panel: rgb(30, 29, 36),
                row_alt: rgb(36, 34, 43),
                row_current: rgb(46, 44, 54),
            },
            text: TextPalette {
                primary: rgb(216, 210, 198),
                secondary: rgb(176, 169, 159),
            },
            chrome: ChromePalette {
                nav_active: rgb(102, 169, 201),
                heading: rgb(102, 169, 201),
                border: rgb(57, 54, 67),
                focus: rgb(102, 169, 201),
                current: rgb(102, 169, 201),
            },
            selection: SelectionPalette {
                background: rgb(54, 51, 71),
                foreground: rgb(216, 210, 198),
            },
            metrics: MetricsPalette {
                tokens: rgb(102, 169, 201),
                cost: rgb(189, 153, 69),
                input: rgb(109, 169, 150),
                output: rgb(201, 144, 132),
                cache_read: rgb(115, 163, 195),
                cache_write: rgb(189, 153, 69),
                rate: rgb(214, 160, 29),
                total: rgb(216, 210, 198),
                secondary_cost: rgb(214, 160, 29),
            },
            status: StatusPalette {
                success: rgb(109, 169, 150),
                warning: rgb(189, 153, 69),
                danger: rgb(222, 135, 111),
                info: rgb(102, 169, 201),
                pending: rgb(176, 169, 159),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(72, 110, 131),
                    active: [
                        rgb(72, 110, 131),
                        rgb(82, 130, 154),
                        rgb(92, 149, 178),
                        rgb(102, 169, 201),
                    ],
                },
                track: rgb(21, 20, 25),
                grid: rgb(57, 54, 67),
                chart_highlight: rgb(102, 169, 201),
                artwork: rgb(102, 169, 201),
            },
        },
        ThemeName::InkstoneXuanzhi => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(247, 244, 237),
                panel: rgb(241, 236, 226),
                row_alt: rgb(234, 227, 213),
                row_current: rgb(228, 219, 204),
            },
            text: TextPalette {
                primary: rgb(53, 50, 45),
                secondary: rgb(91, 86, 76),
            },
            chrome: ChromePalette {
                nav_active: rgb(15, 89, 164),
                heading: rgb(15, 89, 164),
                border: rgb(214, 204, 185),
                focus: rgb(15, 89, 164),
                current: rgb(15, 89, 164),
            },
            selection: SelectionPalette {
                background: rgb(223, 211, 186),
                foreground: rgb(53, 50, 45),
            },
            metrics: MetricsPalette {
                tokens: rgb(15, 89, 164),
                cost: rgb(148, 69, 41),
                input: rgb(26, 104, 64),
                output: rgb(154, 63, 51),
                cache_read: rgb(46, 96, 136),
                cache_write: rgb(148, 69, 41),
                rate: rgb(129, 60, 133),
                total: rgb(53, 50, 45),
                secondary_cost: rgb(148, 69, 41),
            },
            status: StatusPalette {
                success: rgb(26, 104, 64),
                warning: rgb(148, 69, 41),
                danger: rgb(166, 49, 62),
                info: rgb(15, 89, 164),
                pending: rgb(91, 86, 76),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(91, 139, 185),
                    active: [
                        rgb(91, 139, 185),
                        rgb(66, 122, 178),
                        rgb(40, 106, 171),
                        rgb(15, 89, 164),
                    ],
                },
                track: rgb(255, 253, 248),
                grid: rgb(214, 204, 185),
                chart_highlight: rgb(15, 89, 164),
                artwork: rgb(15, 89, 164),
            },
        },
        ThemeName::InkstoneXuanzhiDimmed => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(233, 226, 212),
                panel: rgb(227, 219, 203),
                row_alt: rgb(220, 211, 193),
                row_current: rgb(216, 206, 187),
            },
            text: TextPalette {
                primary: rgb(47, 43, 37),
                secondary: rgb(79, 74, 64),
            },
            chrome: ChromePalette {
                nav_active: rgb(20, 74, 116),
                heading: rgb(20, 74, 116),
                border: rgb(203, 192, 171),
                focus: rgb(20, 74, 116),
                current: rgb(20, 74, 116),
            },
            selection: SelectionPalette {
                background: rgb(211, 199, 174),
                foreground: rgb(47, 43, 37),
            },
            metrics: MetricsPalette {
                tokens: rgb(20, 74, 116),
                cost: rgb(102, 70, 42),
                input: rgb(29, 95, 60),
                output: rgb(119, 71, 75),
                cache_read: rgb(21, 84, 152),
                cache_write: rgb(102, 70, 42),
                rate: rgb(80, 79, 129),
                total: rgb(47, 43, 37),
                secondary_cost: rgb(96, 84, 34),
            },
            status: StatusPalette {
                success: rgb(29, 95, 60),
                warning: rgb(135, 61, 36),
                danger: rgb(130, 17, 31),
                info: rgb(20, 74, 116),
                pending: rgb(79, 74, 64),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(96, 127, 148),
                    active: [
                        rgb(96, 127, 148),
                        rgb(71, 109, 137),
                        rgb(45, 92, 127),
                        rgb(20, 74, 116),
                    ],
                },
                track: rgb(241, 235, 223),
                grid: rgb(203, 192, 171),
                chart_highlight: rgb(20, 74, 116),
                artwork: rgb(20, 74, 116),
            },
        },
        ThemeName::InkstoneCanghai => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(13, 24, 34),
                panel: rgb(18, 32, 43),
                row_alt: rgb(24, 40, 51),
                row_current: rgb(32, 50, 63),
            },
            text: TextPalette {
                primary: rgb(200, 212, 221),
                secondary: rgb(154, 173, 187),
            },
            chrome: ChromePalette {
                nav_active: rgb(99, 187, 208),
                heading: rgb(99, 187, 208),
                border: rgb(41, 64, 79),
                focus: rgb(99, 187, 208),
                current: rgb(99, 187, 208),
            },
            selection: SelectionPalette {
                background: rgb(29, 54, 68),
                foreground: rgb(200, 212, 221),
            },
            metrics: MetricsPalette {
                tokens: rgb(99, 187, 208),
                cost: rgb(214, 160, 29),
                input: rgb(49, 173, 143),
                output: rgb(213, 134, 118),
                cache_read: rgb(52, 167, 198),
                cache_write: rgb(214, 160, 29),
                rate: rgb(192, 142, 175),
                total: rgb(200, 212, 221),
                secondary_cost: rgb(214, 160, 29),
            },
            status: StatusPalette {
                success: rgb(49, 173, 143),
                warning: rgb(214, 160, 29),
                danger: rgb(217, 132, 114),
                info: rgb(99, 187, 208),
                pending: rgb(154, 173, 187),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(60, 113, 129),
                    active: [
                        rgb(60, 113, 129),
                        rgb(73, 138, 155),
                        rgb(86, 162, 182),
                        rgb(99, 187, 208),
                    ],
                },
                track: rgb(10, 19, 27),
                grid: rgb(41, 64, 79),
                chart_highlight: rgb(99, 187, 208),
                artwork: rgb(99, 187, 208),
            },
        },
        ThemeName::InkstoneQingci => ThemeDefinition {
            surface: SurfacePalette {
                canvas: rgb(232, 240, 236),
                panel: rgb(225, 234, 228),
                row_alt: rgb(217, 227, 220),
                row_current: rgb(215, 226, 218),
            },
            text: TextPalette {
                primary: rgb(43, 51, 62),
                secondary: rgb(79, 91, 90),
            },
            chrome: ChromePalette {
                nav_active: rgb(22, 101, 120),
                heading: rgb(22, 101, 120),
                border: rgb(199, 213, 204),
                focus: rgb(22, 101, 120),
                current: rgb(22, 101, 120),
            },
            selection: SelectionPalette {
                background: rgb(199, 217, 207),
                foreground: rgb(43, 51, 62),
            },
            metrics: MetricsPalette {
                tokens: rgb(22, 101, 120),
                cost: rgb(102, 70, 42),
                input: rgb(26, 104, 64),
                output: rgb(160, 57, 68),
                cache_read: rgb(21, 85, 154),
                cache_write: rgb(102, 70, 42),
                rate: rgb(129, 60, 133),
                total: rgb(43, 51, 62),
                secondary_cost: rgb(100, 88, 34),
            },
            status: StatusPalette {
                success: rgb(26, 104, 64),
                warning: rgb(147, 70, 45),
                danger: rgb(130, 17, 31),
                info: rgb(22, 101, 120),
                pending: rgb(79, 91, 90),
            },
            visualization: VisualizationPalette {
                contribution: ContributionPalette {
                    empty: rgb(82, 140, 152),
                    active: [
                        rgb(82, 140, 152),
                        rgb(62, 127, 141),
                        rgb(42, 114, 131),
                        rgb(22, 101, 120),
                    ],
                },
                track: rgb(242, 248, 244),
                grid: rgb(199, 213, 204),
                chart_highlight: rgb(22, 101, 120),
                artwork: rgb(22, 101, 120),
            },
        },
    };
    let ThemeDefinition {
        surface,
        text,
        chrome,
        selection,
        metrics,
        status,
        mut visualization,
    } = definition;
    visualization.contribution = visualization
        .contribution
        .resolve(surface.panel, text.primary);
    let identities = IdentityPalette::resolve(
        [surface.panel, surface.row_alt, surface.row_current],
        text.primary,
    );

    Theme {
        name,
        surface,
        text,
        chrome,
        selection,
        metrics,
        status,
        visualization,
        identities,
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> Color {
    Color::Rgb(red, green, blue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::contrast::{contrast_ratio, WCAG_AA_TEXT_CONTRAST, WCAG_NON_TEXT_CONTRAST};

    #[test]
    fn theme_names_round_trip_through_settings_values() {
        for theme in ThemeName::all() {
            assert_eq!(theme.as_str().parse::<ThemeName>(), Ok(*theme));
        }
    }

    #[test]
    fn inkstone_themes_preserve_source_canvas_and_text_colors() {
        let cases = [
            (
                ThemeName::InkstoneXuanmo,
                rgb(20, 19, 26),
                rgb(212, 206, 196),
            ),
            (
                ThemeName::InkstoneXuanmoDimmed,
                rgb(25, 24, 30),
                rgb(216, 210, 198),
            ),
            (
                ThemeName::InkstoneXuanzhi,
                rgb(247, 244, 237),
                rgb(53, 50, 45),
            ),
            (
                ThemeName::InkstoneXuanzhiDimmed,
                rgb(233, 226, 212),
                rgb(47, 43, 37),
            ),
            (
                ThemeName::InkstoneCanghai,
                rgb(13, 24, 34),
                rgb(200, 212, 221),
            ),
            (
                ThemeName::InkstoneQingci,
                rgb(232, 240, 236),
                rgb(43, 51, 62),
            ),
        ];
        for (name, canvas, primary) in cases {
            let theme = Theme::from_name(name);
            assert_eq!(theme.surface.canvas, canvas, "{name:?} canvas");
            assert_eq!(theme.text.primary, primary, "{name:?} text");
        }
    }

    #[test]
    fn themes_have_distinct_surface_and_navigation_signatures() {
        for (index, &name) in ThemeName::all().iter().enumerate() {
            let theme = Theme::from_name(name);
            for &other_name in &ThemeName::all()[index + 1..] {
                let other = Theme::from_name(other_name);
                assert_ne!(
                    (
                        theme.surface.canvas,
                        theme.surface.panel,
                        theme.chrome.nav_active
                    ),
                    (
                        other.surface.canvas,
                        other.surface.panel,
                        other.chrome.nav_active
                    ),
                    "{name:?} and {other_name:?} share a theme signature"
                );
            }
        }
    }

    #[test]
    fn contribution_marks_are_visible_and_active_grades_increase_in_contrast() {
        const GRADES: [ContributionGrade; 5] = [
            ContributionGrade::Empty,
            ContributionGrade::Low,
            ContributionGrade::Medium,
            ContributionGrade::High,
            ContributionGrade::Peak,
        ];

        for &name in ThemeName::all() {
            let theme = Theme::from_name(name);
            let contribution = theme.visualization.contribution;

            for grade in GRADES {
                let contrast = contrast_ratio(contribution.color(grade), theme.surface.panel);
                assert!(
                    contrast >= WCAG_NON_TEXT_CONTRAST,
                    "{name:?} {grade:?} contribution contrast {contrast:.2} is below {WCAG_NON_TEXT_CONTRAST:.1}"
                );
            }

            for grades in contribution.active.windows(2) {
                let lower = contrast_ratio(grades[0], theme.surface.panel);
                let higher = contrast_ratio(grades[1], theme.surface.panel);
                assert!(
                    lower <= higher,
                    "{name:?} active grade contrast decreases from {lower:.4} to {higher:.4}"
                );
            }
            assert!(
                contrast_ratio(contribution.active[0], theme.surface.panel)
                    < contrast_ratio(contribution.active[3], theme.surface.panel),
                "{name:?} peak contribution must have more contrast than low contribution"
            );
        }
    }

    #[test]
    fn semantic_text_roles_are_readable_on_theme_surfaces() {
        for &name in ThemeName::all() {
            let theme = Theme::from_name(name);

            let panel_text = [
                ("primary", theme.text.primary),
                ("secondary", theme.text.secondary),
                ("active navigation", theme.chrome.nav_active),
                ("heading", theme.chrome.heading),
                ("focus", theme.chrome.focus),
                ("current chrome", theme.chrome.current),
                ("tokens", theme.metrics.tokens),
                ("cost", theme.metrics.cost),
                ("input", theme.metrics.input),
                ("output", theme.metrics.output),
                ("cache read", theme.metrics.cache_read),
                ("cache write", theme.metrics.cache_write),
                ("rate", theme.metrics.rate),
                ("total", theme.metrics.total),
                ("secondary cost", theme.metrics.secondary_cost),
                ("success", theme.status.success),
                ("warning", theme.status.warning),
                ("danger", theme.status.danger),
                ("info", theme.status.info),
                ("pending", theme.status.pending),
            ];

            for (role, color) in panel_text {
                let contrast = contrast_ratio(color, theme.surface.panel);
                assert!(
                    contrast >= WCAG_AA_TEXT_CONTRAST,
                    "{name:?} {role} on panel has contrast {contrast:.2}, below {WCAG_AA_TEXT_CONTRAST:.1}"
                );
            }

            let row_text = [
                ("primary", theme.text.primary),
                ("secondary", theme.text.secondary),
                ("current chrome", theme.chrome.current),
                ("tokens", theme.metrics.tokens),
                ("cost", theme.metrics.cost),
                ("input", theme.metrics.input),
                ("output", theme.metrics.output),
                ("cache read", theme.metrics.cache_read),
                ("cache write", theme.metrics.cache_write),
                ("rate", theme.metrics.rate),
                ("total", theme.metrics.total),
                ("secondary cost", theme.metrics.secondary_cost),
                ("success", theme.status.success),
                ("warning", theme.status.warning),
                ("danger", theme.status.danger),
                ("info", theme.status.info),
                ("pending", theme.status.pending),
            ];
            for (surface_name, background) in [
                ("alternate row", theme.surface.row_alt),
                ("current row", theme.surface.row_current),
            ] {
                for (role, color) in row_text {
                    let contrast = contrast_ratio(color, background);
                    assert!(
                        contrast >= WCAG_AA_TEXT_CONTRAST,
                        "{name:?} {role} on {surface_name} has contrast {contrast:.2}, below {WCAG_AA_TEXT_CONTRAST:.1}"
                    );
                }
            }

            for (role, color) in [
                ("primary", theme.text.primary),
                ("secondary", theme.text.secondary),
            ] {
                let contrast = contrast_ratio(color, theme.surface.canvas);
                assert!(
                    contrast >= WCAG_AA_TEXT_CONTRAST,
                    "{name:?} {role} on canvas has contrast {contrast:.2}, below {WCAG_AA_TEXT_CONTRAST:.1}"
                );
            }

            // Table cells carry their own foreground styles, so Row selection cannot
            // rely solely on SelectionPalette::foreground to remain readable.
            let selected_cell_text = [
                ("primary", theme.text.primary),
                ("secondary", theme.text.secondary),
                ("current chrome", theme.chrome.current),
                ("tokens", theme.metrics.tokens),
                ("cost", theme.metrics.cost),
                ("input", theme.metrics.input),
                ("output", theme.metrics.output),
                ("cache read", theme.metrics.cache_read),
                ("cache write", theme.metrics.cache_write),
                ("rate", theme.metrics.rate),
                ("total", theme.metrics.total),
                ("secondary cost", theme.metrics.secondary_cost),
                ("success", theme.status.success),
                ("warning", theme.status.warning),
                ("danger", theme.status.danger),
                ("info", theme.status.info),
                ("pending", theme.status.pending),
            ];
            for (role, color) in selected_cell_text {
                let contrast = contrast_ratio(color, theme.selection.background);
                assert!(
                    contrast >= WCAG_AA_TEXT_CONTRAST,
                    "{name:?} selected {role} contrast {contrast:.2} is below {WCAG_AA_TEXT_CONTRAST:.1}"
                );
            }

            let selection_contrast =
                contrast_ratio(theme.selection.foreground, theme.selection.background);
            assert!(
                selection_contrast >= WCAG_AA_TEXT_CONTRAST,
                "{name:?} selection contrast {selection_contrast:.2} is below {WCAG_AA_TEXT_CONTRAST:.1}"
            );
        }
    }

    #[test]
    fn core_status_roles_are_distinct_within_each_theme() {
        for &name in ThemeName::all() {
            let theme = Theme::from_name(name);
            let core_statuses = [
                ("success", theme.status.success),
                ("warning", theme.status.warning),
                ("danger", theme.status.danger),
            ];

            for (index, (role, color)) in core_statuses.iter().enumerate() {
                for (other_role, other_color) in &core_statuses[index + 1..] {
                    assert_ne!(
                        color, other_color,
                        "{name:?} {role} and {other_role} status colors must be distinct"
                    );
                }
            }
        }
    }

    #[test]
    fn identity_palettes_meet_text_contrast_on_every_render_surface() {
        for &name in ThemeName::all() {
            let theme = Theme::from_name(name);
            let surfaces = [
                ("panel", theme.surface.panel),
                ("alternate row", theme.surface.row_alt),
                ("current row", theme.surface.row_current),
            ];

            for family in ModelFamily::ALL {
                let color = theme.model_identity_color(family);
                for (surface_name, background) in surfaces {
                    let contrast = contrast_ratio(color, background);
                    assert!(
                        contrast >= WCAG_AA_TEXT_CONTRAST,
                        "{name:?} {family:?} model identity on {surface_name} has contrast {contrast:.2}"
                    );
                }
            }

            for client in ClientId::iter().map(Some).chain(std::iter::once(None)) {
                let color = theme.client_identity_color(client);
                for (surface_name, background) in surfaces {
                    let contrast = contrast_ratio(color, background);
                    assert!(
                        contrast >= WCAG_AA_TEXT_CONTRAST,
                        "{name:?} {client:?} client identity on {surface_name} has contrast {contrast:.2}"
                    );
                }
            }
        }
    }

    #[test]
    fn style_helpers_use_semantic_roles() {
        let theme = Theme::from_name(ThemeName::Blue);

        assert_eq!(theme.metric_input_style().fg, Some(theme.metrics.input));
        assert_eq!(theme.metric_output_style().fg, Some(theme.metrics.output));
        assert_eq!(
            theme.metric_cache_read_style().fg,
            Some(theme.metrics.cache_read)
        );
        assert_eq!(
            theme.metric_cache_write_style().fg,
            Some(theme.metrics.cache_write)
        );
        assert_eq!(theme.metric_total_style().fg, Some(theme.metrics.total));
        assert_eq!(theme.canvas_style().fg, Some(theme.text.primary));
        assert_eq!(theme.canvas_style().bg, Some(theme.surface.canvas));
        assert_eq!(theme.panel_style().fg, Some(theme.text.primary));
        assert_eq!(theme.panel_style().bg, Some(theme.surface.panel));
        assert_eq!(theme.selection_style().fg, Some(theme.selection.foreground));
        assert_eq!(theme.selection_style().bg, Some(theme.selection.background));
        assert!(theme
            .selection_style()
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(theme.striped_row_style().bg, Some(theme.surface.row_alt));
        assert_eq!(
            theme.current_row_style().bg,
            Some(theme.surface.row_current)
        );
    }
}
